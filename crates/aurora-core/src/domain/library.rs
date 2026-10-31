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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Track {
    pub id: TrackId,
    pub path: String,
    pub title: Option<String>,
    pub artist: Option<Artist>,
    pub album: Option<Album>,
    pub format: AudioFormat,
    pub duration_ms: Option<u64>,
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
            format: AudioFormat::Unknown,
            duration_ms: None,
        }
    }
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
