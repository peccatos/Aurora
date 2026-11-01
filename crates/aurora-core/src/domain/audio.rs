//! Audio representation domain: PCM formats, sample representation and
//! output diagnostics contracts.
//!
//! These types are the vocabulary that decoder, DSP, audio and the tauri
//! adapter share; they must stay free of any backend-specific types (ADR 0001).

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Audio representation
// ---------------------------------------------------------------------------

/// How individual samples are represented in a PCM stream.
///
/// This distinction is load-bearing for the bit-perfect path: converting a
/// 32-bit integer stream to `f32` is a lossy transformation that must never
/// become invisible at the type level (ADR 0002).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SampleFormat {
    /// Signed integer samples, e.g. i16/i24/i32.
    SignedInt,
    /// Unsigned integer samples, e.g. u8.
    UnsignedInt,
    /// IEEE 754 floating point samples, e.g. f32.
    Float,
}

impl SampleFormat {
    /// Typical bit widths this representation supports on real hardware.
    pub fn typical_bit_widths(&self) -> &'static [u16] {
        match self {
            SampleFormat::SignedInt => &[16, 24, 32],
            SampleFormat::UnsignedInt => &[8],
            SampleFormat::Float => &[32, 64],
        }
    }
}

/// Channel layout of a PCM stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelLayout {
    /// Samples interleaved in a single buffer: L R L R ...
    Interleaved,
    /// Each channel in a separate plane.
    Planar,
}

/// The complete, unambiguous description of a PCM stream.
///
/// `bits_per_sample` is the actual sample width in bits (e.g. 24, not the
/// container's 32-bit storage width for 24-bit-in-32 formats).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PcmFormat {
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
    pub sample_format: SampleFormat,
    pub channel_layout: ChannelLayout,
}

impl PcmFormat {
    /// Bits per second of the raw PCM payload.
    pub fn bit_rate(&self) -> u64 {
        u64::from(self.sample_rate) * u64::from(self.channels) * u64::from(self.bits_per_sample)
    }

    /// Validate the format against physically plausible bounds.
    ///
    /// Catches nonsense values before they reach the audio path: zero
    /// channels, absurd sample rates, or width/representation combinations
    /// that no real decoder or device produces.
    pub fn is_plausible(&self) -> bool {
        let rate_ok = (1..=768_000).contains(&self.sample_rate);
        let channels_ok = (1..=64).contains(&self.channels);
        let bits_ok = (1..=64).contains(&self.bits_per_sample)
            && self
                .sample_format
                .typical_bit_widths()
                .contains(&self.bits_per_sample);
        rate_ok && channels_ok && bits_ok
    }
}

/// A transformation applied to the audio signal between decoder and device.
///
/// The diagnostics contract (ADR 0002) requires every active transformation
/// to be reported; this enum is the vocabulary for that report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioTransformation {
    SampleFormatConversion,
    Resampling,
    ChannelRemapping,
    Gain,
    Equalizer,
    ReplayGain,
    Reverb,
    Mixing,
    BitDepthTruncation,
    /// A third-party VST3 plugin is processing the signal (ADR 0004).
    /// Reported with the plugin's display name in diagnostics.
    ExternalPlugin,
}

// ---------------------------------------------------------------------------
// Output backend contract (ADR 0001)
// ---------------------------------------------------------------------------

/// Eligibility vs verification of bit-perfect output (ADR 0002).
///
/// Eligibility is a static property of source format and configured path.
/// Verification is a runtime property of the actual output chain and can
/// never be assumed from device selection alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputIntegrity {
    /// The full chain was verified bit-identical for this path.
    Verified,
    /// Transformations are active or the chain could not be verified.
    Transformed,
    /// Verification was not attempted or is unavailable on this platform.
    Unverified,
}

/// What the output layer reports about the negotiated stream (ADR 0001: no
/// backend-specific types leak into domain contracts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputDiagnostics {
    pub source_format: PcmFormat,
    pub output_format: PcmFormat,
    pub transformations: Vec<AudioTransformation>,
    pub integrity: OutputIntegrity,
}

/// Audio container/codec families the library recognizes.
///
/// The lossless/lossy split matters for bit-perfect eligibility: lossy
/// sources have already discarded information upstream, so bit-perfect
/// reporting for them is meaningless in the source sense, while lossless
/// container-to-PCM decoding can still be exact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioFormat {
    Flac,
    Wav,
    Alac,
    Aiff,
    Mp3,
    Aac,
    OggVorbis,
    Opus,
    WavPack,
    Unknown,
}

impl AudioFormat {
    pub fn is_lossless(&self) -> bool {
        matches!(
            self,
            AudioFormat::Flac
                | AudioFormat::Wav
                | AudioFormat::Alac
                | AudioFormat::Aiff
                | AudioFormat::WavPack
        )
    }

    /// Infer the format from a file extension (ASCII, case-insensitive).
    ///
    /// Returns `Unknown` rather than an error: an unrecognized extension is
    /// a normal library-scanning outcome, not a domain failure.
    pub fn from_extension(ext: &str) -> Self {
        match ext.trim_ascii().to_ascii_lowercase().as_str() {
            "flac" => AudioFormat::Flac,
            "wav" | "wave" => AudioFormat::Wav,
            "m4a" | "alac" => AudioFormat::Alac,
            "aif" | "aiff" => AudioFormat::Aiff,
            "mp3" => AudioFormat::Mp3,
            "aac" => AudioFormat::Aac,
            "ogg" | "oga" => AudioFormat::OggVorbis,
            "opus" => AudioFormat::Opus,
            "wv" => AudioFormat::WavPack,
            _ => AudioFormat::Unknown,
        }
    }
}
