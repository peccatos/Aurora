//! Kernel DTOs: the data shapes every shell sees.
//!
//! These are the public API of the kernel — a shell maps them to JSON,
//! HTTP bodies or wasm values, but the shapes live here so all shells
//! agree on what Aurora is.

use serde::{Deserialize, Serialize};

use crate::queue::QueueTrack;

/// Status snapshot of the main playback engine (ADR 0002 fields included).
#[derive(Debug, Clone, Serialize)]
pub struct PlaybackStatus {
    pub state: aurora_audio::EngineState,
    /// Set when the stream errored (device loss) or the decode worker
    /// panicked since the last status poll. Faults are one-shot (cleared
    /// on read) so each fault reaches the UI exactly once.
    pub fault: Option<String>,
    /// Negotiated output sample rate, when playing.
    pub sample_rate: Option<u32>,
    pub channels: Option<u16>,
    pub bits_per_sample: Option<u16>,
    /// Active transformations, reported per ADR 0002.
    pub transformations: Vec<aurora_core::AudioTransformation>,
    /// ADR 0002 output path integrity classification.
    pub integrity: aurora_core::OutputIntegrity,
    /// Total track duration in seconds, when the container declares it.
    pub duration_s: Option<f64>,
    pub frames_played: u64,
    pub underruns: u64,
}

/// Track as any UI sees it: display fields + what playback needs.
#[derive(Debug, Clone, Serialize)]
pub struct TrackDto {
    pub id: String,
    pub path: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub genre: Option<String>,
    pub year: Option<i32>,
    pub track_number: Option<u32>,
    pub format: String,
    pub duration_ms: Option<u64>,
    pub replaygain_track_db: Option<f64>,
    pub replaygain_album_db: Option<f64>,
    /// Measured tempo (BPM), when analyzed.
    pub bpm: Option<f64>,
    /// "Artist - Title" (file stem when untagged): the correct name.
    pub display: String,
    /// Duration formatted "m:ss" for list rows.
    pub duration_display: String,
}

impl TrackDto {
    pub fn from_track(t: &aurora_core::Track) -> Self {
        let duration_display = t
            .duration_ms
            .map(|ms| {
                let sec = ms / 1000;
                format!("{}:{:02}", sec / 60, sec % 60)
            })
            .unwrap_or_default();
        Self {
            id: t.id.0.to_string(),
            path: t.path.clone(),
            title: t.title.clone(),
            artist: t.artist.as_ref().map(|a| a.name.clone()),
            album: t.album.as_ref().map(|a| a.title.clone()),
            genre: t.genre.clone(),
            year: t.year,
            track_number: t.track_number,
            format: format!("{:?}", t.format).to_lowercase(),
            duration_ms: t.duration_ms,
            replaygain_track_db: t.replaygain_track_db,
            replaygain_album_db: t.replaygain_album_db,
            bpm: t.bpm,
            display: t.display_line(),
            duration_display,
        }
    }
}

/// The user's playback queue, as a shell reads it.
#[derive(Debug, Serialize)]
pub struct QueueDto {
    pub tracks: Vec<QueueTrack>,
    pub current: Option<usize>,
    /// "Artist — Title" per entry, in order (the UI's correct names).
    pub display: Vec<String>,
}

/// Library status: open flag + track count.
#[derive(Debug, Clone, Serialize)]
pub struct LibraryStatus {
    pub open: bool,
    pub path: Option<String>,
    pub track_count: u64,
}

/// Deck status snapshot (Pioneer DDJ-style, two decks).
#[derive(Debug, Serialize)]
pub struct DeckDto {
    pub index: usize,
    /// Deck letter "A"/"B".
    pub name: String,
    pub loaded: Option<QueueTrack>,
    pub state: String,
    pub position_s: f64,
    pub duration_s: Option<f64>,
    /// Channel fader 0..1 (pre-crossfader).
    pub channel_fader: f64,
    /// Applied gain (fader + crossfader), dB.
    pub gain_db: f64,
    /// Hot cue positions (seconds), 8 slots.
    pub cues: Vec<Option<f64>>,
    /// Tempo multiplier (DspConfig.speed).
    pub speed: f64,
    /// Measured tempo of the loaded track (BPM), when analyzed.
    /// Effective BPM = bpm * speed.
    pub bpm: Option<f64>,
}

/// Honest import report for playlist import: what matched, what did not.
#[derive(Debug, Serialize)]
pub struct ImportReport {
    pub playlist_id: String,
    pub playlist_name: String,
    /// Entries that resolved to a local library track.
    pub matched: u64,
    /// Entries with no local counterpart (never fabricated as tracks).
    pub unmatched: u64,
    /// Path-based entries whose files existed but were not in the
    /// library; their folder was scanned to resolve them.
    pub resolved_by_scan: bool,
    /// Names of unmatched entries (title — artist), capped for the UI.
    pub unmatched_names: Vec<String>,
}

/// One VST3 chain slot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VstSlotDto {
    pub path: String,
    pub class_id: String,
}

/// One VST3 plugin as the scan reports it.
#[derive(Debug, Clone, Serialize)]
pub struct VstPluginDto {
    pub path: String,
    pub name: String,
    pub vendor: String,
    pub version: String,
    pub category: String,
    pub class_id: String,
    pub input_channels: i32,
    pub output_channels: i32,
}

/// One automatable plugin parameter (normalized 0..1 value).
#[derive(Debug, Clone, Serialize)]
pub struct VstParamDto {
    pub id: u32,
    pub title: String,
    pub value: f64,
}

/// Saved preset file entry.
#[derive(Debug, Clone, Serialize)]
pub struct VstPresetDto {
    pub name: String,
    pub path: String,
}

/// A play request: path + optional DSP override + ReplayGain tags.
#[derive(Debug, Clone, Deserialize)]
pub struct PlayRequest {
    pub path: String,
    /// Device key from `list_output_devices`; empty/None = default.
    #[serde(default)]
    pub device: Option<String>,
    /// DSP configuration for this stream; defaults to bypassed.
    #[serde(default)]
    pub dsp: aurora_dsp::DspConfig,
    /// ReplayGain tags for this track (from the library), when known.
    #[serde(default)]
    pub replaygain_track_db: Option<f64>,
    #[serde(default)]
    pub replaygain_album_db: Option<f64>,
}

/// Result of an exclusive (bit-perfect candidate) play attempt.
#[derive(Debug, Serialize)]
pub struct ExclusivePlayResult {
    /// True when the exclusive path was taken; false = fell back to the
    /// shared path (device refused the format — reported honestly).
    pub exclusive: bool,
    /// Why the fallback happened, when it did.
    pub fallback_reason: Option<String>,
    pub status: PlaybackStatus,
}

/// ADR 0002 verdict for the physical loopback measurement.
#[derive(Debug, Serialize)]
pub struct BitPerfectVerdict {
    /// True when an exclusive stream is playing.
    pub exclusive_active: bool,
    /// The physical measurement, when it ran.
    pub identity: Option<f64>,
    pub frames_compared: Option<u64>,
    /// Honest verdict ("verified" / "transformed" / "unverified" —
    /// unverified = measurement failed, never assumed).
    pub integrity: String,
    pub note: String,
}

/// Exclusive-path probe result (honest classification, no play).
#[derive(Debug, Serialize)]
pub struct ExclusiveProbe {
    pub playing: bool,
    /// Some(true): the device accepts the format in exclusive mode.
    pub exclusive_capable: Option<bool>,
    pub integrity: String,
    pub note: String,
}
