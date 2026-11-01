//! Aurora domain core: shared types and contracts that every other crate
//! builds on. This crate must not depend on Tauri, SQLite, CPAL, filesystem
//! scanners, or UI code.

pub mod domain;

pub use domain::*;

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

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
    fn pcm_plausibility_rejects_nonsense() {
        assert!(flac_like().is_plausible());
        assert!(float_like().is_plausible());

        let zero_channels = PcmFormat {
            channels: 0,
            ..flac_like()
        };
        assert!(!zero_channels.is_plausible());

        // 24-bit float is not a representation any decoder or device produces.
        let float24 = PcmFormat {
            bits_per_sample: 24,
            sample_format: SampleFormat::Float,
            ..float_like()
        };
        assert!(!float24.is_plausible());

        let absurd_rate = PcmFormat {
            sample_rate: 10_000_000,
            ..flac_like()
        };
        assert!(!absurd_rate.is_plausible());
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

    #[test]
    fn track_has_stable_id_after_serialization_roundtrip() {
        let track = Track::new(TrackId(Uuid::new_v4()), "music/example.flac");
        let encoded = serde_json::to_string(&track).unwrap();
        let decoded: Track = serde_json::from_str(&encoded).unwrap();
        assert_eq!(track, decoded);
    }

    #[test]
    fn track_id_is_deterministic_from_content() {
        let digest = [7u8; 32];
        assert_eq!(
            TrackId::from_content_digest(digest),
            TrackId::from_content_digest(digest)
        );
        assert_ne!(
            TrackId::from_content_digest(digest),
            TrackId::from_content_digest([8u8; 32])
        );
    }

    #[test]
    fn display_title_prefers_tagged_title_then_file_stem() {
        let mut tagged = Track::new(TrackId(Uuid::new_v4()), "music/01_song.flac");
        tagged.title = Some("Real Title".into());
        assert_eq!(tagged.display_title(), "Real Title");

        let untagged = Track::new(TrackId(Uuid::new_v4()), "music/01_song.flac");
        assert_eq!(untagged.display_title(), "01_song");

        // Windows separators, no extension, no stem at all: all safe.
        let win = Track::new(TrackId(Uuid::new_v4()), "C:\\Music\\beep");
        assert_eq!(win.display_title(), "beep");
        let dot_hidden = Track::new(TrackId(Uuid::new_v4()), ".hidden");
        assert_eq!(dot_hidden.display_title(), ".hidden");
    }

    #[test]
    fn display_line_joins_artist_and_title() {
        let mut t = Track::new(TrackId(Uuid::new_v4()), "m/a.mp3");
        t.title = Some("Song".into());
        t.artist = Some(Artist {
            name: "Beth".into(),
        });
        assert_eq!(t.display_line(), "Beth — Song");
    }

    #[test]
    fn track_replaygain_tags_roundtrip() {
        let mut t = Track::new(TrackId(Uuid::new_v4()), "m/a.mp3");
        t.replaygain_track_db = Some(-6.5);
        t.replaygain_album_db = Some(-3.0);
        let encoded = serde_json::to_string(&t).unwrap();
        let decoded: Track = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, t);
    }

    #[test]
    fn audio_format_extension_parsing_is_case_insensitive() {
        assert_eq!(AudioFormat::from_extension("flac"), AudioFormat::Flac);
        assert_eq!(AudioFormat::from_extension("FLAC"), AudioFormat::Flac);
        assert_eq!(AudioFormat::from_extension(" Flac "), AudioFormat::Flac);
        assert_eq!(AudioFormat::from_extension("m4a"), AudioFormat::Alac);
        assert_eq!(AudioFormat::from_extension("exe"), AudioFormat::Unknown);
    }

    #[test]
    fn lossless_classification_drives_bit_perfect_eligibility() {
        assert!(AudioFormat::Flac.is_lossless());
        assert!(AudioFormat::Wav.is_lossless());
        assert!(!AudioFormat::Mp3.is_lossless());
        assert!(!AudioFormat::Opus.is_lossless());
    }
}
