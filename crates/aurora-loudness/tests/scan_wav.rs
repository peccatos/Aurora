//! End-to-end loudness scan against generated WAV fixtures (hound):
//! decode -> K-weight -> gate -> LUFS -> ReplayGain 2.0 gain.

use std::path::Path;

use aurora_decoder::AudioSource;
use aurora_loudness::{RG2_TARGET_LUFS, integrated_lufs, replaygain2_db};

fn write_sine_wav(path: &Path, amp: f64, secs: f64) {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 44_100,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).expect("create wav");
    let frames = (secs * 44_100.0) as usize;
    for i in 0..frames {
        // Same phase per channel: a plain stereo sine.
        let v = (amp * (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / 44_100.0).sin() * 32_000.0)
            as i16;
        writer.write_sample(v).expect("write l");
        writer.write_sample(v).expect("write r");
    }
    writer.finalize().expect("finalize");
}

#[test]
fn wav_scan_roundtrip_matches_direct_measurement() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("sine.wav");
    write_sine_wav(&path, 0.5, 3.0);

    let mut source = AudioSource::open(&path).expect("open source");
    let scan = aurora_loudness::analyze_source(&mut source, 60.0).expect("scan");
    assert_eq!(scan.sample_rate, 44_100);
    assert_eq!(scan.channels, 2);

    // The scanner must equal a direct measurement of the same decoded
    // samples (one algorithm, no drift between paths).
    let mut source = AudioSource::open(&path).expect("reopen");
    let mut decoded = Vec::new();
    loop {
        match source.next_chunk() {
            Ok(chunk) => decoded.extend(chunk.to_f32_interleaved()),
            Err(aurora_decoder::DecodeError::EndOfStream) => break,
            Err(e) => panic!("decode: {e}"),
        }
    }
    let direct = integrated_lufs(&decoded, 2, 44_100).expect("direct measurable");
    let measured = scan.integrated_lufs.expect("scan measurable");
    assert!(
        (measured - direct).abs() < 1e-9,
        "scan {measured} vs direct {direct}"
    );

    // Gain math: a -6.27-ish LUFS track at the rg2 target -18 gets a
    // strongly positive gain; at -14 a smaller one.
    let rg2 = replaygain2_db(scan.integrated_lufs, RG2_TARGET_LUFS).unwrap();
    assert!((rg2 - (-18.0 - measured)).abs() < 1e-9);
    // 0.5-amp stereo sine ~-6.4 LUFS -> rg2 gain ~-11.5 dB (a LOUD track
    // relative to the -18 reference; negative gain = attenuation).
    assert!(rg2 > -12.5 && rg2 < -10.5, "unexpected rg2 gain {rg2}");
}

#[test]
fn wav_scan_silence_reports_none() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("silence.wav");
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 44_100,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&path, spec).expect("create wav");
    for _ in 0..44_100 * 2 {
        writer.write_sample(0i16).expect("l");
        writer.write_sample(0i16).expect("r");
    }
    writer.finalize().expect("finalize");

    let mut source = AudioSource::open(&path).expect("open source");
    let scan = aurora_loudness::analyze_source(&mut source, 60.0).expect("scan");
    assert!(
        scan.integrated_lufs.is_none(),
        "silence must measure None, got {:?}",
        scan.integrated_lufs
    );
    // None loudness -> None gain (honest no-normalization).
    assert!(replaygain2_db(scan.integrated_lufs, RG2_TARGET_LUFS).is_none());
}
