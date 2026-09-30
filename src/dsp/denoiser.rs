//! Wrapper around the DeepFilterNet3 real-time model (tract inference, CPU only).

use anyhow::{Context, Result};
use df::tract::{DfParams, DfTract, RuntimeParams};
use ndarray::Array2;

/// The model operates at 48 kHz on 10 ms hops.
pub const SAMPLE_RATE: usize = 48_000;
pub const HOP_SIZE: usize = 480;
/// Post-filter beta recommended by the DeepFilterNet authors.
pub const POST_FILTER_BETA: f32 = 0.02;

/// Local-SNR thresholds that decide which model stages run for a frame.
///
/// These are the values DeepFilterNet's own tools use. They span the model's whole
/// local-SNR range (-15..35 dB), so the mask decoder and the deep-filter decoder run on
/// every frame. Narrower thresholds (the library struct defaults are -10/30/20) skip
/// stages on some frames; the skipped decoder's recurrent state then goes stale and
/// speech below 4.8 kHz comes out garbled. `tests/pipeline.rs` guards against that.
pub const MIN_DB_THRESH: f32 = -15.0;
pub const MAX_DB_ERB_THRESH: f32 = 35.0;
pub const MAX_DB_DF_THRESH: f32 = 35.0;

pub struct Denoiser {
    model: DfTract,
    strength: u8,
    post_filter: bool,
    in_buf: Array2<f32>,
    out_buf: Array2<f32>,
}

impl Denoiser {
    /// Load the embedded model. Takes ~0.3-2 s depending on the CPU.
    pub fn new(strength: u8, post_filter: bool) -> Result<Self> {
        let params = DfParams::default();
        let runtime = RuntimeParams::default_with_ch(1)
            .with_atten_lim(strength_to_atten_db(strength))
            .with_post_filter(if post_filter { POST_FILTER_BETA } else { 0.0 })
            .with_thresholds(MIN_DB_THRESH, MAX_DB_ERB_THRESH, MAX_DB_DF_THRESH);
        let model = DfTract::new(params, &runtime).context("initialise DeepFilterNet model")?;
        anyhow::ensure!(
            model.sr == SAMPLE_RATE && model.hop_size == HOP_SIZE && model.ch == 1,
            "unexpected model format (sr={}, hop={}, ch={})",
            model.sr,
            model.hop_size,
            model.ch
        );
        Ok(Self {
            model,
            strength,
            post_filter,
            in_buf: Array2::zeros((1, HOP_SIZE)),
            out_buf: Array2::zeros((1, HOP_SIZE)),
        })
    }

    pub fn strength(&self) -> u8 {
        self.strength
    }

    pub fn set_strength(&mut self, strength: u8) {
        let strength = strength.min(100);
        if strength != self.strength {
            self.strength = strength;
            self.model.set_atten_lim(strength_to_atten_db(strength));
        }
    }

    pub fn post_filter(&self) -> bool {
        self.post_filter
    }

    pub fn set_post_filter(&mut self, on: bool) {
        if on != self.post_filter {
            self.post_filter = on;
            self.model
                .set_pf_beta(if on { POST_FILTER_BETA } else { 0.0 });
        }
    }

    /// Number of 10 ms frames the model looks ahead.
    pub fn lookahead_frames(&self) -> usize {
        self.model.lookahead
    }

    /// Delay introduced by the model between input and output, in samples at 48 kHz.
    pub fn delay_samples(&self) -> usize {
        (self.model.fft_size - self.model.hop_size) + self.model.lookahead * self.model.hop_size
    }

    pub fn algorithmic_latency_ms(&self) -> f32 {
        (self.delay_samples() + HOP_SIZE) as f32 * 1000.0 / SAMPLE_RATE as f32
    }

    /// Enhance one 480-sample frame. Returns the estimated local SNR in dB
    /// (-15 = only noise, 35 = clean speech).
    pub fn process(&mut self, input: &[f32], output: &mut [f32]) -> Result<f32> {
        debug_assert_eq!(input.len(), HOP_SIZE);
        debug_assert_eq!(output.len(), HOP_SIZE);
        self.in_buf
            .as_slice_mut()
            .expect("contiguous")
            .copy_from_slice(input);
        let lsnr = self
            .model
            .process(self.in_buf.view(), self.out_buf.view_mut())
            .context("DeepFilterNet inference")?;
        output.copy_from_slice(self.out_buf.as_slice().expect("contiguous"));
        Ok(lsnr)
    }
}

/// Map the UI strength (0..=100) to DeepFilterNet's attenuation limit in dB.
///
/// * 100 = unlimited suppression (model decides)
/// * 1..99 = 6 dB .. 50 dB limit, perceptually spaced
/// * 0 = handled by the pipeline as bypass
pub fn strength_to_atten_db(strength: u8) -> f32 {
    match strength.min(100) {
        0 => 0.0,
        100 => 100.0,
        s => {
            let t = s as f32 / 99.0;
            6.0 + 44.0 * t.powf(1.5)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strength_mapping_is_monotonic() {
        let mut last = -1.0;
        for s in 1..=100u8 {
            let db = strength_to_atten_db(s);
            assert!(db > last, "not monotonic at {s}");
            last = db;
        }
        assert_eq!(strength_to_atten_db(100), 100.0);
        assert!((strength_to_atten_db(1) - 6.0).abs() < 0.1);
    }
}
