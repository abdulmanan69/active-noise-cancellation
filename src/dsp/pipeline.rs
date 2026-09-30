//! Complete per-frame processing chain:
//! high-pass -> DeepFilterNet -> voice gate -> gain -> limiter.
//!
//! The gain sits after the model on purpose: boosting before it can push the model input
//! past full scale (a +12 dB setting reached 3.8x in the field), which distorts speech.

use anyhow::Result;

use super::denoiser::{Denoiser, HOP_SIZE, SAMPLE_RATE};
use super::filters::{HighPass, SmoothedGain, SoftLimiter, db_to_lin, peak_rms};
use super::gate::VoiceGate;

/// Local SNR (dB) above which a frame is considered to contain speech.
pub const VOICE_LSNR_DB: f32 = 2.0;
const HIGH_PASS_HZ: f32 = 80.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProcParams {
    /// Master switch. When false audio passes through untouched.
    pub enabled: bool,
    /// 0..=100 suppression strength (0 = bypass model).
    pub strength: u8,
    /// Extra spectral post-filter for stubborn stationary noise.
    pub post_filter: bool,
    /// Mute completely between words.
    pub voice_gate: bool,
    /// 80 Hz low-cut to remove rumble/handling noise before the model. Off by default:
    /// the model removes rumble on its own, and the filter rotates low-frequency phase
    /// (inaudible, but it lowers waveform-accuracy scores such as SI-SDR).
    pub high_pass: bool,
    /// Volume trim in dB (-24..=24), applied to the cleaned signal.
    pub input_gain_db: f32,
}

impl Default for ProcParams {
    fn default() -> Self {
        Self {
            enabled: true,
            strength: 85,
            post_filter: false,
            voice_gate: false,
            high_pass: false,
            input_gain_db: 0.0,
        }
    }
}

/// Per-frame diagnostics.
#[derive(Debug, Clone, Copy, Default)]
pub struct FrameInfo {
    pub lsnr_db: f32,
    pub voice: bool,
    pub gate_open: bool,
    pub processed: bool,
    pub input_peak: f32,
    pub input_rms: f32,
    pub output_peak: f32,
    pub output_rms: f32,
}

pub struct Pipeline {
    params: ProcParams,
    denoiser: Denoiser,
    high_pass: HighPass,
    gate: VoiceGate,
    limiter: SoftLimiter,
    gain: SmoothedGain,
    scratch: Vec<f32>,
}

impl Pipeline {
    pub fn new(params: ProcParams) -> Result<Self> {
        let sr = SAMPLE_RATE as f32;
        let denoiser = Denoiser::new(params.strength.max(1), params.post_filter)?;
        Ok(Self {
            params,
            denoiser,
            high_pass: HighPass::new(sr, HIGH_PASS_HZ),
            gate: VoiceGate::new(sr, HOP_SIZE),
            limiter: SoftLimiter::new(0.9),
            gain: SmoothedGain::new(sr, 20.0, db_to_lin(params.input_gain_db)),
            scratch: vec![0.0; HOP_SIZE],
        })
    }

    pub fn params(&self) -> ProcParams {
        self.params
    }

    pub fn set_params(&mut self, p: ProcParams) {
        if p.strength > 0 {
            self.denoiser.set_strength(p.strength);
        }
        self.denoiser.set_post_filter(p.post_filter);
        if p.high_pass != self.params.high_pass {
            self.high_pass.reset();
        }
        if !p.voice_gate || !p.enabled {
            self.gate.reset();
        }
        self.gain.set_target(db_to_lin(p.input_gain_db.clamp(-24.0, 24.0)));
        self.params = p;
    }

    pub fn denoiser(&self) -> &Denoiser {
        &self.denoiser
    }

    pub fn algorithmic_latency_ms(&self) -> f32 {
        self.denoiser.algorithmic_latency_ms()
    }

    /// Delay between input and output in samples (48 kHz) when the model is active.
    pub fn delay_samples(&self) -> usize {
        self.denoiser.delay_samples()
    }

    /// Process one 480-sample frame in place.
    pub fn process_frame(&mut self, frame: &mut [f32]) -> Result<FrameInfo> {
        debug_assert_eq!(frame.len(), HOP_SIZE);
        let (input_peak, input_rms) = peak_rms(frame);

        let active = self.params.enabled && self.params.strength > 0;
        if self.params.high_pass && active {
            self.high_pass.process(frame);
        }

        let mut lsnr_db = 35.0;
        let mut voice = input_rms > 1e-4;
        if active {
            lsnr_db = self.denoiser.process(frame, &mut self.scratch)?;
            frame.copy_from_slice(&self.scratch);
            voice = lsnr_db > VOICE_LSNR_DB;
        }

        let gate_open = if active && self.params.voice_gate {
            self.gate.process(lsnr_db, frame)
        } else {
            true
        };

        self.gain.process(frame);
        self.limiter.process(frame);
        let (output_peak, output_rms) = peak_rms(frame);
        Ok(FrameInfo {
            lsnr_db,
            voice,
            gate_open,
            processed: active,
            input_peak,
            input_rms,
            output_peak,
            output_rms,
        })
    }
}
