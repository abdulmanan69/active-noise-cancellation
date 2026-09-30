//! Streaming sample-rate converter (band-limited sinc, real-time safe).

use anyhow::{Result, anyhow};
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{
    Async, FixedAsync, Resampler, SincInterpolationParameters, SincInterpolationType,
    WindowFunction,
};

const CHUNK: usize = 256;

pub struct StreamResampler {
    inner: Async<f32>,
    rate_in: u32,
    rate_out: u32,
    pending: Vec<f32>,
    out: Vec<f32>,
}

impl StreamResampler {
    pub fn new(rate_in: u32, rate_out: u32) -> Result<Self> {
        anyhow::ensure!(rate_in > 0 && rate_out > 0, "invalid sample rate");
        let params = SincInterpolationParameters {
            sinc_len: 128,
            f_cutoff: Some(0.95),
            interpolation: SincInterpolationType::Linear,
            oversampling_factor: 256,
            window: WindowFunction::BlackmanHarris2,
        };
        let ratio = rate_out as f64 / rate_in as f64;
        let inner = Async::<f32>::new_sinc(ratio, 1.1, &params, CHUNK, 1, FixedAsync::Input)
            .map_err(|e| anyhow!("resampler {rate_in}->{rate_out}: {e}"))?;
        let out_max = inner.output_frames_max();
        Ok(Self {
            inner,
            rate_in,
            rate_out,
            pending: Vec::with_capacity(CHUNK * 8),
            out: vec![0.0; out_max],
        })
    }

    pub fn rates(&self) -> (u32, u32) {
        (self.rate_in, self.rate_out)
    }

    /// Group delay of the converter in output frames.
    pub fn delay_frames(&self) -> usize {
        self.inner.output_delay()
    }

    /// Feed input samples; converted samples are appended to `dst`.
    pub fn push(&mut self, input: &[f32], dst: &mut Vec<f32>) -> Result<()> {
        self.pending.extend_from_slice(input);
        let mut consumed = 0;
        loop {
            let need = self.inner.input_frames_next();
            if self.pending.len() - consumed < need {
                break;
            }
            let src = InterleavedSlice::new(&self.pending[consumed..consumed + need], 1, need)
                .map_err(|e| anyhow!("resampler input: {e:?}"))?;
            let cap = self.out.len();
            let mut out = InterleavedSlice::new_mut(&mut self.out[..], 1, cap)
                .map_err(|e| anyhow!("resampler output: {e:?}"))?;
            let (used, produced) = self
                .inner
                .process_into_buffer(&src, &mut out, None)
                .map_err(|e| anyhow!("resample: {e}"))?;
            consumed += used;
            dst.extend_from_slice(&self.out[..produced]);
        }
        self.pending.drain(..consumed);
        Ok(())
    }

    /// Push silence so everything buffered inside the converter comes out.
    pub fn flush(&mut self, dst: &mut Vec<f32>) -> Result<()> {
        let zeros = vec![0.0f32; CHUNK * 2 + self.inner.output_delay() * 2];
        self.push(&zeros, dst)
    }

    pub fn reset(&mut self) {
        self.inner.reset();
        self.pending.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_44k1_tone_to_48k() {
        let mut rs = StreamResampler::new(44_100, 48_000).unwrap();
        let n = 44_100;
        let input: Vec<f32> = (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 44_100.0).sin())
            .collect();
        let mut out = Vec::new();
        rs.push(&input, &mut out).unwrap();
        rs.flush(&mut out).unwrap();
        assert!(out.len() >= 48_000, "got {} samples", out.len());
        // amplitude preserved in the steady-state region
        let mid = &out[10_000..40_000];
        let peak = mid.iter().fold(0.0f32, |a, &b| a.max(b.abs()));
        assert!((peak - 1.0).abs() < 0.05, "peak {peak}");
    }
}
