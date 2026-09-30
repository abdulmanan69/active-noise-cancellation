//! Device enumeration and format-agnostic capture/playback streams (cpal / WASAPI).

pub mod resampler;

use anyhow::{Context, Result, bail};
use cpal::traits::{DeviceTrait, HostTrait};
use cpal::{
    Device, FromSample, Sample, SampleFormat, SizedSample, Stream, StreamConfig,
    SupportedStreamConfig,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    Physical,
    VirtualCable,
}

#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub name: String,
    pub is_default: bool,
    pub sample_rate: u32,
    pub channels: u16,
    pub kind: DeviceKind,
}

#[derive(Debug, Clone, Default)]
pub struct DeviceList {
    pub inputs: Vec<DeviceInfo>,
    pub outputs: Vec<DeviceInfo>,
}

impl DeviceList {
    /// The render endpoint of an installed virtual cable, if any.
    pub fn virtual_cable_output(&self) -> Option<&DeviceInfo> {
        self.outputs
            .iter()
            .find(|d| {
                d.kind == DeviceKind::VirtualCable && d.name.to_lowercase().contains("cable input")
            })
            .or_else(|| {
                self.outputs
                    .iter()
                    .find(|d| d.kind == DeviceKind::VirtualCable)
            })
    }

    /// The capture endpoint other apps should select as their microphone.
    pub fn virtual_cable_input(&self) -> Option<&DeviceInfo> {
        self.inputs
            .iter()
            .find(|d| {
                d.kind == DeviceKind::VirtualCable && d.name.to_lowercase().contains("cable output")
            })
            .or_else(|| {
                self.inputs
                    .iter()
                    .find(|d| d.kind == DeviceKind::VirtualCable)
            })
    }
}

const VIRTUAL_MARKERS: &[&str] = &[
    "cable input",
    "cable output",
    "vb-audio",
    "voicemeeter",
    "virtual audio cable",
    "virtual cable",
    "blackhole",
    "soundflower",
    "clearmic",
];

pub fn is_virtual_cable_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    VIRTUAL_MARKERS.iter().any(|m| lower.contains(m))
}

pub fn device_name(d: &Device) -> String {
    d.description()
        .map(|desc| desc.name().to_string())
        .unwrap_or_else(|_| "Unknown device".to_string())
}

fn info(d: &Device, is_default: bool, input: bool) -> DeviceInfo {
    let name = device_name(d);
    let cfg = if input {
        d.default_input_config().ok()
    } else {
        d.default_output_config().ok()
    };
    DeviceInfo {
        kind: if is_virtual_cable_name(&name) {
            DeviceKind::VirtualCable
        } else {
            DeviceKind::Physical
        },
        is_default,
        sample_rate: cfg.as_ref().map(|c| c.sample_rate()).unwrap_or(0),
        channels: cfg.as_ref().map(|c| c.channels()).unwrap_or(0),
        name,
    }
}

/// Enumerate all capture and render endpoints of the default host.
pub fn enumerate() -> DeviceList {
    let host = cpal::default_host();
    let default_in = host.default_input_device().map(|d| device_name(&d));
    let default_out = host.default_output_device().map(|d| device_name(&d));
    let mut list = DeviceList::default();
    if let Ok(devs) = host.input_devices() {
        for d in devs {
            let is_default = default_in.as_deref() == Some(device_name(&d).as_str());
            list.inputs.push(info(&d, is_default, true));
        }
    }
    if let Ok(devs) = host.output_devices() {
        for d in devs {
            let is_default = default_out.as_deref() == Some(device_name(&d).as_str());
            list.outputs.push(info(&d, is_default, false));
        }
    }
    // physical devices first, defaults on top
    let rank = |d: &DeviceInfo| (d.kind == DeviceKind::VirtualCable, !d.is_default);
    list.inputs.sort_by_key(rank);
    list.outputs.sort_by_key(rank);
    list
}

/// Find a capture device by name, falling back to the system default.
pub fn find_input(name: Option<&str>) -> Option<Device> {
    let host = cpal::default_host();
    if let Some(name) = name {
        if let Ok(devs) = host.input_devices() {
            for d in devs {
                if device_name(&d) == name {
                    return Some(d);
                }
            }
        }
        log::warn!("microphone '{name}' not found, using default");
    }
    // Windows may make the virtual cable the default recording device. Capturing from it
    // would feed our own output back in, so prefer the first real microphone instead.
    let default = host.default_input_device();
    match &default {
        Some(d) if is_virtual_cable_name(&device_name(d)) => host
            .input_devices()
            .ok()
            .and_then(|mut devs| devs.find(|d| !is_virtual_cable_name(&device_name(d))))
            .or(default),
        _ => default,
    }
}

/// Find a render device by exact name. No fallback (callers decide).
pub fn find_output_exact(name: &str) -> Option<Device> {
    let host = cpal::default_host();
    host.output_devices()
        .ok()?
        .find(|d| device_name(d) == name)
}

/// Find the virtual cable render endpoint, if installed.
pub fn find_virtual_cable_output() -> Option<Device> {
    let list = enumerate();
    let name = list.virtual_cable_output()?.name.clone();
    find_output_exact(&name)
}

pub fn default_output() -> Option<Device> {
    cpal::default_host().default_output_device()
}

/// Build a capture stream that delivers mono `f32` regardless of the device format.
pub fn build_input_stream<F, E>(
    device: &Device,
    cfg: &SupportedStreamConfig,
    on_data: F,
    on_error: E,
) -> Result<Stream>
where
    F: FnMut(&[f32]) + Send + 'static,
    E: FnMut(cpal::Error) + Send + 'static,
{
    let channels = cfg.channels() as usize;
    let config = cfg.config();
    match cfg.sample_format() {
        SampleFormat::F32 => build_in::<f32, F, E>(device, config, channels, on_data, on_error),
        SampleFormat::I16 => build_in::<i16, F, E>(device, config, channels, on_data, on_error),
        SampleFormat::I32 => build_in::<i32, F, E>(device, config, channels, on_data, on_error),
        SampleFormat::U16 => build_in::<u16, F, E>(device, config, channels, on_data, on_error),
        SampleFormat::U8 => build_in::<u8, F, E>(device, config, channels, on_data, on_error),
        SampleFormat::I8 => build_in::<i8, F, E>(device, config, channels, on_data, on_error),
        SampleFormat::F64 => build_in::<f64, F, E>(device, config, channels, on_data, on_error),
        other => bail!("unsupported capture format {other:?}"),
    }
}

fn build_in<T, F, E>(
    device: &Device,
    config: StreamConfig,
    channels: usize,
    mut on_data: F,
    on_error: E,
) -> Result<Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
    F: FnMut(&[f32]) + Send + 'static,
    E: FnMut(cpal::Error) + Send + 'static,
{
    let channels = channels.max(1);
    let mut mono: Vec<f32> = Vec::with_capacity(8192);
    let stream = device
        .build_input_stream::<T, _, _>(
            config,
            move |data: &[T], _| {
                mono.clear();
                if channels == 1 {
                    mono.extend(data.iter().map(|s| f32::from_sample(*s)));
                } else {
                    let inv = 1.0 / channels as f32;
                    for frame in data.chunks_exact(channels) {
                        let sum: f32 = frame.iter().map(|s| f32::from_sample(*s)).sum();
                        mono.push(sum * inv);
                    }
                }
                on_data(&mono);
            },
            on_error,
            None,
        )
        .context("open capture stream")?;
    Ok(stream)
}

/// Build a render stream fed by a mono `f32` callback; channels are duplicated.
pub fn build_output_stream<F, E>(
    device: &Device,
    cfg: &SupportedStreamConfig,
    fill: F,
    on_error: E,
) -> Result<Stream>
where
    F: FnMut(&mut [f32]) + Send + 'static,
    E: FnMut(cpal::Error) + Send + 'static,
{
    let channels = cfg.channels() as usize;
    let config = cfg.config();
    match cfg.sample_format() {
        SampleFormat::F32 => build_out::<f32, F, E>(device, config, channels, fill, on_error),
        SampleFormat::I16 => build_out::<i16, F, E>(device, config, channels, fill, on_error),
        SampleFormat::I32 => build_out::<i32, F, E>(device, config, channels, fill, on_error),
        SampleFormat::U16 => build_out::<u16, F, E>(device, config, channels, fill, on_error),
        SampleFormat::U8 => build_out::<u8, F, E>(device, config, channels, fill, on_error),
        SampleFormat::I8 => build_out::<i8, F, E>(device, config, channels, fill, on_error),
        SampleFormat::F64 => build_out::<f64, F, E>(device, config, channels, fill, on_error),
        other => bail!("unsupported playback format {other:?}"),
    }
}

fn build_out<T, F, E>(
    device: &Device,
    config: StreamConfig,
    channels: usize,
    mut fill: F,
    on_error: E,
) -> Result<Stream>
where
    T: SizedSample + FromSample<f32>,
    F: FnMut(&mut [f32]) + Send + 'static,
    E: FnMut(cpal::Error) + Send + 'static,
{
    let channels = channels.max(1);
    let mut mono: Vec<f32> = vec![0.0; 8192];
    let stream = device
        .build_output_stream::<T, _, _>(
            config,
            move |data: &mut [T], _| {
                let frames = data.len() / channels;
                if mono.len() < frames {
                    mono.resize(frames, 0.0);
                }
                fill(&mut mono[..frames]);
                for (i, frame) in data.chunks_exact_mut(channels).enumerate() {
                    let v = T::from_sample(mono[i]);
                    for s in frame.iter_mut() {
                        *s = v;
                    }
                }
            },
            on_error,
            None,
        )
        .context("open playback stream")?;
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_virtual_cable_names() {
        assert!(is_virtual_cable_name("CABLE Input (VB-Audio Virtual Cable)"));
        assert!(is_virtual_cable_name("VoiceMeeter Input (VB-Audio VoiceMeeter VAIO)"));
        assert!(!is_virtual_cable_name("Microphone (USB Audio Device)"));
    }
}
