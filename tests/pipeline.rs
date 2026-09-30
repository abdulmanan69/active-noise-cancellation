//! End-to-end checks of the DSP pipeline against synthetic signals.

use clearmic::dsp::{HOP_SIZE, Pipeline, ProcParams, SAMPLE_RATE};

struct Rng(u32);
impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0 as f32 / u32::MAX as f32 * 2.0 - 1.0
    }
}

fn white_noise(seconds: f32, amp: f32, seed: u32) -> Vec<f32> {
    let mut r = Rng(seed);
    (0..(seconds * SAMPLE_RATE as f32) as usize)
        .map(|_| r.next() * amp)
        .collect()
}

/// Harmonic complex with vibrato and syllable-like amplitude modulation.
fn voice_like(seconds: f32, amp: f32) -> Vec<f32> {
    let sr = SAMPLE_RATE as f32;
    let n = (seconds * sr) as usize;
    let mut phase = 0.0f32;
    (0..n)
        .map(|i| {
            let t = i as f32 / sr;
            let f0 = 140.0 + 8.0 * (2.0 * std::f32::consts::PI * 5.0 * t).sin();
            phase += 2.0 * std::f32::consts::PI * f0 / sr;
            let mut s = 0.0;
            for h in 1..=12 {
                let w = 1.0 / h as f32;
                s += w * (phase * h as f32).sin();
            }
            let env = (0.5 + 0.5 * (2.0 * std::f32::consts::PI * 3.0 * t).sin()).powf(1.5);
            s * env * amp * 0.25
        })
        .collect()
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

fn db(x: f32) -> f32 {
    20.0 * x.max(1e-9).log10()
}

fn run(pipeline: &mut Pipeline, input: &[f32]) -> Vec<f32> {
    let mut out = input.to_vec();
    for frame in out.chunks_exact_mut(HOP_SIZE) {
        pipeline.process_frame(frame).unwrap();
    }
    out
}

#[test]
fn stationary_noise_is_strongly_suppressed() {
    let input = white_noise(3.0, 0.03, 1);
    let mut p = Pipeline::new(ProcParams {
        strength: 100,
        ..Default::default()
    })
    .unwrap();
    let out = run(&mut p, &input);
    let skip = SAMPLE_RATE / 2;
    let reduction = db(rms(&input[skip..])) - db(rms(&out[skip..]));
    assert!(reduction > 15.0, "only {reduction:.1} dB of noise reduction");
}

#[test]
fn strength_limits_the_attenuation() {
    let input = white_noise(3.0, 0.03, 2);
    let mut p = Pipeline::new(ProcParams {
        strength: 20, // ~10 dB limit
        ..Default::default()
    })
    .unwrap();
    let out = run(&mut p, &input);
    let skip = SAMPLE_RATE / 2;
    let reduction = db(rms(&input[skip..])) - db(rms(&out[skip..]));
    assert!(
        reduction > 3.0 && reduction < 12.0,
        "reduction {reduction:.1} dB outside the limit window"
    );
}

#[test]
fn bypass_is_bit_transparent() {
    let input = voice_like(1.0, 0.5);
    let mut p = Pipeline::new(ProcParams {
        enabled: false,
        high_pass: false,
        ..Default::default()
    })
    .unwrap();
    let out = run(&mut p, &input);
    assert_eq!(input, out);
}

#[test]
fn silence_in_silence_out() {
    let input = vec![0.0f32; SAMPLE_RATE];
    let mut p = Pipeline::new(ProcParams::default()).unwrap();
    let out = run(&mut p, &input);
    assert!(out.iter().all(|s| s.abs() < 1e-6));
}

#[test]
fn voice_survives_noise_and_delay_matches_declared() {
    let clean = voice_like(3.0, 0.6);
    let noise = white_noise(3.0, 0.02, 3);
    let noisy: Vec<f32> = clean.iter().zip(&noise).map(|(a, b)| a + b).collect();
    let mut p = Pipeline::new(ProcParams {
        strength: 100,
        high_pass: false,
        ..Default::default()
    })
    .unwrap();
    let declared = p.delay_samples();
    let out = run(&mut p, &noisy);

    // find the lag that maximises correlation between clean input and output
    let start = SAMPLE_RATE; // skip warm-up
    let len = SAMPLE_RATE;
    let mut best = (0usize, f32::MIN);
    for lag in 0..(HOP_SIZE * 8) {
        let c: f32 = (0..len)
            .map(|i| clean[start + i] * out[start + i + lag])
            .sum();
        if c > best.1 {
            best = (lag, c);
        }
    }
    assert!(
        best.0.abs_diff(declared) <= 2,
        "measured delay {} samples, declared {}",
        best.0,
        declared
    );

    // the voice should still be there: output level within 6 dB of the clean signal
    let clean_db = db(rms(&clean[start..start + len]));
    let out_db = db(rms(&out[start + best.0..start + len + best.0]));
    assert!(
        (clean_db - out_db).abs() < 6.0,
        "voice level changed by {:.1} dB",
        clean_db - out_db
    );
}

/// Scale-invariant signal-to-distortion ratio in dB.
fn si_sdr(reference: &[f32], estimate: &[f32]) -> f32 {
    let n = reference.len().min(estimate.len());
    let (r, e) = (&reference[..n], &estimate[..n]);
    let dot: f64 = r.iter().zip(e).map(|(a, b)| *a as f64 * *b as f64).sum();
    let pr: f64 = r.iter().map(|a| (*a as f64).powi(2)).sum::<f64>() + 1e-12;
    let g = dot / pr;
    let sig: f64 = r.iter().map(|a| (g * *a as f64).powi(2)).sum();
    let err: f64 = r
        .iter()
        .zip(e)
        .map(|(a, b)| (*b as f64 - g * *a as f64).powi(2))
        .sum::<f64>()
        + 1e-12;
    (10.0 * (sig / err).log10()) as f32
}

/// Short spoken sentence (Windows text-to-speech, 48 kHz mono) used as real speech material.
fn load_speech() -> Vec<f32> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/speech.wav");
    let mut reader = hound::WavReader::open(path).expect("tests/data/speech.wav is missing");
    let spec = reader.spec();
    assert_eq!(spec.sample_rate as usize, SAMPLE_RATE);
    assert_eq!(spec.channels, 1);
    reader
        .samples::<i16>()
        .map(|s| s.unwrap() as f32 / 32768.0 * 0.5)
        .collect()
}

/// Mix speech with white noise at `snr_db`, denoise, and return (input, output) SI-SDR.
fn denoise_speech(clean: &[f32], snr_db: f32) -> (f32, f32) {
    let noise = white_noise(clean.len() as f32 / SAMPLE_RATE as f32 + 0.1, 1.0, 7);
    let gain =
        (rms(clean).powi(2) / (rms(&noise).powi(2) * 10f32.powf(snr_db / 10.0))).sqrt();
    let mut noisy: Vec<f32> = clean.iter().zip(&noise).map(|(s, n)| s + gain * n).collect();
    let mut p = Pipeline::new(ProcParams {
        strength: 100,
        high_pass: false,
        ..Default::default()
    })
    .unwrap();
    let delay = p.delay_samples();
    // pad so the tail passes through the model and whole frames are processed
    let padded = (noisy.len() + delay).div_ceil(HOP_SIZE) * HOP_SIZE;
    noisy.resize(padded, 0.0);
    let out = run(&mut p, &noisy);
    let n = clean.len();
    (
        si_sdr(clean, &noisy[..n]),
        si_sdr(clean, &out[delay..delay + n]),
    )
}

/// Regression guard for the model stage thresholds (see `dsp::denoiser`).
///
/// With the library's default thresholds the deep-filter stage was skipped on some frames
/// and speech below 4.8 kHz came out garbled: a 20 dB input dropped to about 4 dB.
/// Denoising must improve real speech at every noise level, and never damage it.
#[test]
fn real_speech_is_improved_not_damaged() {
    let clean = load_speech();
    for (snr_db, min_gain_db) in [(0.0f32, 6.0f32), (10.0, 4.0), (20.0, 1.0)] {
        let (input, output) = denoise_speech(&clean, snr_db);
        println!("input {input:5.1} dB -> output {output:5.1} dB");
        assert!(
            output > input + min_gain_db,
            "at {snr_db} dB SNR: SI-SDR {input:.1} dB -> {output:.1} dB (needs +{min_gain_db} dB)"
        );
    }
}

/// The volume boost is applied after the model and can never clip the output.
#[test]
fn volume_boost_is_applied_after_the_model_and_stays_below_full_scale() {
    let clean = load_speech(); // peaks around 0.5
    let mut noisy = clean.clone();
    noisy.resize(noisy.len().div_ceil(HOP_SIZE) * HOP_SIZE, 0.0);
    let mut plain = Pipeline::new(ProcParams {
        strength: 100,
        ..Default::default()
    })
    .unwrap();
    let mut boosted = Pipeline::new(ProcParams {
        strength: 100,
        input_gain_db: 12.0,
        ..Default::default()
    })
    .unwrap();
    let a = run(&mut plain, &noisy);
    let b = run(&mut boosted, &noisy);
    // Below the limiter knee the boosted output is exactly the plain output times the gain,
    // which is only true when the model saw the same (unboosted) input in both runs.
    let gain = 10f32.powf(12.0 / 20.0);
    let mut compared = 0usize;
    for (x, y) in a.iter().zip(&b) {
        if (x * gain).abs() < 0.85 {
            assert!((x * gain - y).abs() < 1e-3, "boost changed the model input: {x} -> {y}");
            compared += 1;
        }
    }
    assert!(compared > SAMPLE_RATE, "too few samples compared");
    // the output is louder but never reaches full scale
    let peak_b = b.iter().fold(0f32, |m, s| m.max(s.abs()));
    assert!(peak_b <= 0.98 + 1e-4, "output peak {peak_b}");
    assert!(rms(&b) > rms(&a) * 2.0, "boost had no effect");
}
