//! Output device enumeration behind an Aurora-owned boundary (ADR 0001).
//!
//! CPAL types never leak past this module: the frontend sees
//! [`OutputDevice`] with a stable string key and a human-readable name.
//!
//! The wasm build (Aurora Web) has no CPAL: the browser owns output
//! through an AudioWorklet driven by the JS shell. Device enumeration
//! there is the single default endpoint, and "negotiation" is honest
//! identity — the worklet consumes f32 interleaved at the source rate
//! (resampling, when the device needs it, happens in the browser's
//! AudioContext and is beyond our measurement — reported, not claimed).

use aurora_core::{ChannelLayout, PcmFormat, SampleFormat};

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

/// The result of format negotiation.
#[derive(Debug, Clone)]
pub(crate) struct NegotiatedConfig {
    /// PCM wire format the fallback path converts into.
    pub pcm: PcmFormat,
    /// Transformations the fallback path applies (empty for a native match).
    pub transformations: Vec<aurora_core::AudioTransformation>,
}

// ---------------------------------------------------------------------------
// Native (CPAL) implementation
// ---------------------------------------------------------------------------

#[cfg(not(target_family = "wasm"))]
pub(crate) mod imp {
    use super::*;

    use cpal::traits::{DeviceTrait, HostTrait};

    /// Extra native-only fields (CPAL stream config).
    pub(crate) struct NativeConfig {
        pub stream_config: cpal::StreamConfig,
        pub device_sample_format: cpal::SampleFormat,
    }

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
    pub fn find_by_key(key: &str) -> Result<cpal::Device, EngineError> {
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
    /// Strategy (Phase 3): a *native* match requires rate, channels AND the
    /// sample representation to line up — anything else would reinterpret
    /// decoder bytes in a different wire format (the white-noise bug this
    /// fixed). When no native match exists, the device default is used and the
    /// fallback path converts to the device's exact format, with every applied
    /// transformation reported (ADR 0002).
    pub fn negotiate(
        device: &cpal::Device,
        source: PcmFormat,
    ) -> Result<(NegotiatedConfig, NativeConfig), EngineError> {
        let supported = device
            .supported_output_configs()
            .map_err(|e| EngineError::FormatUnsupported(e.to_string()))?;

        // Native match: same channels, source rate in range, AND a device
        // sample format whose wire representation is byte-compatible with the
        // source (same format family and bit width).
        for cfg in supported {
            let min = cfg.min_sample_rate();
            let max = cfg.max_sample_rate();
            if (min..=max).contains(&source.sample_rate)
                && cfg.channels() == source.channels
                && wire_compatible(cfg.sample_format(), source)
            {
                let config = cpal::StreamConfig {
                    channels: cfg.channels(),
                    sample_rate: source.sample_rate,
                    buffer_size: cpal::BufferSize::Default,
                };
                let pcm = PcmFormat {
                    sample_rate: source.sample_rate,
                    channels: source.channels,
                    bits_per_sample: cpal_bits_per_sample(cfg.sample_format()),
                    sample_format: source.sample_format,
                    channel_layout: ChannelLayout::Interleaved,
                };
                let native = NativeConfig {
                    stream_config: config,
                    device_sample_format: cfg.sample_format(),
                };
                return Ok((
                    NegotiatedConfig {
                        pcm,
                        transformations: Vec::new(),
                    },
                    native,
                ));
            }
        }

        // No native match: use the device default; the fallback path converts
        // samples to the device's exact format. Conversions are listed, never
        // hidden.
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
            bits_per_sample: cpal_bits_per_sample(default.sample_format()),
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
        if pcm.sample_format != source.sample_format
            || pcm.bits_per_sample != source.bits_per_sample
        {
            transformations.push(aurora_core::AudioTransformation::SampleFormatConversion);
        }

        let native = NativeConfig {
            stream_config: config,
            device_sample_format: default.sample_format(),
        };
        Ok((
            NegotiatedConfig {
                pcm,
                transformations,
            },
            native,
        ))
    }

    /// True when the device's cpal sample format has the same wire
    /// representation as the source PCM samples (what the decoder pushes
    /// into the ring byte-for-byte).
    fn wire_compatible(device: cpal::SampleFormat, source: PcmFormat) -> bool {
        match (device, source.sample_format) {
            (cpal::SampleFormat::I8, SampleFormat::SignedInt) => source.bits_per_sample == 8,
            (cpal::SampleFormat::I16, SampleFormat::SignedInt) => source.bits_per_sample == 16,
            // CPAL I24 is 24 significant bits in a 4-byte container; the
            // decoder's PcmChunk stores 3-byte packed samples, which is NOT
            // the same wire format -> not wire-compatible (must convert).
            (cpal::SampleFormat::I32, SampleFormat::SignedInt) => source.bits_per_sample == 32,
            (cpal::SampleFormat::U8, SampleFormat::UnsignedInt) => source.bits_per_sample == 8,
            (cpal::SampleFormat::F32, SampleFormat::Float) => source.bits_per_sample == 32,
            (cpal::SampleFormat::F64, SampleFormat::Float) => source.bits_per_sample == 64,
            _ => false,
        }
    }

    /// Significant bits per sample of a cpal format.
    fn cpal_bits_per_sample(format: cpal::SampleFormat) -> u16 {
        match format {
            cpal::SampleFormat::I8 | cpal::SampleFormat::U8 => 8,
            cpal::SampleFormat::I16 | cpal::SampleFormat::U16 => 16,
            cpal::SampleFormat::I24 | cpal::SampleFormat::U24 => 24,
            cpal::SampleFormat::I32 | cpal::SampleFormat::U32 | cpal::SampleFormat::F32 => 32,
            cpal::SampleFormat::I64 | cpal::SampleFormat::U64 | cpal::SampleFormat::F64 => 64,
            _ => 16,
        }
    }

    fn sample_format_from_cpal(format: cpal::SampleFormat) -> SampleFormat {
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
}

// ---------------------------------------------------------------------------
// wasm (browser) implementation
// ---------------------------------------------------------------------------

#[cfg(target_family = "wasm")]
pub(crate) mod imp {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// The browser device sample rate, set by the JS shell once it has an
    /// AudioContext (`ctx.sampleRate`). 0 = unknown yet; the engine then
    /// keeps the source rate and the BROWSER resamples — which is
    /// per-buffer and phase-discontinuous at chunk boundaries (the
    /// "frequencies jumping" bug). With the rate known, the engine's DSP
    /// resampler converts to the device rate as ONE continuous stream and
    /// reports the `Resampling` transformation honestly (ADR 0002).
    static DEVICE_RATE: AtomicU32 = AtomicU32::new(0);

    /// Called by the wasm shell (WebKernel::set_output_rate) at startup.
    pub fn set_device_rate(rate: u32) {
        if rate > 0 {
            DEVICE_RATE.store(rate, Ordering::Release);
        }
    }

    /// The single browser output (the default endpoint).
    pub fn output_devices() -> Result<Vec<OutputDevice>, EngineError> {
        Ok(vec![default_output_device()?])
    }

    pub fn default_output_device() -> Result<OutputDevice, EngineError> {
        Ok(OutputDevice {
            key: "web:default".into(),
            name: "Browser output".into(),
        })
    }

    /// Honest "negotiation": the JS bridge consumes f32 interleaved at
    /// the DEVICE rate when known (the engine resamples through its DSP
    /// pipeline — one continuous stream, reported as `Resampling`).
    /// Without a known rate the source rate passes through and the
    /// browser resamples per-buffer (phase-discontinuous; kept as the
    /// honest fallback and reported). Every conversion is listed.
    pub fn negotiate(source: PcmFormat) -> Result<NegotiatedConfig, EngineError> {
        let device_rate = DEVICE_RATE.load(Ordering::Acquire);
        let target_rate = if device_rate > 0 {
            device_rate
        } else {
            source.sample_rate
        };
        let pcm = PcmFormat {
            sample_rate: target_rate,
            channels: source.channels,
            bits_per_sample: 32,
            sample_format: SampleFormat::Float,
            channel_layout: ChannelLayout::Interleaved,
        };
        let mut transformations = Vec::new();
        if pcm.sample_rate != source.sample_rate {
            transformations.push(aurora_core::AudioTransformation::Resampling);
        }
        if source.sample_format != SampleFormat::Float || source.bits_per_sample != 32 {
            transformations.push(aurora_core::AudioTransformation::SampleFormatConversion);
        }
        Ok(NegotiatedConfig {
            pcm,
            transformations,
        })
    }
}

#[cfg(not(target_family = "wasm"))]
pub use imp::find_by_key;
pub use imp::{default_output_device, output_devices};

/// Wasm: JS shell reports the AudioContext's device rate once at
/// startup so the engine resamples as one continuous stream. Native:
/// no-op (CPAL negotiates against the real device) — the function
/// exists on BOTH targets so callers compile identically everywhere.
pub fn set_output_rate(rate: u32) {
    #[cfg(target_family = "wasm")]
    {
        imp::set_device_rate(rate);
    }
    #[cfg(not(target_family = "wasm"))]
    {
        // Native: the device rate is negotiated per-stream via CPAL;
        // a browser-reported rate is meaningless here.
        let _ = rate;
    }
}
