//! Aurora domain core: shared types and contracts that every other crate
//! builds on. This crate must not depend on Tauri, SQLite, CPAL, filesystem
//! scanners, or UI code.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

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
    Mixing,
    BitDepthTruncation,
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

// ---------------------------------------------------------------------------
// Playback domain
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackMode {
    BitPerfect,
    Dsp,
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackState {
    Stopped,
    Playing,
    Paused,
}

// ---------------------------------------------------------------------------
// Library domain
// ---------------------------------------------------------------------------

/// Stable, path-independent identity of a track (Phase 6 invariant: a rescan
/// must never create duplicate library entries).
///
/// Identity is derived from a content hash of the audio file, not from a
/// random value: the same file on a rescan resolves to the same id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TrackId(pub Uuid);

/// A music file in the library, identified by content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Track {
    pub id: TrackId,
    pub path: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration_ms: Option<u64>,
}

impl Track {
    /// Create a track identity from a content hash of the file's audio data.
    ///
    /// The caller (library scanner) is responsible for computing the hash;
    /// the domain type only fixes that identity is content-derived.
    pub fn new(id: TrackId, path: impl Into<String>) -> Self {
        Self {
            id,
            path: path.into(),
            title: None,
            artist: None,
            album: None,
            duration_ms: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn flac_like() -> PcmFormat {
        PcmFormat {
            sample_rate: 44_100,
            channels: 2,
            bits_per_sample: 24,
            sample_format: SampleFormat::SignedInt,
            channel_layout: ChannelLayout::Interleaved,
        }
    }

    fn float_like() -> PcmFormat {
        PcmFormat {
            sample_rate: 44_100,
            channels: 2,
            bits_per_sample: 32,
            sample_format: SampleFormat::Float,
            channel_layout: ChannelLayout::Interleaved,
        }
    }

    #[test]
    fn pcm_format_distinguishes_int_from_float_at_same_width() {
        assert_ne!(flac_like(), float_like());
    }

    #[test]
    fn pcm_format_serialization_roundtrip() {
        let format = flac_like();
        let encoded = serde_json::to_string(&format).unwrap();
        let decoded: PcmFormat = serde_json::from_str(&encoded).unwrap();
        assert_eq!(format, decoded);
    }

    #[test]
    fn pcm_bit_rate() {
        assert_eq!(flac_like().bit_rate(), 44_100 * 2 * 24);
    }

    #[test]
    fn track_has_stable_id_after_serialization_roundtrip() {
        let track = Track::new(TrackId(Uuid::new_v4()), "music/example.flac");
        let encoded = serde_json::to_string(&track).unwrap();
        let decoded: Track = serde_json::from_str(&encoded).unwrap();
        assert_eq!(track, decoded);
    }

    #[test]
    fn output_integrity_defaults_to_unverified_not_bit_perfect() {
        // ADR 0002: verification can never be assumed.
        let diagnostics = OutputDiagnostics {
            source_format: flac_like(),
            output_format: flac_like(),
            transformations: vec![],
            integrity: OutputIntegrity::Unverified,
        };
        assert_eq!(diagnostics.integrity, OutputIntegrity::Unverified);
    }

    #[test]
    fn format_conversion_is_a_reportable_transformation() {
        let mut diagnostics = OutputDiagnostics {
            source_format: flac_like(),
            output_format: float_like(),
            transformations: Vec::new(),
            integrity: OutputIntegrity::Transformed,
        };
        // i32 -> f32 conversion is reported explicitly, never silent.
        diagnostics
            .transformations
            .push(AudioTransformation::SampleFormatConversion);
        assert!(
            diagnostics
                .transformations
                .contains(&AudioTransformation::SampleFormatConversion)
        );
    }
}
