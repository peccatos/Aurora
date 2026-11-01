//! Metadata extraction: reads tags and audio properties from real files
//! via lofty, mapped onto the domain `Track` shape.
//!
//! Design rules:
//! - Extraction never fails the whole scan: a file without tags is a
//!   normal outcome (fields stay `None`), and a corrupt file is reported
//!   as an error the scanner can skip and log.
//! - ReplayGain tags are parsed here (they live in arbitrary tag frames
//!   across formats: "REPLAYGAIN_TRACK_GAIN" etc.), because the DSP layer
//!   should only ever see clean dB values.

use std::path::Path;
use std::time::Duration;

use aurora_core::{Album, Artist, Track};
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::prelude::Accessor;
use lofty::tag::{ItemKey, Tag};

#[derive(Debug, thiserror::Error)]
pub enum MetadataError {
    #[error("failed to read file: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to parse audio metadata: {0}")]
    Parse(#[from] lofty::error::FileParseError),
}

/// Tag values a scanner needs, extracted in one file read.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrackMetadata {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub genre: Option<String>,
    pub year: Option<i32>,
    pub track_number: Option<u32>,
    pub duration_ms: Option<u64>,
    pub replaygain_track_db: Option<f64>,
    pub replaygain_album_db: Option<f64>,
}

/// Read tags and audio properties from an audio file.
///
/// Missing tags are `None` (not an error); only unreadable/corrupt files
/// produce `Err`.
pub fn read_metadata(path: &Path) -> Result<TrackMetadata, MetadataError> {
    let tagged = lofty::probe::read_from_path(path)?;
    Ok(metadata_from(
        tagged.properties().duration(),
        tagged.first_tag(),
    ))
}

/// Pure mapping from lofty results to the metadata struct.
///
/// Exposed for tests: the lofty glue is trivial, the value is in the
/// parsing rules (ReplayGain string formats, empty-tag handling).
pub fn metadata_from(duration: Duration, tag: Option<&Tag>) -> TrackMetadata {
    let tag = match tag {
        Some(t) => t,
        None => return TrackMetadata::default_from(duration),
    };

    let title = Accessor::title(tag)
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    let artist = Accessor::artist(tag)
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    let album = Accessor::album(tag)
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    let genre = Accessor::genre(tag)
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    let year = Accessor::date(tag)
        .map(|t| i32::from(t.year))
        .filter(|y| *y > 0);
    let track_number = Accessor::track(tag).filter(|t| *t > 0);

    let replaygain_track_db = parse_replaygain_db(tag.get_string(ItemKey::ReplayGainTrackGain));
    let replaygain_album_db = parse_replaygain_db(tag.get_string(ItemKey::ReplayGainAlbumGain));

    TrackMetadata {
        title,
        artist,
        album,
        genre,
        year,
        track_number,
        duration_ms: Some(duration.as_millis() as u64).filter(|ms| *ms > 0),
        replaygain_track_db,
        replaygain_album_db,
    }
}

impl TrackMetadata {
    /// An all-`None` record with only the duration filled in.
    fn default_from(duration: Duration) -> Self {
        Self {
            duration_ms: Some(duration.as_millis() as u64).filter(|ms| *ms > 0),
            ..Self::default()
        }
    }
}

/// Parse a ReplayGain gain value: `"-6.5 dB"`, `"-6.5"`, `"+2.0 dB"`.
///
/// Empty/garbage values yield `None` — a nonsense tag must not become a
/// nonsense volume change.
pub fn parse_replaygain_db(raw: Option<&str>) -> Option<f64> {
    let raw = raw?.trim();
    // Values are conventionally suffixed with " dB" / "db" / "DB".
    let stripped = raw
        .strip_suffix("dB")
        .or_else(|| raw.strip_suffix("db"))
        .or_else(|| raw.strip_suffix(" db"))
        .or_else(|| raw.strip_suffix(" DB"))
        .unwrap_or(raw)
        .trim();
    let value: f64 = stripped.parse().ok()?;
    // Reject absurd tags; real ReplayGain lives within ±50 dB comfortably.
    (-50.0..=50.0).contains(&value).then_some(value)
}

impl TrackMetadata {
    /// Apply to a domain `Track` (id/path/format already set by the scanner).
    pub fn apply_to(&self, track: &mut Track) {
        track.title = self.title.clone();
        track.artist = self.artist.clone().map(Artist::new);
        track.album = self.album.clone().map(|title| Album {
            title,
            artist: None,
        });
        track.genre = self.genre.clone();
        track.year = self.year;
        track.track_number = self.track_number;
        track.duration_ms = self.duration_ms;
        track.replaygain_track_db = self.replaygain_track_db;
        track.replaygain_album_db = self.replaygain_album_db;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lofty::tag::Tag;
    use lofty::tag::TagType;

    fn wav_path(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
        dir.join(name)
    }

    fn write_sine_wav(path: &std::path::Path, frames: usize) {
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).expect("create wav");
        for i in 0..frames {
            let v = ((i as f64 * 0.05).sin() * 10_000.0) as i16;
            writer.write_sample(v).unwrap();
            writer.write_sample(v).unwrap();
        }
        writer.finalize().unwrap();
    }

    #[test]
    fn untagged_wav_has_no_tags_but_real_duration() {
        let dir = tempfile::tempdir().unwrap();
        let path = wav_path(dir.path(), "a.wav");
        write_sine_wav(&path, 44_100);
        let meta = read_metadata(&path).unwrap();
        assert_eq!(meta.title, None);
        assert_eq!(meta.duration_ms, Some(1_000));
    }

    #[test]
    fn empty_tag_fields_become_none_not_whitespace() {
        let mut tag = Tag::new(TagType::Id3v2);
        tag.set_title("  ".into());
        tag.set_artist("".into());
        let meta = metadata_from(Duration::from_secs(10), Some(&tag));
        assert_eq!(meta.title, None);
        assert_eq!(meta.artist, None);
        assert_eq!(meta.duration_ms, Some(10_000));
    }

    #[test]
    fn tagged_fields_map_through() {
        let mut tag = Tag::new(TagType::Id3v2);
        tag.set_title("Nightcall".into());
        tag.set_artist("Kavinsky".into());
        tag.set_album("OutRun".into());
        tag.set_genre("Synthwave".into());
        tag.set_track(1);
        tag.insert_text(ItemKey::ReplayGainTrackGain, "-6.53 dB".into());
        tag.insert_text(ItemKey::ReplayGainAlbumGain, "-5.80 dB".into());
        let meta = metadata_from(Duration::from_secs(254), Some(&tag));
        assert_eq!(meta.title.as_deref(), Some("Nightcall"));
        assert_eq!(meta.artist.as_deref(), Some("Kavinsky"));
        assert_eq!(meta.album.as_deref(), Some("OutRun"));
        assert_eq!(meta.track_number, Some(1));
        assert!((meta.replaygain_track_db.unwrap() - (-6.53)).abs() < 1e-9);
        assert!((meta.replaygain_album_db.unwrap() - (-5.8)).abs() < 1e-9);
    }

    #[test]
    fn replaygain_parsing_accepts_known_forms_and_rejects_garbage() {
        assert_eq!(parse_replaygain_db(Some("-6.5 dB")), Some(-6.5));
        assert_eq!(parse_replaygain_db(Some("-6.5")), Some(-6.5));
        assert_eq!(parse_replaygain_db(Some("+2.0 dB")), Some(2.0));
        assert_eq!(parse_replaygain_db(Some("")), None);
        assert_eq!(parse_replaygain_db(Some("loud")), None);
        assert_eq!(parse_replaygain_db(Some("9999 dB")), None);
        assert_eq!(parse_replaygain_db(None), None);
    }

    #[test]
    fn metadata_applies_to_domain_track() {
        let meta = TrackMetadata {
            title: Some("Song".into()),
            artist: Some("Beth".into()),
            album: Some("LP".into()),
            genre: Some("Pop".into()),
            year: Some(2024),
            track_number: Some(3),
            duration_ms: Some(180_000),
            replaygain_track_db: Some(-3.0),
            replaygain_album_db: None,
        };
        let mut track = Track::new(aurora_core::TrackId(uuid::Uuid::new_v4()), "m/song.flac");
        meta.apply_to(&mut track);
        assert_eq!(track.title.as_deref(), Some("Song"));
        assert_eq!(track.artist.as_ref().map(|a| a.name.as_str()), Some("Beth"));
        assert_eq!(track.display_line(), "Beth — Song");
    }
}
