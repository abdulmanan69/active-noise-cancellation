//! Voice-activity gate driven by the denoiser's local SNR estimate.
//!
//! DeepFilterNet already zeroes frames that contain only noise; the gate adds
//! a hold/release stage so residual breathing and keyboard bursts between
//! words are fully muted, similar to Krisp's "voice only" behaviour.

use super::filters::one_pole_coef;

#[derive(Debug, Clone)]
pub struct VoiceGate {
    open_threshold_db: f32,
    close_threshold_db: f32,
    hold_frames: u32,
    frames_below: u32,
    gain: f32,
    target: f32,
    attack_coef: f32,
    release_coef: f32,
}

impl VoiceGate {
    pub fn new(sample_rate: f32, hop: usize) -> Self {
        let frame_ms = hop as f32 * 1000.0 / sample_rate;
        Self {
            open_threshold_db: 0.0,
            close_threshold_db: -4.0,
            hold_frames: (400.0 / frame_ms).round().max(1.0) as u32,
            frames_below: 0,
            gain: 1.0,
            target: 1.0,
            attack_coef: one_pole_coef(sample_rate, 3.0),
            release_coef: one_pole_coef(sample_rate, 60.0),
        }
    }

    pub fn reset(&mut self) {
        self.gain = 1.0;
        self.target = 1.0;
        self.frames_below = 0;
    }

    pub fn is_open(&self) -> bool {
        self.target >= 0.5
    }

    /// Apply the gate to one frame. Returns whether the gate is open afterwards.
    pub fn process(&mut self, lsnr_db: f32, frame: &mut [f32]) -> bool {
        if lsnr_db >= self.open_threshold_db {
            self.frames_below = 0;
            self.target = 1.0;
        } else if lsnr_db < self.close_threshold_db {
            self.frames_below = self.frames_below.saturating_add(1);
            if self.frames_below >= self.hold_frames {
                self.target = 0.0;
            }
        }
        if (self.gain - self.target).abs() < 1e-5 {
            self.gain = self.target;
            if self.gain == 0.0 {
                frame.fill(0.0);
            }
        } else {
            let coef = if self.target > self.gain {
                self.attack_coef
            } else {
                self.release_coef
            };
            for s in frame.iter_mut() {
                self.gain += (self.target - self.gain) * coef;
                *s *= self.gain;
            }
        }
        self.is_open()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_closes_after_hold_and_reopens_fast() {
        let mut g = VoiceGate::new(48_000.0, 480);
        let mut frame = vec![0.5f32; 480];
        assert!(g.process(20.0, &mut frame));
        // 400 ms hold = 40 frames of silence before it closes
        for _ in 0..39 {
            assert!(g.process(-15.0, &mut frame));
        }
        assert!(!g.process(-15.0, &mut frame));
        // let the 60 ms release finish (600 ms is ten time constants)
        for _ in 0..60 {
            frame.fill(0.5);
            g.process(-15.0, &mut frame);
        }
        assert!(frame.iter().all(|s| s.abs() < 1e-3));
        frame.fill(0.5);
        assert!(g.process(10.0, &mut frame));
        assert!(frame[479] > 0.45, "attack too slow: {}", frame[479]);
    }
}
