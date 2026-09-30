//! Cheap time-domain helpers that run before/after the neural denoiser.

use std::f32::consts::PI;

/// Second-order Butterworth high-pass (RBJ cookbook), transposed direct form II.
#[derive(Debug, Clone)]
pub struct HighPass {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl HighPass {
    pub fn new(sample_rate: f32, cutoff_hz: f32) -> Self {
        let w0 = 2.0 * PI * cutoff_hz / sample_rate;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * std::f32::consts::FRAC_1_SQRT_2);
        let a0 = 1.0 + alpha;
        Self {
            b0: ((1.0 + cos) / 2.0) / a0,
            b1: (-(1.0 + cos)) / a0,
            b2: ((1.0 + cos) / 2.0) / a0,
            a1: (-2.0 * cos) / a0,
            a2: (1.0 - alpha) / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    #[inline]
    pub fn tick(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }

    pub fn process(&mut self, buf: &mut [f32]) {
        for s in buf.iter_mut() {
            *s = self.tick(*s);
        }
    }

    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }
}

/// Soft-knee limiter that keeps |y| at or below [`SoftLimiter::CEILING`] without pumping.
#[derive(Debug, Clone, Copy)]
pub struct SoftLimiter {
    threshold: f32,
}

impl SoftLimiter {
    /// Absolute output ceiling (about -0.2 dBFS), so integer conversion can never clip.
    pub const CEILING: f32 = 0.98;

    pub fn new(threshold: f32) -> Self {
        Self {
            threshold: threshold.clamp(0.1, 0.95),
        }
    }

    #[inline]
    pub fn tick(&self, x: f32) -> f32 {
        let a = x.abs();
        if a <= self.threshold {
            x
        } else {
            let t = self.threshold;
            let room = Self::CEILING - t;
            (t + room * ((a - t) / room).tanh()).copysign(x)
        }
    }

    pub fn process(&self, buf: &mut [f32]) {
        for s in buf.iter_mut() {
            *s = self.tick(*s);
        }
    }
}

/// Click-free gain ramp.
#[derive(Debug, Clone, Copy)]
pub struct SmoothedGain {
    current: f32,
    target: f32,
    coef: f32,
}

impl SmoothedGain {
    pub fn new(sample_rate: f32, ramp_ms: f32, initial: f32) -> Self {
        Self {
            current: initial,
            target: initial,
            coef: one_pole_coef(sample_rate, ramp_ms),
        }
    }

    pub fn set_target(&mut self, target: f32) {
        self.target = target;
    }

    pub fn target(&self) -> f32 {
        self.target
    }

    pub fn process(&mut self, buf: &mut [f32]) {
        if (self.current - self.target).abs() < 1e-6 {
            if self.target != 1.0 {
                for s in buf.iter_mut() {
                    *s *= self.target;
                }
            }
            return;
        }
        for s in buf.iter_mut() {
            self.current += (self.target - self.current) * self.coef;
            *s *= self.current;
        }
    }
}

/// One-pole smoothing coefficient for a time constant in milliseconds.
#[inline]
pub fn one_pole_coef(sample_rate: f32, ms: f32) -> f32 {
    1.0 - (-1.0 / (sample_rate * ms.max(0.01) / 1000.0)).exp()
}

/// Peak and RMS of a buffer.
pub fn peak_rms(buf: &[f32]) -> (f32, f32) {
    let mut peak = 0.0f32;
    let mut energy = 0.0f32;
    for &s in buf {
        peak = peak.max(s.abs());
        energy += s * s;
    }
    let rms = if buf.is_empty() {
        0.0
    } else {
        (energy / buf.len() as f32).sqrt()
    };
    (peak, rms)
}

pub fn db_to_lin(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone_rms_after_filter(freq: f32, sr: f32) -> f32 {
        let mut f = HighPass::new(sr, 80.0);
        let n = sr as usize;
        let mut buf: Vec<f32> = (0..n)
            .map(|i| (2.0 * PI * freq * i as f32 / sr).sin())
            .collect();
        f.process(&mut buf);
        // skip the transient
        peak_rms(&buf[n / 2..]).1
    }

    #[test]
    fn high_pass_removes_rumble_and_keeps_voice() {
        let sr = 48_000.0;
        let low = tone_rms_after_filter(20.0, sr);
        let mid = tone_rms_after_filter(1000.0, sr);
        let ref_rms = std::f32::consts::FRAC_1_SQRT_2;
        assert!(20.0 * (low / ref_rms).log10() < -20.0, "20 Hz not attenuated: {low}");
        assert!((20.0 * (mid / ref_rms).log10()).abs() < 0.5, "1 kHz altered: {mid}");
    }

    #[test]
    fn limiter_never_exceeds_unity() {
        let l = SoftLimiter::new(0.85);
        for x in [-10.0f32, -1.5, -1.0, -0.5, 0.0, 0.5, 1.0, 1.5, 10.0] {
            let y = l.tick(x);
            assert!(y.abs() <= SoftLimiter::CEILING, "{x} -> {y}");
            if x.abs() <= 0.85 {
                assert_eq!(x, y);
            }
        }
    }

    #[test]
    fn smoothed_gain_converges() {
        let mut g = SmoothedGain::new(48_000.0, 5.0, 1.0);
        g.set_target(0.5);
        let mut buf = vec![1.0f32; 4800];
        g.process(&mut buf);
        assert!((buf[4799] - 0.5).abs() < 1e-3);
        assert!(buf[0] > 0.9);
    }
}
