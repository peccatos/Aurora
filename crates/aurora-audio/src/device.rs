//! Output device enumeration behind an Aurora-owned boundary (ADR 0001).
//!
//! CPAL types never leak past this module: the frontend sees
//! [`OutputDevice`] with a stable string key and a human-readable name.

use aurora_core::{ChannelLayout, PcmFormat, SampleFormat};
use cpal::traits::{DeviceTrait, HostTrait};

use crate::error::EngineError;

/// A selectable output device, identified by a stable key.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OutputDevice {
    /// Stable identity: host name + device name.
    key: String,
    /// Human-readable name for the UI.
    name: String,
}

impl OutputDevice {
    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

/// Formats the device can accept, as negotiated PCM configurations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceFormat {
    pub pcm: PcmFormat,
}

/// List output devices of the default host.
pub fn output_devices() -> Result<Vec<OutputDevice>, EngineError> {
    let host = cpal::default_host();
    let host_name = host.id().to_string();

    let devices = host
        .output_devices()
        .map_err(|e| EngineError::Internal(e.to_string()))?;

    let mut out = Vec::new();
    for device in devices {
        let name = device.to_string();
        let key = format!("{host_name}:{name}");
        out.push(OutputDevice { key, name });
    }

    if out.is_empty() {
        return Err(EngineError::NoOutputDevices);
    }
    Ok(out)
}

/// The default output device of the default host, if any.
pub fn default_output_device() -> Result<OutputDevice, EngineError> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or(EngineError::NoOutputDevices)?;

    let host_name = host.id().to_string();
    let name = device.to_string();
    Ok(OutputDevice {
        key: format!("{host_name}:{name}"),
        name,
    })
}

/// Resolve a device key back to the CPAL device.
pub(crate) fn find_by_key(key: &str) -> Result<cpal::Device, EngineError> {
    let host = cpal::default_host();
    let devices = host
        .output_devices()
        .map_err(|e| EngineError::Internal(e.to_string()))?;

    for device in devices {
        let name = device.to_string();
        if format!("{}:{}", host.id(), name) == key {
            return Ok(device);
        }
    }
    Err(EngineError::DeviceNotFound(key.to_string()))
}

/// Negotiate a stream configuration for a device given the source PCM.
///
/// Strategy (Phase 3 — no resampling/DSP yet): if the device supports the
/// source rate/channels directly, use them; otherwise fall back to the
/// device default and report the mismatch via the returned diagnostics.
/// The mismatch itself is *reported*, never hidden (ADR 0002).
pub(crate) fn negotiate(
    device: &cpal::Device,
    source: PcmFormat,
) -> Result<NegotiatedConfig, EngineError> {
    let supported = device
        .supported_output_configs()
        .map_err(|e| EngineError::FormatUnsupported(e.to_string()))?;

    // Try to match: source rate within range, same channels.
    for cfg in supported {
        let min = cfg.min_sample_rate();
        let max = cfg.max_sample_rate();
        if (min..=max).contains(&source.sample_rate) && cfg.channels() == source.channels {
            let rate = source.sample_rate;
            let config = cpal::StreamConfig {
                channels: cfg.channels(),
                sample_rate: rate,
                buffer_size: cpal::BufferSize::Default,
            };
            let pcm = PcmFormat {
                sample_rate: source.sample_rate,
                channels: source.channels,
                bits_per_sample: source.bits_per_sample,
                sample_format: source.sample_format,
                channel_layout: ChannelLayout::Interleaved,
            };
            return Ok(NegotiatedConfig {
                stream_config: config,
                device_sample_format: cfg.sample_format(),
                pcm,
                transformations: Vec::new(),
            });
        }
    }

    // No exact match: use the device default and flag the conversion.
    let default = device
        .default_output_config()
        .map_err(|e| EngineError::FormatUnsupported(e.to_string()))?;

    let config = cpal::StreamConfig {
        channels: default.channels(),
        sample_rate: default.sample_rate(),
        buffer_size: cpal::BufferSize::Default,
    };

    let pcm = PcmFormat {
        sample_rate: default.sample_rate(),
        channels: default.channels(),
        bits_per_sample: source.bits_per_sample,
        sample_format: sample_format_from_cpal(default.sample_format()),
        channel_layout: ChannelLayout::Interleaved,
    };

    let mut transformations = Vec::new();
    if pcm.sample_rate != source.sample_rate {
        transformations.push(aurora_core::AudioTransformation::Resampling);
    }
    if pcm.channels != source.channels {
        transformations.push(aurora_core::AudioTransformation::ChannelRemapping);
    }
    if pcm.sample_format != source.sample_format || pcm.bits_per_sample != source.bits_per_sample {
        transformations.push(aurora_core::AudioTransformation::SampleFormatConversion);
    }

    Ok(NegotiatedConfig {
        stream_config: config,
        device_sample_format: default.sample_format(),
        pcm,
        transformations,
    })
}

/// The result of format negotiation.
#[derive(Debug, Clone)]
pub(crate) struct NegotiatedConfig {
    pub stream_config: cpal::StreamConfig,
    pub device_sample_format: cpal::SampleFormat,
    pub pcm: PcmFormat,
    /// Transformations the fallback path applies (empty for a direct match).
    pub transformations: Vec<aurora_core::AudioTransformation>,
}

pub(crate) fn sample_format_from_cpal(format: cpal::SampleFormat) -> SampleFormat {
    match format {
        cpal::SampleFormat::I8
        | cpal::SampleFormat::I16
        | cpal::SampleFormat::I24
        | cpal::SampleFormat::I32
        | cpal::SampleFormat::I64 => SampleFormat::SignedInt,
        cpal::SampleFormat::U8
        | cpal::SampleFormat::U16
        | cpal::SampleFormat::U24
        | cpal::SampleFormat::U32
        | cpal::SampleFormat::U64 => SampleFormat::UnsignedInt,
        cpal::SampleFormat::F32 | cpal::SampleFormat::F64 => SampleFormat::Float,
        other => {
            tracing::warn!(?other, "unmapped cpal sample format, assuming signed int");
            SampleFormat::SignedInt
        }
    }
}
