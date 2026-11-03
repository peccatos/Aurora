//! Playback queue: entries and advance/rewind semantics.

use serde::{Deserialize, Serialize};

/// One queue entry: the path plus what the engine needs from the library
/// (ReplayGain tags) and what the UI shows (display name).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QueueTrack {
    pub path: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub artist: Option<String>,
    #[serde(default)]
    pub replaygain_track_db: Option<f64>,
    #[serde(default)]
    pub replaygain_album_db: Option<f64>,
    /// Measured tempo (BPM), when the content has been analyzed. This is
    /// a measurement, not a tag echo — filled lazily on first deck load
    /// or explicit analysis and persisted in the library.
    #[serde(default)]
    pub bpm: Option<f64>,
}

impl QueueTrack {
    /// Display label: "Artist — Title" with file-stem fallback.
    pub fn display_line(&self) -> String {
        let title = self
            .title
            .as_deref()
            .unwrap_or_else(|| file_stem(&self.path));
        match &self.artist {
            Some(a) if !a.is_empty() => format!("{a} — {title}"),
            _ => title.to_string(),
        }
    }
}

/// File stem of a path (both separators, no extension).
fn file_stem(path: &str) -> &str {
    let start = path.rfind(['/', '\\']).map_or(0, |i| i + 1);
    let end = path.rfind('.').filter(|&i| i > start).unwrap_or(path.len());
    &path[start..end]
}

/// The user's playback queue.
#[derive(Default)]
pub struct TrackQueue {
    pub tracks: Vec<QueueTrack>,
    pub current: Option<usize>,
}

impl TrackQueue {
    /// Advance to the next entry; None at the end.
    pub fn advance(&mut self) -> Option<QueueTrack> {
        let idx = self.current? + 1;
        if idx >= self.tracks.len() {
            return None;
        }
        self.current = Some(idx);
        self.tracks.get(idx).cloned()
    }

    /// Rewind to the previous entry (0 wraps to the first).
    pub fn rewind(&mut self) -> Option<QueueTrack> {
        let idx = self.current?;
        if idx == 0 {
            return self.tracks.first().cloned();
        }
        self.current = Some(idx - 1);
        self.tracks.get(idx - 1).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(path: &str) -> QueueTrack {
        QueueTrack {
            path: path.into(),
            title: Some(format!("T {path}")),
            artist: None,
            replaygain_track_db: None,
            replaygain_album_db: None,
            bpm: None,
        }
    }

    #[test]
    fn display_line_uses_artist_title_and_stem_fallback() {
        let t = QueueTrack {
            path: "C:\\m\\a.flac".into(),
            title: Some("Song".into()),
            artist: Some("Artist".into()),
            replaygain_track_db: None,
            replaygain_album_db: None,
            bpm: None,
        };
        assert_eq!(t.display_line(), "Artist — Song");
        let bare = track("C:\\m\\b.mp3");
        assert_eq!(bare.display_line(), "T C:\\m\\b.mp3");
    }

    #[test]
    fn advance_moves_forward_and_stops_at_the_end() {
        let mut q = TrackQueue {
            tracks: vec![track("a"), track("b"), track("c")],
            current: Some(0),
        };
        assert_eq!(q.advance().unwrap().path, "b");
        q.current = Some(2);
        assert!(q.advance().is_none());
    }

    #[test]
    fn rewind_wraps_at_zero() {
        let mut q = TrackQueue {
            tracks: vec![track("a"), track("b")],
            current: Some(1),
        };
        assert_eq!(q.rewind().unwrap().path, "a");
        assert!(q.rewind().is_some()); // wraps to first
    }
}
