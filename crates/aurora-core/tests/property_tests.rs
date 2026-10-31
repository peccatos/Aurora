//! Property-based contracts of the domain core.
//!
//! These encode the invariants that later phases lean on:
//! - serialization round-trips never corrupt domain state (frontend/backend
//!   contract, ARCHITECTURE.md invariant 7);
//! - content-derived identity is path-independent and collision-resistant
//!   in practice (Phase 6 rescan idempotency);
//! - plausible PCM formats stay plausible after round-trips.

use aurora_core::{
    Album, Artist, AudioFormat, ChannelLayout, PcmFormat, SampleFormat, Track, TrackId,
};
use proptest::prelude::*;

fn arb_sample_format() -> impl Strategy<Value = SampleFormat> {
    prop_oneof![
        Just(SampleFormat::SignedInt),
        Just(SampleFormat::UnsignedInt),
        Just(SampleFormat::Float),
    ]
}

fn arb_channel_layout() -> impl Strategy<Value = ChannelLayout> {
    prop_oneof![
        Just(ChannelLayout::Interleaved),
        Just(ChannelLayout::Planar),
    ]
}

fn arb_audio_format() -> impl Strategy<Value = AudioFormat> {
    prop_oneof![
        Just(AudioFormat::Flac),
        Just(AudioFormat::Wav),
        Just(AudioFormat::Alac),
        Just(AudioFormat::Aiff),
        Just(AudioFormat::Mp3),
        Just(AudioFormat::Aac),
        Just(AudioFormat::OggVorbis),
        Just(AudioFormat::Opus),
        Just(AudioFormat::WavPack),
        Just(AudioFormat::Unknown),
    ]
}

fn arb_pcm_format() -> impl Strategy<Value = PcmFormat> {
    (
        (1u32..768_000),
        (1u16..64),
        (1u16..64),
        arb_sample_format(),
        arb_channel_layout(),
    )
        .prop_map(|(sample_rate, channels, bits, sf, cl)| PcmFormat {
            sample_rate,
            channels,
            bits_per_sample: bits,
            sample_format: sf,
            channel_layout: cl,
        })
}

proptest! {
    #[test]
    fn pcm_format_roundtrips_through_json(fmt in arb_pcm_format()) {
        let encoded = serde_json::to_string(&fmt).unwrap();
        let decoded: PcmFormat = serde_json::from_str(&encoded).unwrap();
        prop_assert_eq!(decoded, fmt);
    }

    #[test]
    fn track_roundtrips_through_json(
        digest in prop::collection::vec(any::<u8>(), 32),
        path in "music/.+/[a-z]{1,10}\\.(flac|mp3|wav)",
        title in proptest::option::of(".{0,40}"),
        artist_name in proptest::option::of(".{0,40}"),
        album_title in proptest::option::of(".{0,40}"),
        duration in proptest::option::of(0u64..360_000_000),
        fmt in arb_audio_format(),
    ) {
        let mut digest_arr = [0u8; 32];
        digest_arr.copy_from_slice(&digest);

        let mut track = Track::new(TrackId::from_content_digest(digest_arr), path);
        track.title = title;
        track.artist = artist_name.map(Artist::new);
        track.album = album_title.map(Album::new);
        track.duration_ms = duration;
        track.format = fmt;

        let encoded = serde_json::to_string(&track).unwrap();
        let decoded: Track = serde_json::from_str(&encoded).unwrap();
        prop_assert_eq!(decoded, track);
    }

    #[test]
    fn track_identity_is_path_independent(
        digest in prop::collection::vec(any::<u8>(), 32),
        path_a in "/a/[a-z]{1,8}.flac",
        path_b in "/deeply/nested/[a-z]{1,8}.flac",
    ) {
        let mut digest_arr = [0u8; 32];
        digest_arr.copy_from_slice(&digest);

        let a = Track::new(TrackId::from_content_digest(digest_arr), path_a);
        let b = Track::new(TrackId::from_content_digest(digest_arr), path_b);
        // Same content, different path: same identity.
        prop_assert_eq!(a.id, b.id);
        prop_assert_ne!(a.path, b.path);
    }

    #[test]
    fn track_identity_spread_is_wide(
        digests in prop::collection::vec(prop::collection::vec(any::<u8>(), 32), 64),
    ) {
        // 64 distinct digests should map to 64 distinct ids: a fold that
        // collapsed input space (see the XOR bug this guards against)
        // produces far fewer.
        let ids: std::collections::HashSet<TrackId> = digests
            .iter()
            .map(|d| {
                let mut arr = [0u8; 32];
                arr.copy_from_slice(d);
                TrackId::from_content_digest(arr)
            })
            .collect();
        prop_assert_eq!(ids.len(), digests.len());
    }

    #[test]
    fn losslessness_is_stable_under_format_roundtrip(fmt in arb_audio_format()) {
        let encoded = serde_json::to_string(&fmt).unwrap();
        let decoded: AudioFormat = serde_json::from_str(&encoded).unwrap();
        prop_assert_eq!(decoded.is_lossless(), fmt.is_lossless());
    }

    #[test]
    fn extension_parsing_roundtrips_known_formats(fmt in arb_audio_format(), case in 0u8..3) {
        // Only lossless containers have canonical extensions; unknown stays
        // unknown through parsing.
        let ext = match fmt {
            AudioFormat::Flac => "flac",
            AudioFormat::Wav => "wav",
            AudioFormat::Alac => "m4a",
            AudioFormat::Aiff => "aiff",
            AudioFormat::Mp3 => "mp3",
            AudioFormat::Aac => "aac",
            AudioFormat::OggVorbis => "ogg",
            AudioFormat::Opus => "opus",
            AudioFormat::WavPack => "wv",
            AudioFormat::Unknown => return Ok(()),
        };
        let cased = match case {
            0 => ext.to_string(),
            1 => ext.to_uppercase(),
            _ => format!(" {}", ext),
        };
        prop_assert_eq!(AudioFormat::from_extension(&cased), fmt);
    }
}
