//! Lock-free runtime statistics shared between the audio thread and the UI.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

#[derive(Default)]
pub struct Stats {
    input_peak: AtomicU32,
    input_rms: AtomicU32,
    output_peak: AtomicU32,
    output_rms: AtomicU32,
    lsnr_db: AtomicU32,
    proc_us: AtomicU32,
    proc_avg_us: AtomicU32,
    proc_max_us: AtomicU32,
    cpu_pct: AtomicU32,
    latency_ms: AtomicU32,
    pub frames: AtomicU64,
    pub input_overruns: AtomicU64,
    pub output_underruns: AtomicU64,
    pub dropped_frames: AtomicU64,
    pub voice_active: AtomicBool,
    pub gate_open: AtomicBool,
    pub processing: AtomicBool,
    pub input_rate: AtomicU32,
    pub output_rate: AtomicU32,
}

/// Plain-old-data copy of [`Stats`] for rendering.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Snapshot {
    pub input_peak: f32,
    pub input_rms: f32,
    pub output_peak: f32,
    pub output_rms: f32,
    pub lsnr_db: f32,
    pub proc_us: f32,
    pub proc_avg_us: f32,
    pub proc_max_us: f32,
    pub cpu_pct: f32,
    pub latency_ms: f32,
    pub frames: u64,
    pub input_overruns: u64,
    pub output_underruns: u64,
    pub dropped_frames: u64,
    pub voice_active: bool,
    pub gate_open: bool,
    pub processing: bool,
    pub input_rate: u32,
    pub output_rate: u32,
}

#[inline]
fn set_f(a: &AtomicU32, v: f32) {
    a.store(v.to_bits(), Ordering::Relaxed);
}
#[inline]
fn get_f(a: &AtomicU32) -> f32 {
    f32::from_bits(a.load(Ordering::Relaxed))
}

impl Stats {
    pub fn set_levels(&self, in_peak: f32, in_rms: f32, out_peak: f32, out_rms: f32) {
        set_f(&self.input_peak, in_peak);
        set_f(&self.input_rms, in_rms);
        set_f(&self.output_peak, out_peak);
        set_f(&self.output_rms, out_rms);
    }
    pub fn set_lsnr(&self, v: f32) {
        set_f(&self.lsnr_db, v);
    }
    pub fn set_timing(&self, proc_us: f32, avg_us: f32, max_us: f32, cpu_pct: f32) {
        set_f(&self.proc_us, proc_us);
        set_f(&self.proc_avg_us, avg_us);
        set_f(&self.proc_max_us, max_us);
        set_f(&self.cpu_pct, cpu_pct);
    }
    pub fn set_latency_ms(&self, v: f32) {
        set_f(&self.latency_ms, v);
    }
    pub fn reset_counters(&self) {
        self.frames.store(0, Ordering::Relaxed);
        self.input_overruns.store(0, Ordering::Relaxed);
        self.output_underruns.store(0, Ordering::Relaxed);
        self.dropped_frames.store(0, Ordering::Relaxed);
        set_f(&self.proc_max_us, 0.0);
    }
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            input_peak: get_f(&self.input_peak),
            input_rms: get_f(&self.input_rms),
            output_peak: get_f(&self.output_peak),
            output_rms: get_f(&self.output_rms),
            lsnr_db: get_f(&self.lsnr_db),
            proc_us: get_f(&self.proc_us),
            proc_avg_us: get_f(&self.proc_avg_us),
            proc_max_us: get_f(&self.proc_max_us),
            cpu_pct: get_f(&self.cpu_pct),
            latency_ms: get_f(&self.latency_ms),
            frames: self.frames.load(Ordering::Relaxed),
            input_overruns: self.input_overruns.load(Ordering::Relaxed),
            output_underruns: self.output_underruns.load(Ordering::Relaxed),
            dropped_frames: self.dropped_frames.load(Ordering::Relaxed),
            voice_active: self.voice_active.load(Ordering::Relaxed),
            gate_open: self.gate_open.load(Ordering::Relaxed),
            processing: self.processing.load(Ordering::Relaxed),
            input_rate: self.input_rate.load(Ordering::Relaxed),
            output_rate: self.output_rate.load(Ordering::Relaxed),
        }
    }
}

/// Convert a linear amplitude to decibels, clamped to a display floor.
pub fn to_db(lin: f32) -> f32 {
    20.0 * lin.max(1e-6).log10()
}
