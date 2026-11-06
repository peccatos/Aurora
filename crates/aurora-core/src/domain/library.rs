//! Library domain: tracks, artists, albums and their identity contracts.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::audio::AudioFormat;

/// Stable, path-independent identity of a track (Phase 6 invariant: a rescan
/// must never create duplicate library entries).
///
/// Identity is derived from a content hash of the audio file, not from a
/// random value: the same file on a rescan resolves to the same id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TrackId(pub Uuid);

impl TrackId {
    /// Derive an id deterministically from file content.
    ///
    /// Takes any 32-byte hash digest (blake3/sha-256 from the scanner) and
    /// folds it into the 16-byte UUID space. The mapping digest -> id is
    /// stable: identical content always yields the same id regardless of
    /// file path.
    ///
    /// Folding uses the first 16 bytes directly. A good hash already
    /// distributes uniformly, so truncation preserves the practical
    /// collision resistance of 128 random bits.
    pub fn from_content_digest(digest: [u8; 32]) -> Self {
        let mut truncated = [0u8; 16];
        truncated.copy_from_slice(&digest[..16]);
        Self(Uuid::from_bytes(truncated))
    }
}

/// A music file in the library, identified by content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: TrackId,
    pub path: String,
    pub title: Option<String>,
    pub artist: Option<Artist>,
    pub album: Option<Album>,
    pub genre: Option<String>,
    pub year: Option<i32>,
    /// Position on the album (1-based), when tagged.
    pub track_number: Option<u32>,
    pub format: AudioFormat,
    pub duration_ms: Option<u64>,
    /// ReplayGain track gain tag in dB, when present.
    ///
    /// Read from tags by the metadata layer (Phase 6); consumed by the DSP
    /// pipeline when the user enables ReplayGain normalization.
    pub replaygain_track_db: Option<f64>,
    /// ReplayGain album gain tag in dB, when present.
    pub replaygain_album_db: Option<f64>,
    /// Detected tempo in beats per minute, when measured.
    ///
    /// Measured (not guessed from tags): the analyzer decodes the content
    /// and derives the beat grid from onset energy. It is `None` until the
    /// first measurement of this file's content; a rescan never fabricates
    /// a value. The measurement is honest: a half/double-tempo ambiguity
    /// is NOT silently resolved — the analyzer keeps the range it actually
    /// observed (see `aurora-decoder`'s tempo analyzer).
    pub bpm: Option<f64>,
    /// Plain or synced lyrics supplied by a tag, user, or provider.
    pub lyrics: Option<String>,
    pub lyrics_language: Option<String>,
    /// Optional translated lyrics; never presented as the original text.
    pub lyrics_translation: Option<String>,
    pub lyrics_translation_language: Option<String>,
    /// Embedded or user-selected artwork as a data URL.
    pub artwork_data_url: Option<String>,
    /// Provenance for remote enrichment/import.
    pub source_url: Option<String>,
    pub source_provider: Option<String>,
    pub video_url: Option<String>,
    pub metadata_source: Option<String>,
}

impl Track {
    /// Create a track with the given content identity.
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
            genre: None,
            year: None,
            track_number: None,
            format: AudioFormat::Unknown,
            duration_ms: None,
            replaygain_track_db: None,
            replaygain_album_db: None,
            bpm: None,
            lyrics: None,
            lyrics_language: None,
            lyrics_translation: None,
            lyrics_translation_language: None,
            artwork_data_url: None,
            source_url: None,
            source_provider: None,
            video_url: None,
            metadata_source: None,
        }
    }

    /// The name a UI should display for this track ("correct names" rule):
    /// the tagged title, or — for untagged files — the file stem, never the
    /// full raw path and never a synthetic placeholder.
    pub fn display_title(&self) -> &str {
        self.title.as_deref().unwrap_or_else(|| {
            let p = self.path.as_str();
            let stem_start = p.rfind(['/', '\\']).map_or(0, |i| i + 1);
            let stem_end = p.rfind('.').filter(|&i| i > stem_start).unwrap_or(p.len());
            &p[stem_start..stem_end]
        })
    }

    /// One-line label: "Artist — Title" or just the title when untagged.
    pub fn display_line(&self) -> String {
        match &self.artist {
            Some(a) => format!("{} — {}", a.name, self.display_title()),
            None => self.display_title().to_string(),
        }
    }
}

/// A saved, named, ordered list of library tracks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Playlist {
    /// Stable identifier (UUID string form).
    pub id: String,
    pub name: String,
    /// Number of tracks in the playlist (denormalized for list views).
    pub track_count: u64,
}

/// A performing artist. Identity by name for now; storage may promote to a
/// stable row id later without changing this domain shape.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Artist {
    pub name: String,
}

impl Artist {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

/// An album, optionally attributed to an artist.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Album {
    pub title: String,
    pub artist: Option<Artist>,
}

impl Album {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            artist: None,
        }
    }
}
