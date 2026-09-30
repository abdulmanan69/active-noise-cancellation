//! Command-line interface: device listing, offline file processing, benchmark, headless mode.

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clearmic::audio::resampler::StreamResampler;
use clearmic::config::Settings;
use clearmic::dsp::{HOP_SIZE, Pipeline, ProcParams, SAMPLE_RATE};
use clearmic::engine::{Engine, EngineParams, Event};
use clearmic::{APP_NAME, MODEL_NAME, VERSION, audio};

pub fn run(args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        None | Some("--help") | Some("-h") => {
            print_help();
            Ok(())
        }
        Some("--version") | Some("-V") => {
            println!("{APP_NAME} {VERSION} ({MODEL_NAME})");
            Ok(())
        }
        Some("--list-devices") => list_devices(),
        Some("--process") => process_file(&args[1..]),
        Some("--bench") => bench(args.get(1).map(String::as_str)),
        Some("--headless") => headless(),
        Some("--selftest") => selftest(args.get(1).map(String::as_str)),
        Some("--export-icon") => export_icon(args.get(1).map(String::as_str)),
        Some(other) => bail!("unknown option '{other}' (try --help)"),
    }
}

fn print_help() {
    println!(
        "{APP_NAME} {VERSION} - AI noise cancellation for any microphone

USAGE:
  clearmic                         start the desktop app
  clearmic --minimized             start hidden in the system tray
  clearmic --headless              run the engine without a window (uses saved settings)
  clearmic --list-devices          print capture/render endpoints
  clearmic --process IN.wav OUT.wav [--strength N] [--post-filter] [--gate] [--no-hpf]
                                   clean a recording offline (delay-compensated)
  clearmic --bench [SECONDS]       measure processing speed on this CPU
  clearmic --selftest [SECONDS]    open the real devices, run muted, report health (exit code 0 = ok)
  clearmic --export-icon [PATH]    write the application icon as .ico
  clearmic --version | --help
"
    );
}

fn print_device(d: &audio::DeviceInfo) {
    println!(
        "  {}{}  [{} Hz, {} ch]{}",
        if d.is_default { "* " } else { "  " },
        d.name,
        d.sample_rate,
        d.channels,
        if d.kind == audio::DeviceKind::VirtualCable {
            "  (virtual)"
        } else {
            ""
        }
    );
}

fn list_devices() -> Result<()> {
    let list = audio::enumerate();
    println!("Capture devices (microphones):");
    list.inputs.iter().for_each(print_device);
    println!("\nRender devices (outputs):");
    list.outputs.iter().for_each(print_device);
    println!();
    match list.virtual_cable_output() {
        Some(d) => println!("Virtual microphone: ready ('{}')", d.name),
        None => println!(
            "Virtual microphone: NOT installed - see {}",
            clearmic::VB_CABLE_URL
        ),
    }
    Ok(())
}

fn read_wav_mono(path: &Path) -> Result<(Vec<f32>, u32)> {
    let mut reader =
        hound::WavReader::open(path).with_context(|| format!("open {}", path.display()))?;
    let spec = reader.spec();
    let ch = spec.channels.max(1) as usize;
    anyhow::ensure!(
        (8..=32).contains(&spec.bits_per_sample),
        "unsupported bit depth {}",
        spec.bits_per_sample
    );
    let decode = || format!("decode {}", path.display());
    let samples: Vec<f32> = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Float, _) => reader
            .samples::<f32>()
            .collect::<Result<Vec<_>, _>>()
            .with_context(decode)?,
        (hound::SampleFormat::Int, bits) => {
            let scale = 1.0 / (1u64 << (bits - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 * scale))
                .collect::<Result<Vec<_>, _>>()
                .with_context(decode)?
        }
    };
    let mono: Vec<f32> = samples
        .chunks_exact(ch)
        .map(|f| f.iter().sum::<f32>() / ch as f32)
        .collect();
    Ok((mono, spec.sample_rate))
}

fn write_wav_mono(path: &Path, samples: &[f32], rate: u32) -> Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer =
        hound::WavWriter::create(path, spec).with_context(|| format!("create {}", path.display()))?;
    for &s in samples {
        writer.write_sample((s.clamp(-1.0, 1.0) * 32767.0).round() as i16)?;
    }
    writer.finalize()?;
    Ok(())
}

/// Offline processing with full delay compensation (output aligned to input).
pub fn process_buffer(input: &[f32], rate: u32, params: ProcParams) -> Result<(Vec<f32>, f32)> {
    let model_rate = SAMPLE_RATE as u32;
    let mut pipeline = Pipeline::new(params)?;

    // -> 48 kHz
    let mut in_rs = (rate != model_rate)
        .then(|| StreamResampler::new(rate, model_rate))
        .transpose()?;
    let mut work = Vec::with_capacity(input.len() * 2);
    let mut skip_48k = 0usize;
    match in_rs.as_mut() {
        Some(rs) => {
            rs.push(input, &mut work)?;
            rs.flush(&mut work)?;
            skip_48k += rs.delay_frames();
        }
        None => work.extend_from_slice(input),
    }
    if params.enabled && params.strength > 0 {
        skip_48k += pipeline.delay_samples();
    }
    let expected_48k = (input.len() as f64 * model_rate as f64 / rate as f64).round() as usize;
    let needed = skip_48k + expected_48k;
    if work.len() < needed {
        work.resize(needed, 0.0);
    }
    // pad so the tail is flushed through the model
    let pad = HOP_SIZE - (work.len() % HOP_SIZE);
    work.extend(std::iter::repeat_n(0.0, pad + skip_48k));

    let mut lsnr_sum = 0.0f32;
    let mut frames = 0usize;
    for frame in work.chunks_exact_mut(HOP_SIZE) {
        let info = pipeline.process_frame(frame)?;
        lsnr_sum += info.lsnr_db;
        frames += 1;
    }
    let cleaned = &work[skip_48k..skip_48k + expected_48k];

    // -> original rate
    let out = if rate != model_rate {
        let mut rs = StreamResampler::new(model_rate, rate)?;
        let mut buf = Vec::with_capacity(input.len() + 1024);
        rs.push(cleaned, &mut buf)?;
        rs.flush(&mut buf)?;
        let d = rs.delay_frames();
        if buf.len() < d + input.len() {
            buf.resize(d + input.len(), 0.0);
        }
        buf[d..d + input.len()].to_vec()
    } else {
        cleaned.to_vec()
    };
    let mean_lsnr = if frames > 0 {
        lsnr_sum / frames as f32
    } else {
        0.0
    };
    Ok((out, mean_lsnr))
}

fn process_file(args: &[String]) -> Result<()> {
    let (Some(input), Some(output)) = (args.first(), args.get(1)) else {
        bail!(
            "usage: clearmic --process IN.wav OUT.wav [--strength N] [--post-filter] [--gate] [--no-hpf]"
        );
    };
    let mut params = ProcParams::default();
    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--strength" => {
                params.strength = args
                    .get(i + 1)
                    .and_then(|v| v.parse::<u8>().ok())
                    .context("--strength needs a number 0-100")?
                    .min(100);
                i += 1;
            }
            "--post-filter" => params.post_filter = true,
            "--gate" => params.voice_gate = true,
            "--no-hpf" => params.high_pass = false,
            other => bail!("unknown option '{other}'"),
        }
        i += 1;
    }
    let (samples, rate) = read_wav_mono(Path::new(input))?;
    let secs = samples.len() as f32 / rate as f32;
    println!("{input}: {rate} Hz, {secs:.2} s, strength {}", params.strength);
    let t0 = Instant::now();
    let (cleaned, mean_lsnr) = process_buffer(&samples, rate, params)?;
    let dt = t0.elapsed().as_secs_f32();
    write_wav_mono(Path::new(output), &cleaned, rate)?;
    println!(
        "wrote {output}: processed in {dt:.2} s ({:.1}x real-time), mean local SNR {mean_lsnr:.1} dB",
        secs / dt.max(1e-6)
    );
    Ok(())
}

fn bench(seconds: Option<&str>) -> Result<()> {
    let secs: f32 = seconds.and_then(|s| s.parse().ok()).unwrap_or(10.0);
    let n = (secs * SAMPLE_RATE as f32) as usize / HOP_SIZE * HOP_SIZE;
    let mut x: u32 = 0x1234_5678;
    let mut buf: Vec<f32> = (0..n)
        .map(|i| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            let noise = (x as f32 / u32::MAX as f32 - 0.5) * 0.1;
            let tone = (i as f32 * 0.02).sin() * 0.2 * (i as f32 * 0.0005).sin().max(0.0);
            noise + tone
        })
        .collect();
    let t0 = Instant::now();
    let mut pipeline = Pipeline::new(ProcParams::default())?;
    let load = t0.elapsed();
    let mut max_us = 0f32;
    let t1 = Instant::now();
    for frame in buf.chunks_exact_mut(HOP_SIZE) {
        let t = Instant::now();
        pipeline.process_frame(frame)?;
        max_us = max_us.max(t.elapsed().as_secs_f32() * 1e6);
    }
    let total = t1.elapsed().as_secs_f32();
    let frames = (n / HOP_SIZE).max(1);
    let avg_us = total * 1e6 / frames as f32;
    println!("{APP_NAME} benchmark ({MODEL_NAME}, single thread)");
    println!("  model load       : {:.0} ms", load.as_secs_f32() * 1000.0);
    println!("  audio processed  : {secs:.1} s in {total:.2} s");
    println!("  per 10 ms frame  : avg {avg_us:.0} us, max {max_us:.0} us");
    println!("  CPU load (1 core): {:.1}%", avg_us / 10_000.0 * 100.0);
    println!("  real-time factor : {:.1}x", secs / total.max(1e-6));
    if avg_us > 8_000.0 {
        println!("  WARNING: this CPU is too slow for real-time processing");
    }
    Ok(())
}

fn headless() -> Result<()> {
    let settings = Settings::load();
    let engine = Engine::start(EngineParams {
        input_device: settings.input_device.clone(),
        output_device: settings.output_device.clone(),
        monitor_device: settings
            .monitor_enabled
            .then(|| settings.monitor_device.clone())
            .flatten(),
        proc: settings.proc_params(),
        buffer_ms: settings.buffer_ms,
        mute_output: false,
    });
    println!("{APP_NAME} headless - press Ctrl+C to stop");
    let mut last_print = Instant::now();
    loop {
        if let Some(ev) = engine.wait_event(Duration::from_millis(250)) {
            match ev {
                Event::Starting => println!("starting audio engine..."),
                Event::Started(info) => println!(
                    "running: '{}' ({} Hz) -> '{}' ({} Hz){}, model loaded in {} ms, latency ~{:.0} ms",
                    info.input_name,
                    info.input_rate,
                    info.output_name,
                    info.output_rate,
                    if info.output_is_virtual_cable {
                        " [virtual mic]"
                    } else {
                        " [WARNING: not a virtual cable]"
                    },
                    info.model_load_ms,
                    info.algorithmic_latency_ms
                ),
                Event::Error(e) => eprintln!("error: {e}"),
                Event::Stopped => bail!("audio engine stopped"),
            }
        }
        if last_print.elapsed() >= Duration::from_secs(2) {
            last_print = Instant::now();
            let s = engine.stats().snapshot();
            println!(
                "in {:>6.1} dB  out {:>6.1} dB  voice {}  snr {:>5.1} dB  cpu {:>4.1}%  latency {:>4.0} ms  xruns {}/{}",
                clearmic::stats::to_db(s.input_peak),
                clearmic::stats::to_db(s.output_peak),
                if s.voice_active { "yes" } else { "no " },
                s.lsnr_db,
                s.cpu_pct,
                s.latency_ms,
                s.input_overruns,
                s.output_underruns
            );
        }
    }
}

fn export_icon(path: Option<&str>) -> Result<()> {
    let path = path.unwrap_or("assets/clearmic.ico");
    if let Some(dir) = Path::new(path).parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, super::icon::ico_bytes())?;
    println!("wrote {path}");
    Ok(())
}

/// Open the configured devices and run the complete real-time path with the output muted.
/// Fails (non-zero exit code) when the engine cannot start, stops, or drops too much audio.
fn selftest(seconds: Option<&str>) -> Result<()> {
    let secs: u64 = seconds.and_then(|s| s.parse().ok()).unwrap_or(5).clamp(2, 120);
    let settings = Settings::load();
    let engine = Engine::start(EngineParams {
        input_device: settings.input_device.clone(),
        output_device: settings.output_device.clone(),
        monitor_device: None,
        proc: ProcParams {
            enabled: true,
            ..settings.proc_params()
        },
        buffer_ms: settings.buffer_ms,
        mute_output: true,
    });
    let deadline = Instant::now() + Duration::from_secs(60);
    let info = loop {
        match engine.wait_event(Duration::from_millis(250)) {
            Some(Event::Started(info)) => break info,
            Some(Event::Error(e)) => bail!("engine failed to start: {e}"),
            Some(Event::Stopped) => bail!("engine stopped before it started"),
            _ if Instant::now() > deadline => bail!("engine did not start within 60 s"),
            _ => {}
        }
    };
    println!("microphone : {} ({} Hz)", info.input_name, info.input_rate);
    println!(
        "output     : {} ({} Hz){}",
        info.output_name,
        info.output_rate,
        if info.output_is_virtual_cable { "" } else { "  [no virtual microphone installed]" }
    );
    println!("model load : {} ms, real-time priority: {}", info.model_load_ms, info.realtime_priority);

    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(secs) {
        match engine.wait_event(Duration::from_millis(200)) {
            Some(Event::Error(e)) => bail!("engine error: {e}"),
            Some(Event::Stopped) => bail!("engine stopped unexpectedly"),
            _ => {}
        }
    }
    let s = engine.stats().snapshot();
    let expected = secs * 100;
    println!("frames     : {} of ~{expected} (10 ms each)", s.frames);
    println!("cpu        : {:.1}% of one core, {:.2} ms avg / {:.2} ms peak per frame", s.cpu_pct, s.proc_avg_us / 1000.0, s.proc_max_us / 1000.0);
    println!("latency    : ~{:.0} ms end to end inside ClearMic", s.latency_ms);
    println!("dropouts   : capture {} / playback {} / frames dropped {}", s.input_overruns, s.output_underruns, s.dropped_frames);
    println!("mic level  : {:.1} dBFS peak", clearmic::stats::to_db(s.input_peak));
    let stopped = engine.stop(Duration::from_secs(3));
    if s.frames < expected * 8 / 10 {
        bail!("only {} of ~{expected} frames were processed", s.frames);
    }
    if !stopped {
        bail!("audio thread did not shut down in time");
    }
    println!("RESULT     : OK");
    Ok(())
}
