//! End-to-end decoder tests against generated WAV fixtures (hound).
//!
//! Phase 2 exit criteria: fixtures decode with correct duration, rate,
//! channels and sample count; PCM content survives the round-trip.

use std::path::Path;

use aurora_core::{ChannelLayout, PcmFormat, SampleFormat};
use aurora_decoder::{AudioSource, DecodeError, PcmChunk, SourceError};

/// Write a WAV fixture with a known signal: `frames` frames of a sine wave.
fn write_sine_wav(path: &Path, spec: hound::WavSpec, frames: usize, freq: f64) -> Vec<i16> {
    let mut writer = hound::WavWriter::create(path, spec).expect("create wav");
    let mut samples = Vec::with_capacity(frames * spec.channels as usize);
    for frame in 0..frames {
        for ch in 0..spec.channels as usize {
            // Distinct phase per channel keeps channels distinguishable.
            let phase = (frame as f64) * freq / spec.sample_rate as f64
                + (ch as f64) * std::f64::consts::FRAC_PI_2;
            let s = (phase.sin() * 20000.0).round() as i16;
            samples.push(s);
            writer.write_sample(s).expect("write sample");
        }
    }
    writer.finalize().expect("finalize wav");
    samples
}

fn spec_44k_stereo() -> hound::WavSpec {
    hound::WavSpec {
        channels: 2,
        sample_rate: 44_100,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    }
}

#[test]
fn wav_decodes_with_correct_stream_parameters() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("sine.wav");
    let frames = 44_100; // exactly 1 second
    write_sine_wav(&path, spec_44k_stereo(), frames, 440.0);

    let source = AudioSource::open(&path).expect("open source");
    let info = source.info();

    assert_eq!(info.pcm_format.sample_rate, 44_100);
    assert_eq!(info.pcm_format.channels, 2);
    assert_eq!(info.pcm_format.bits_per_sample, 16);
    assert_eq!(info.pcm_format.sample_format, SampleFormat::SignedInt);
    assert_eq!(info.pcm_format.channel_layout, ChannelLayout::Interleaved);
    assert_eq!(info.audio_format, aurora_core::AudioFormat::Wav);

    // Container-declared duration: exactly 1000 ms.
    assert_eq!(info.duration_ms, Some(1000));
    assert_eq!(info.total_frames, Some(frames as u64));
}

#[test]
fn wav_pcm_content_survives_roundtrip_bit_exact() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("sine.wav");
    let frames = 10_000;
    let written = write_sine_wav(&path, spec_44k_stereo(), frames, 440.0);

    let mut source = AudioSource::open(&path).expect("open source");
    let chunks = source.decode_all(10 * 1024 * 1024).expect("decode all");

    // Rebuild the full interleaved i16 stream from decoded chunks.
    let mut decoded_samples: Vec<i16> = Vec::with_capacity(written.len());
    for chunk in &chunks {
        assert_eq!(chunk.format().sample_format, SampleFormat::SignedInt);
        let bytes = chunk.as_bytes();
        for pair in bytes.as_chunks::<2>().0 {
            decoded_samples.push(i16::from_le_bytes([pair[0], pair[1]]));
        }
    }

    assert_eq!(decoded_samples, written, "PCM must round-trip bit-exact");
    let total_frames: usize = chunks.iter().map(PcmChunk::frames).sum();
    assert_eq!(total_frames, frames);
}

#[test]
fn wav_mono_decodes_with_one_channel() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("mono.wav");
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    write_sine_wav(&path, spec, 4800, 1000.0);

    let source = AudioSource::open(&path).expect("open source");
    assert_eq!(source.pcm_format().channels, 1);
    assert_eq!(source.pcm_format().sample_rate, 48_000);
}

#[test]
fn truncated_file_reports_recoverable_behavior_not_hang() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("sine.wav");
    write_sine_wav(&path, spec_44k_stereo(), 44_100, 440.0);

    // Truncate the file mid-stream: copy first 30% of bytes only.
    let full = std::fs::read(&path).expect("read");
    let cut = full.len() * 3 / 10;
    let truncated_path = dir.path().join("truncated.wav");
    std::fs::write(&truncated_path, &full[..cut]).expect("write truncated");

    // Either probing fails (clean SourceError) or decoding terminates
    // within a bounded number of steps — never hangs, never panics.
    match AudioSource::open(&truncated_path) {
        Err(SourceError::UnsupportedFormat) => { /* clean rejection: fine */ }
        Ok(mut source) => {
            let mut steps = 0;
            loop {
                steps += 1;
                assert!(steps < 10_000, "decoding truncated file must terminate");
                match source.next_chunk() {
                    Ok(_) => continue,
                    Err(DecodeError::EndOfStream) => break,
                    Err(DecodeError::Recoverable(_)) => continue,
                    Err(DecodeError::Unrecoverable(_)) => break,
                    Err(other) => panic!("unexpected error: {other:?}"),
                }
            }
        }
        Err(other) => panic!("unexpected source error: {other:?}"),
    }
}

#[test]
fn garbage_file_is_rejected_cleanly() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("garbage.wav");
    std::fs::write(&path, b"this is not audio data at all, just noise bytes").expect("write");

    let result = AudioSource::open(&path);
    assert!(
        matches!(
            result,
            Err(SourceError::UnsupportedFormat) | Err(SourceError::NoAudioTrack)
        ),
        "garbage must be rejected as unsupported"
    );
}

#[test]
fn pcm_chunk_f32_conversion_is_symmetric_for_s16() {
    // The DSP path conversion: s16 -> f32 must be exact at the 16-bit grid.
    let format = PcmFormat {
        sample_rate: 44_100,
        channels: 1,
        bits_per_sample: 16,
        sample_format: SampleFormat::SignedInt,
        channel_layout: ChannelLayout::Interleaved,
    };
    // One frame, min/zero/max samples.
    let data: Vec<u8> = i16::MIN
        .to_le_bytes()
        .iter()
        .chain(0i16.to_le_bytes().iter())
        .chain(i16::MAX.to_le_bytes().iter())
        .copied()
        .collect();
    let chunk = PcmChunk::from_bytes(format, data).expect("chunk");

    let f32s = chunk.to_f32_interleaved();
    assert_eq!(f32s.len(), 3);
    assert!((f32s[0] + 1.0).abs() < 1e-6, "i16::MIN maps to -1.0");
    assert_eq!(f32s[1], 0.0);
    assert!(
        f32s[2] < 1.0 && f32s[2] > 0.9999,
        "i16::MAX maps just below +1.0"
    );
}
