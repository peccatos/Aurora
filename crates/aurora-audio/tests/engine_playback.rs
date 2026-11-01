//! Engine integration tests: real device playback.
//!
//! Phase 3 exit criteria: playback runs, state transitions work, underrun
//! metrics are observable, device enumeration never panics.
//!
//! These tests require an audio device (CI runners have a null/dummy
//! device; local machines have real ones) and are skipped gracefully when
//! no device exists.

use std::path::PathBuf;
use std::time::Duration;

use aurora_audio::{EngineState, PlaybackEngine};
use aurora_decoder::AudioSource;

fn write_test_wav(path: &PathBuf, seconds: f64) {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 44_100,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).expect("create wav");
    let frames = (seconds * 44_100.0) as usize;
    for frame in 0..frames {
        for ch in 0..2u32 {
            let phase =
                frame as f64 * 440.0 / 44_100.0 + f64::from(ch) * std::f64::consts::FRAC_PI_2;
            let s = (phase.sin() * 15_000.0) as i16;
            writer.write_sample(s).expect("write");
        }
    }
    writer.finalize().expect("finalize");
}

#[test]
fn device_enumeration_never_panics() {
    // Enumerating on a machine with no devices returns a typed error, not
    // a panic (invariant 8: device loss must not kill the process).
    match aurora_audio::output_devices() {
        Ok(devices) => {
            assert!(!devices.is_empty());
            // Keys are stable and unique.
            let mut keys: Vec<_> = devices.iter().map(|d| d.key().to_string()).collect();
            keys.sort();
            keys.dedup();
            assert_eq!(keys.len(), devices.len());
        }
        Err(aurora_audio::EngineError::NoOutputDevices) => { /* acceptable */ }
        Err(e) => panic!("unexpected enumeration error: {e}"),
    }
}

#[test]
fn engine_plays_file_and_reports_state() {
    let device = match aurora_audio::default_output_device() {
        Ok(d) => d,
        Err(_) => {
            eprintln!("SKIP: no output device available");
            return;
        }
    };

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("sine.wav");
    write_test_wav(&path, 1.0);

    // The file must be decodable first (sanity).
    let probe = AudioSource::open(&path).expect("source opens");
    assert!(probe.info().duration_ms.unwrap_or(0) > 900);

    let mut engine = PlaybackEngine::new();
    assert_eq!(engine.state(), EngineState::Idle);

    engine
        .play(path.clone(), &device, Default::default())
        .expect("playback starts");

    assert_eq!(engine.state(), EngineState::Playing);
    // A negotiated output format is exposed for diagnostics.
    let out = engine.output_format().expect("output format");
    assert!(out.sample_rate > 0);
    assert!(out.channels > 0);

    // Let it play for a while: frames must advance.
    std::thread::sleep(Duration::from_millis(700));
    let frames1 = engine.metrics().frames_played();
    assert!(
        frames1 > 0,
        "callback must emit frames (got {frames1}); underruns: {}",
        engine.metrics().underruns()
    );

    // Pause / resume transitions.
    engine.pause().expect("pause");
    assert_eq!(engine.state(), EngineState::Paused);
    engine.resume().expect("resume");
    assert_eq!(engine.state(), EngineState::Playing);

    engine.stop();
    assert_eq!(engine.state(), EngineState::Idle);

    // Restart on the same engine instance (state machine reset).
    engine
        .play(path, &device, Default::default())
        .expect("second play");
    std::thread::sleep(Duration::from_millis(100));
    engine.stop();

    // Underruns during a 1s local-file playback should be minimal or zero:
    // the decoder is far faster than realtime.
    let underruns = engine.metrics().underruns();
    assert!(
        underruns < 100_000, // generous bound; real number is 0..~thousands
        "excessive underruns: {underruns}"
    );
}

#[test]
fn double_play_restarts_the_stream_while_playing() {
    let device = match aurora_audio::default_output_device() {
        Ok(d) => d,
        Err(_) => {
            eprintln!("SKIP: no output device available");
            return;
        }
    };

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("sine.wav");
    write_test_wav(&path, 2.0);

    let mut engine = PlaybackEngine::new();
    engine
        .play(path.clone(), &device, Default::default())
        .expect("first play");
    // Restart semantics: re-play while playing switches the stream
    // (mode switch, e.g. shared -> exclusive), never errors out.
    engine
        .play(path, &device, Default::default())
        .expect("second play restarts");
    assert_eq!(engine.state(), EngineState::Playing);
    engine.stop();
}
