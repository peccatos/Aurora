//! Conversion tests: the fallback playback path's wire-format conversion.

use aurora_core::{ChannelLayout, PcmFormat, SampleFormat};
use aurora_decoder::PcmChunk;

fn s16_stereo() -> PcmFormat {
    PcmFormat {
        sample_rate: 44_100,
        channels: 2,
        bits_per_sample: 16,
        sample_format: SampleFormat::SignedInt,
        channel_layout: ChannelLayout::Interleaved,
    }
}

fn f32_stereo() -> PcmFormat {
    PcmFormat {
        sample_rate: 44_100,
        channels: 2,
        bits_per_sample: 32,
        sample_format: SampleFormat::Float,
        channel_layout: ChannelLayout::Interleaved,
    }
}

#[test]
fn s16_converts_to_f32_wire_bytes_exactly() {
    let data: Vec<u8> = i16::MIN
        .to_le_bytes()
        .iter()
        .chain(0i16.to_le_bytes().iter())
        .chain(i16::MAX.to_le_bytes().iter())
        .chain((i16::MAX / 2).to_le_bytes().iter())
        .copied()
        .collect();

    let chunk = PcmChunk::from_bytes(s16_stereo(), data).expect("chunk");
    let bytes = chunk.convert_to(f32_stereo()).expect("convert");

    assert_eq!(bytes.len(), 4 * 4);
    let mut floats = Vec::new();
    for pair in bytes.as_chunks::<4>().0 {
        floats.push(f32::from_le_bytes(*pair));
    }
    assert!((floats[0] + 1.0).abs() < 1e-6);
    assert_eq!(floats[1], 0.0);
    assert!((floats[2] - 1.0).abs() < 1e-4);
    assert!((floats[3] - 0.5).abs() < 1e-3);
}

#[test]
fn conversion_to_same_format_is_identity() {
    let data: Vec<u8> = vec![0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
    let chunk = PcmChunk::from_bytes(s16_stereo(), data.clone()).expect("chunk");
    let out = chunk.convert_to(s16_stereo()).expect("convert");
    assert_eq!(out, data);
}

#[test]
fn mono_duplicated_to_stereo() {
    let mono = PcmFormat {
        channels: 1,
        ..s16_stereo()
    };
    let data = i16::MAX.to_le_bytes().to_vec();
    let chunk = PcmChunk::from_bytes(mono, data).expect("chunk");

    let stereo_out = chunk.convert_to(s16_stereo()).expect("convert");
    assert_eq!(stereo_out.len(), 4);
    assert_eq!(&stereo_out[..2], &i16::MAX.to_le_bytes());
    assert_eq!(&stereo_out[2..], &i16::MAX.to_le_bytes());
}

#[test]
fn stereo_folded_to_mono() {
    let data: Vec<u8> = (i16::MAX / 4)
        .to_le_bytes()
        .iter()
        .chain((i16::MAX / 4).to_le_bytes().iter())
        .copied()
        .collect();
    let chunk = PcmChunk::from_bytes(s16_stereo(), data).expect("chunk");

    let mono = PcmFormat {
        channels: 1,
        ..s16_stereo()
    };
    let out = chunk.convert_to(mono).expect("convert");
    assert_eq!(out.len(), 2);
    let v = i16::from_le_bytes([out[0], out[1]]);
    assert!((v - i16::MAX / 4).abs() < 100, "channel fold: {v}");
}

#[test]
fn u8_silence_is_midpoint_not_zero() {
    let u8f = PcmFormat {
        channels: 1,
        bits_per_sample: 8,
        sample_format: SampleFormat::UnsignedInt,
        channel_layout: ChannelLayout::Interleaved,
        sample_rate: 44_100,
    };
    let silence = PcmChunk::silence(u8f, 3).expect("silence");
    assert_eq!(silence.as_bytes(), &[0x80, 0x80, 0x80]);
}
