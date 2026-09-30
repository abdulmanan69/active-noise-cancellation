//! Real-time engine: microphone -> DSP pipeline -> virtual microphone (+ optional monitor).
//!
//! Layout (one dedicated worker thread, lock-free rings to the device callbacks):
//!
//! ```text
//! mic callback --> in_ring --> worker: resample->48k, 10 ms frames, DSP --> out_ring --> cable callback
//!                                                                     \--> mon_ring --> headphones
//! ```

use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, StreamTrait};
use ringbuf::HeapRb;
use ringbuf::traits::{Consumer, Observer, Producer, Split};

use crate::audio::{self, resampler::StreamResampler};
use crate::dsp::{FrameInfo, HOP_SIZE, Pipeline, ProcParams, SAMPLE_RATE};
use crate::stats::Stats;

#[derive(Debug, Clone)]
pub struct EngineParams {
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub monitor_device: Option<String>,
    pub proc: ProcParams,
    /// Output pre-buffer in ms; larger is safer on slow machines.
    pub buffer_ms: u32,
    /// Run everything but send silence to the outputs (used by the self-test).
    pub mute_output: bool,
}

#[derive(Debug)]
enum Command {
    Update(ProcParams),
    Stop,
}

#[derive(Debug, Clone)]
pub struct StartInfo {
    pub input_name: String,
    pub output_name: String,
    pub output_is_virtual_cable: bool,
    pub monitor_name: Option<String>,
    pub input_rate: u32,
    pub output_rate: u32,
    pub model_load_ms: u32,
    pub algorithmic_latency_ms: f32,
    pub realtime_priority: bool,
}

#[derive(Debug, Clone)]
pub enum Event {
    Starting,
    Started(StartInfo),
    Error(String),
    Stopped,
}

pub struct Engine {
    cmd_tx: Sender<Command>,
    events: Receiver<Event>,
    stats: Arc<Stats>,
    thread: Option<JoinHandle<()>>,
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

impl Engine {
    /// Spawn the audio thread. Progress is reported through [`Engine::poll_event`].
    /// `Event::Stopped` is always the last event, even if the worker fails or panics.
    pub fn start(params: EngineParams) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (ev_tx, ev_rx) = mpsc::channel();
        let stats = Arc::new(Stats::default());
        let stats_worker = stats.clone();
        let thread = thread::Builder::new()
            .name("clearmic-audio".into())
            .spawn(move || {
                let _ = ev_tx.send(Event::Starting);
                let tx = ev_tx.clone();
                let result = panic::catch_unwind(AssertUnwindSafe(move || {
                    run_worker(params, cmd_rx, tx, stats_worker)
                }));
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => {
                        log::error!("engine stopped: {e:#}");
                        let _ = ev_tx.send(Event::Error(format!("{e:#}")));
                    }
                    Err(payload) => {
                        let msg = panic_message(payload.as_ref());
                        log::error!("audio thread panicked: {msg}");
                        let _ = ev_tx.send(Event::Error(format!("internal error: {msg}")));
                    }
                }
                let _ = ev_tx.send(Event::Stopped);
            })
            .expect("spawn audio thread");
        Self {
            cmd_tx,
            events: ev_rx,
            stats,
            thread: Some(thread),
        }
    }

    pub fn stats(&self) -> &Arc<Stats> {
        &self.stats
    }

    pub fn update(&self, params: ProcParams) {
        let _ = self.cmd_tx.send(Command::Update(params));
    }

    pub fn poll_event(&self) -> Option<Event> {
        self.events.try_recv().ok()
    }

    /// Block until the next event or timeout.
    pub fn wait_event(&self, timeout: Duration) -> Option<Event> {
        self.events.recv_timeout(timeout).ok()
    }

    pub fn is_alive(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }

    fn request_stop(&self) {
        let _ = self.cmd_tx.send(Command::Stop);
        if let Some(t) = &self.thread {
            t.thread().unpark();
        }
    }

    /// Stop the engine and wait up to `timeout` for the audio thread to release its devices.
    /// Returns true if the thread finished in time.
    pub fn stop(mut self, timeout: Duration) -> bool {
        self.request_stop();
        let Some(handle) = self.thread.take() else {
            return true;
        };
        let deadline = Instant::now() + timeout;
        while !handle.is_finished() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        if handle.is_finished() {
            let _ = handle.join();
            true
        } else {
            false
        }
    }
}

impl Drop for Engine {
    /// Non-blocking: the worker is told to stop and finishes on its own, so a slow
    /// audio driver can never freeze the caller (usually the UI thread).
    fn drop(&mut self) {
        self.request_stop();
    }
}

struct Ports {
    input: cpal::Device,
    input_name: String,
    output: cpal::Device,
    output_name: String,
    output_is_virtual_cable: bool,
    monitor: Option<(cpal::Device, String)>,
}

fn resolve_ports(params: &EngineParams) -> Result<Ports> {
    let input = audio::find_input(params.input_device.as_deref())
        .context("no microphone available")?;
    let input_name = audio::device_name(&input);

    let (output, output_name, output_is_virtual_cable) = match params
        .output_device
        .as_deref()
        .and_then(audio::find_output_exact)
    {
        Some(d) => {
            let n = audio::device_name(&d);
            let v = audio::is_virtual_cable_name(&n);
            (d, n, v)
        }
        None => {
            if let Some(want) = &params.output_device {
                log::warn!("output '{want}' not found, auto-detecting virtual cable");
            }
            match audio::find_virtual_cable_output() {
                Some(d) => {
                    let n = audio::device_name(&d);
                    (d, n, true)
                }
                None => {
                    let d = audio::default_output().context("no output device available")?;
                    let n = audio::device_name(&d);
                    log::warn!("no virtual cable installed; sending audio to '{n}'");
                    (d, n, false)
                }
            }
        }
    };

    let monitor = params
        .monitor_device
        .as_deref()
        .and_then(audio::find_output_exact)
        .map(|d| {
            let n = audio::device_name(&d);
            (d, n)
        })
        .filter(|(_, n)| *n != output_name);

    Ok(Ports {
        input,
        input_name,
        output,
        output_name,
        output_is_virtual_cable,
        monitor,
    })
}

/// Fill-level thresholds (in samples of the destination ring) used for clock-drift control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DriftLimits {
    /// Below this a quiet frame is repeated to rebuild the cushion.
    low: usize,
    /// Above this quiet frames are dropped.
    high: usize,
    /// Above this every frame is dropped (bounds latency even during continuous speech).
    hard: usize,
}

impl DriftLimits {
    fn new(rate: u32, prebuffer: usize, device_period: usize) -> Self {
        let ms = |m: usize| rate as usize * m / 1000;
        let target = prebuffer + device_period;
        Self {
            low: prebuffer / 2,
            high: target + ms(40),
            hard: target + ms(100),
        }
    }
}

/// Push one processed frame into a playback ring while keeping its fill level bounded.
///
/// The microphone and the playback device run on different clocks, so the ring slowly
/// fills or drains. Frames are dropped or repeated only while nobody is speaking
/// (`quiet`), which is inaudible; the hard limit applies regardless.
/// Returns false when the frame was dropped.
fn push_frame<P: Producer<Item = f32>>(
    prod: &mut P,
    data: &[f32],
    quiet: bool,
    lim: DriftLimits,
) -> bool {
    let occupied = prod.occupied_len();
    if occupied > lim.hard || (occupied > lim.high && quiet) || prod.vacant_len() < data.len() {
        return false;
    }
    prod.push_slice(data);
    if occupied < lim.low && quiet && prod.vacant_len() >= data.len() {
        prod.push_slice(data);
    }
    true
}

/// Apply queued commands. Returns true when the engine should stop.
fn drain_commands(cmd_rx: &Receiver<Command>, latest: &mut Option<ProcParams>) -> bool {
    loop {
        match cmd_rx.try_recv() {
            Ok(Command::Update(p)) => *latest = Some(p),
            Ok(Command::Stop) | Err(TryRecvError::Disconnected) => return true,
            Err(TryRecvError::Empty) => return false,
        }
    }
}

/// Build a playback stream fed from `cons` with pre-buffering and underrun recovery.
fn playback_stream<C>(
    device: &cpal::Device,
    cfg: &cpal::SupportedStreamConfig,
    mut cons: C,
    prebuffer: usize,
    period: Arc<AtomicUsize>,
    underruns: Option<Arc<Stats>>,
    on_error: impl FnMut(cpal::Error) + Send + 'static,
) -> Result<cpal::Stream>
where
    C: Consumer<Item = f32> + Send + 'static,
{
    let mut primed = false;
    audio::build_output_stream(
        device,
        cfg,
        move |buf| {
            period.fetch_max(buf.len(), Ordering::Relaxed);
            if !primed {
                if cons.occupied_len() >= prebuffer + buf.len() {
                    primed = true;
                } else {
                    buf.fill(0.0);
                    return;
                }
            }
            let n = cons.pop_slice(buf);
            if n < buf.len() {
                buf[n..].fill(0.0);
                primed = false;
                if let Some(stats) = &underruns {
                    stats.output_underruns.fetch_add(1, Ordering::Relaxed);
                }
            }
        },
        on_error,
    )
}

fn run_worker(
    params: EngineParams,
    cmd_rx: Receiver<Command>,
    ev_tx: Sender<Event>,
    stats: Arc<Stats>,
) -> Result<()> {
    let realtime_priority = crate::platform::promote_audio_thread();
    let ports = resolve_ports(&params)?;

    let in_cfg = ports
        .input
        .default_input_config()
        .with_context(|| format!("query format of '{}'", ports.input_name))?;
    let out_cfg = ports
        .output
        .default_output_config()
        .with_context(|| format!("query format of '{}'", ports.output_name))?;
    let mon_cfg = match &ports.monitor {
        Some((d, n)) => Some(
            d.default_output_config()
                .with_context(|| format!("query format of '{n}'"))?,
        ),
        None => None,
    };
    let in_rate = in_cfg.sample_rate();
    let out_rate = out_cfg.sample_rate();
    let mon_rate = mon_cfg.as_ref().map(|c| c.sample_rate());
    log::info!(
        "mic '{}' {} Hz x{} {:?} -> '{}' {} Hz x{} {:?}",
        ports.input_name,
        in_rate,
        in_cfg.channels(),
        in_cfg.sample_format(),
        ports.output_name,
        out_rate,
        out_cfg.channels(),
        out_cfg.sample_format()
    );

    // --- model -------------------------------------------------------------
    let t0 = Instant::now();
    let mut pipeline = Pipeline::new(params.proc).context("load noise model")?;
    let model_load_ms = t0.elapsed().as_millis() as u32;
    log::info!("model ready in {model_load_ms} ms");

    // A newer engine may already have replaced this one while the model was loading.
    let mut pending: Option<ProcParams> = None;
    if drain_commands(&cmd_rx, &mut pending) {
        return Ok(());
    }
    if let Some(p) = pending.take() {
        pipeline.set_params(p);
    }

    // --- converters ----------------------------------------------------------
    let model_rate = SAMPLE_RATE as u32;
    let mut in_rs = (in_rate != model_rate)
        .then(|| StreamResampler::new(in_rate, model_rate))
        .transpose()?;
    let mut out_rs = (out_rate != model_rate)
        .then(|| StreamResampler::new(model_rate, out_rate))
        .transpose()?;
    let mut mon_rs = match mon_rate {
        Some(r) if r != model_rate => Some(StreamResampler::new(model_rate, r)?),
        _ => None,
    };

    // --- rings (1 s capacity each) -------------------------------------------
    let (mut in_prod, mut in_cons) = HeapRb::<f32>::new(in_rate as usize).split();
    let (mut out_prod, out_cons) = HeapRb::<f32>::new(out_rate as usize).split();
    let (mut mon_prod, mon_cons) = match mon_rate {
        Some(r) => {
            let (p, c) = HeapRb::<f32>::new(r as usize).split();
            (Some(p), Some(c))
        }
        None => (None, None),
    };

    let buffer_ms = params.buffer_ms.clamp(10, 200) as usize;
    let prebuffer = out_rate as usize * buffer_ms / 1000;
    let mon_prebuffer = mon_rate.unwrap_or(0) as usize * buffer_ms / 1000;
    let max_in_backlog = in_rate as usize / 5; // 200 ms
    let mute_output = params.mute_output;
    let out_period = Arc::new(AtomicUsize::new(0));
    let mon_period = Arc::new(AtomicUsize::new(0));

    let failed = Arc::new(AtomicBool::new(false));
    let worker = thread::current();

    let err_cb = |what: &'static str| {
        let failed = failed.clone();
        let ev_tx = ev_tx.clone();
        let worker = worker.clone();
        move |e: cpal::Error| match e.kind() {
            // Informational: the stream keeps running.
            cpal::ErrorKind::Xrun => log::debug!("{what}: {e}"),
            cpal::ErrorKind::RealtimeDenied | cpal::ErrorKind::DeviceChanged => {
                log::warn!("{what}: {e}")
            }
            _ => {
                log::error!("{what} stream error: {e}");
                failed.store(true, Ordering::SeqCst);
                let _ = ev_tx.send(Event::Error(format!("{what}: {e}")));
                worker.unpark();
            }
        }
    };

    // --- streams -------------------------------------------------------------
    let in_stream = {
        let stats = stats.clone();
        let worker = worker.clone();
        audio::build_input_stream(
            &ports.input,
            &in_cfg,
            move |mono| {
                let pushed = in_prod.push_slice(mono);
                if pushed < mono.len() {
                    stats.input_overruns.fetch_add(1, Ordering::Relaxed);
                }
                worker.unpark();
            },
            err_cb("microphone"),
        )
        .with_context(|| format!("open microphone '{}'", ports.input_name))?
    };

    let out_stream = playback_stream(
        &ports.output,
        &out_cfg,
        out_cons,
        prebuffer,
        out_period.clone(),
        Some(stats.clone()),
        err_cb("output"),
    )
    .with_context(|| format!("open output '{}'", ports.output_name))?;

    let mon_stream = match (&ports.monitor, mon_cfg.as_ref(), mon_cons) {
        (Some((dev, name)), Some(cfg), Some(cons)) => Some(
            playback_stream(
                dev,
                cfg,
                cons,
                mon_prebuffer,
                mon_period.clone(),
                None,
                err_cb("monitor"),
            )
            .with_context(|| format!("open monitor '{name}'"))?,
        ),
        _ => None,
    };

    out_stream.play().context("start output")?;
    if let Some(m) = &mon_stream {
        m.play().context("start monitor")?;
    }
    in_stream.play().context("start microphone")?;

    stats.reset_counters();
    stats.input_rate.store(in_rate, Ordering::Relaxed);
    stats.output_rate.store(out_rate, Ordering::Relaxed);
    let algo_ms = pipeline.algorithmic_latency_ms();
    let _ = ev_tx.send(Event::Started(StartInfo {
        input_name: ports.input_name.clone(),
        output_name: ports.output_name.clone(),
        output_is_virtual_cable: ports.output_is_virtual_cable,
        monitor_name: ports.monitor.as_ref().map(|(_, n)| n.clone()),
        input_rate: in_rate,
        output_rate: out_rate,
        model_load_ms,
        algorithmic_latency_ms: algo_ms,
        realtime_priority,
    }));

    // --- main loop -----------------------------------------------------------
    let mut raw = vec![0f32; 8192];
    let mut fifo: Vec<f32> = Vec::with_capacity(SAMPLE_RATE);
    let mut frame = [0f32; HOP_SIZE];
    let mut out_scratch: Vec<f32> = Vec::with_capacity(HOP_SIZE * 2);
    let mut mon_scratch: Vec<f32> = Vec::with_capacity(HOP_SIZE * 2);
    let mut proc_avg = 0f32;
    let mut proc_max = 0f32;
    let mut cpu = 0f32;
    let frame_us = HOP_SIZE as f32 * 1e6 / SAMPLE_RATE as f32;
    let mut last_stats = Instant::now();

    loop {
        if drain_commands(&cmd_rx, &mut pending) || failed.load(Ordering::SeqCst) {
            break;
        }
        if let Some(p) = pending.take() {
            pipeline.set_params(p);
        }

        // Overload guard: never let more than 200 ms of microphone audio queue up.
        let backlog = in_cons.occupied_len();
        if backlog > max_in_backlog {
            let skipped = in_cons.skip(backlog - max_in_backlog);
            let frames = skipped as u64 * SAMPLE_RATE as u64 / in_rate as u64 / HOP_SIZE as u64;
            stats
                .dropped_frames
                .fetch_add(frames.max(1), Ordering::Relaxed);
        }

        let n = in_cons.pop_slice(&mut raw);
        if n == 0 {
            thread::park_timeout(Duration::from_millis(10));
            continue;
        }

        match in_rs.as_mut() {
            Some(rs) => rs.push(&raw[..n], &mut fifo)?,
            None => fifo.extend_from_slice(&raw[..n]),
        }

        let out_limits = DriftLimits::new(out_rate, prebuffer, out_period.load(Ordering::Relaxed));
        let mon_limits = DriftLimits::new(
            mon_rate.unwrap_or(model_rate),
            mon_prebuffer,
            mon_period.load(Ordering::Relaxed),
        );

        let mut consumed = 0;
        while fifo.len() - consumed >= HOP_SIZE {
            frame.copy_from_slice(&fifo[consumed..consumed + HOP_SIZE]);
            consumed += HOP_SIZE;

            let t = Instant::now();
            let info = match pipeline.process_frame(&mut frame) {
                Ok(i) => i,
                Err(e) => {
                    log::error!("dsp error, passing audio through: {e:#}");
                    FrameInfo::default()
                }
            };
            let dt_us = t.elapsed().as_secs_f32() * 1e6;
            proc_avg += (dt_us - proc_avg) * 0.02;
            proc_max = proc_max.max(dt_us);
            cpu += (dt_us / frame_us * 100.0 - cpu) * 0.02;

            stats.set_levels(info.input_peak, info.input_rms, info.output_peak, info.output_rms);
            stats.set_lsnr(info.lsnr_db);
            stats.voice_active.store(info.voice, Ordering::Relaxed);
            stats.gate_open.store(info.gate_open, Ordering::Relaxed);
            stats.processing.store(info.processed, Ordering::Relaxed);
            stats.frames.fetch_add(1, Ordering::Relaxed);

            if mute_output {
                frame.fill(0.0);
            }

            // "Quiet" frames are safe places for drift correction.
            let quiet = if info.processed {
                !info.voice
            } else {
                info.output_rms < 0.006
            };

            let out: &[f32] = match out_rs.as_mut() {
                Some(rs) => {
                    out_scratch.clear();
                    rs.push(&frame, &mut out_scratch)?;
                    &out_scratch
                }
                None => &frame,
            };
            if !push_frame(&mut out_prod, out, quiet, out_limits) {
                stats.dropped_frames.fetch_add(1, Ordering::Relaxed);
            }

            if let Some(mp) = mon_prod.as_mut() {
                let m: &[f32] = match mon_rs.as_mut() {
                    Some(rs) => {
                        mon_scratch.clear();
                        rs.push(&frame, &mut mon_scratch)?;
                        &mon_scratch
                    }
                    None => &frame,
                };
                push_frame(mp, m, quiet, mon_limits);
            }

            if last_stats.elapsed() >= Duration::from_millis(100) {
                last_stats = Instant::now();
                stats.set_timing(dt_us, proc_avg, proc_max, cpu);
                let latency = in_cons.occupied_len() as f32 * 1000.0 / in_rate as f32
                    + (fifo.len() - consumed) as f32 * 1000.0 / SAMPLE_RATE as f32
                    + algo_ms
                    + out_prod.occupied_len() as f32 * 1000.0 / out_rate as f32;
                stats.set_latency_ms(latency);
            }
        }
        fifo.drain(..consumed);
    }

    drop(in_stream);
    drop(out_stream);
    drop(mon_stream);
    log::info!("engine stopped");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> DriftLimits {
        // 48 kHz, 30 ms prebuffer, 10 ms device period
        DriftLimits::new(48_000, 1440, 480)
    }

    #[test]
    fn drift_limits_scale_with_rate_and_period() {
        let l = limits();
        assert_eq!(l.low, 720);
        assert_eq!(l.high, 1440 + 480 + 1920);
        assert_eq!(l.hard, 1440 + 480 + 4800);
        // a large device period must still leave room to prime during silence
        let big = DriftLimits::new(48_000, 1440, 4800);
        assert!(big.high > 1440 + 4800);
    }

    #[test]
    fn quiet_frames_are_dropped_above_high_and_voice_above_hard() {
        let (mut prod, mut cons) = HeapRb::<f32>::new(48_000).split();
        let frame = [0.1f32; 480];
        let l = limits();
        // fill to just above `high`
        while prod.occupied_len() <= l.high {
            assert!(push_frame(&mut prod, &frame, false, l));
        }
        let before = prod.occupied_len();
        assert!(!push_frame(&mut prod, &frame, true, l), "quiet frame must be dropped");
        assert_eq!(prod.occupied_len(), before);
        assert!(push_frame(&mut prod, &frame, false, l), "voice is kept below hard");
        while prod.occupied_len() <= l.hard {
            push_frame(&mut prod, &frame, false, l);
        }
        assert!(!push_frame(&mut prod, &frame, false, l), "hard limit applies to voice");
        assert!(cons.occupied_len() <= l.hard + 480);
        cons.skip(480);
    }

    #[test]
    fn quiet_frames_are_repeated_when_the_ring_runs_low() {
        let (mut prod, _cons) = HeapRb::<f32>::new(48_000).split();
        let frame = [0.0f32; 480];
        let l = limits();
        assert!(push_frame(&mut prod, &frame, true, l));
        assert_eq!(prod.occupied_len(), 960, "quiet frame repeated below low");
        let (mut prod, _cons) = HeapRb::<f32>::new(48_000).split();
        assert!(push_frame(&mut prod, &frame, false, l));
        assert_eq!(prod.occupied_len(), 480, "voice frame never repeated");
    }

    #[test]
    fn worker_failure_always_ends_with_stopped() {
        // Nonexistent devices are fine: whatever happens, Stopped must be the last event.
        let engine = Engine::start(EngineParams {
            input_device: Some("__no_such_microphone__".into()),
            output_device: Some("__no_such_output__".into()),
            monitor_device: None,
            proc: ProcParams::default(),
            buffer_ms: 30,
            mute_output: false,
        });
        engine.request_stop();
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut saw_stopped = false;
        while Instant::now() < deadline {
            match engine.wait_event(Duration::from_millis(200)) {
                Some(Event::Stopped) => {
                    saw_stopped = true;
                    break;
                }
                _ => continue,
            }
        }
        assert!(saw_stopped);
    }
}
