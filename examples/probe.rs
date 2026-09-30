//! Diagnostic: record from a capture device (name substring) and print its level.
//! Usage: cargo run --example probe -- "CABLE Output" 5 [out.wav]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use clearmic::audio;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let needle = args.first().map(|s| s.to_lowercase()).unwrap_or_default();
    let secs: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(5);
    let host = cpal::default_host();
    let device = host
        .input_devices()?
        .find(|d| audio::device_name(d).to_lowercase().contains(&needle))
        .ok_or_else(|| anyhow::anyhow!("no capture device matching '{needle}'"))?;
    let cfg = device.default_input_config()?;
    let rate = cfg.sample_rate();
    println!("recording '{}' at {} Hz for {secs} s", audio::device_name(&device), rate);
    let buf = Arc::new(Mutex::new(Vec::<f32>::new()));
    let sink = buf.clone();
    let stream = audio::build_input_stream(
        &device,
        &cfg,
        move |mono| sink.lock().unwrap().extend_from_slice(mono),
        |e| eprintln!("stream error: {e}"),
    )?;
    stream.play()?;
    std::thread::sleep(Duration::from_secs(secs));
    drop(stream);
    let samples = buf.lock().unwrap().clone();
    let db = |x: f32| 20.0 * x.max(1e-7).log10();
    for (i, chunk) in samples.chunks(rate as usize / 2).enumerate() {
        let peak = chunk.iter().fold(0f32, |a, s| a.max(s.abs()));
        let rms = (chunk.iter().map(|s| s * s).sum::<f32>() / chunk.len().max(1) as f32).sqrt();
        println!("{:4.1}s  rms {:6.1} dBFS  peak {:6.1} dBFS", i as f32 * 0.5, db(rms), db(peak));
    }
    let rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len().max(1) as f32).sqrt();
    println!("overall rms {:.1} dBFS over {} samples", db(rms), samples.len());
    if let Some(path) = args.get(2) {
        let spec = hound::WavSpec { channels: 1, sample_rate: rate, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut w = hound::WavWriter::create(path, spec)?;
        for s in &samples {
            w.write_sample((s.clamp(-1.0, 1.0) * 32767.0) as i16)?;
        }
        w.finalize()?;
        println!("wrote {path}");
    }
    Ok(())
}
