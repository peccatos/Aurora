//! Phase 8 performance tests: end-to-end DSP throughput (decode shape ->
//! pipeline -> wire bytes), the exact per-chunk work the decode worker
//! performs on the real path.
//!
//! These are regression guards, not benchmarks: they fail loudly if the
//! pipeline ever becomes slower than real-time by an order of magnitude.
//! Thresholds are deliberately generous (10x real-time floor) so CI noise
//! and cold caches do not flake; a real regression falls far below.

use std::time::Instant;

use aurora_core::{PcmFormat, SampleFormat};
use aurora_dsp::{DspConfig, DspPipeline, TpdfDither};

/// The per-chunk transformation the engine worker runs on the DSP path.
fn process_chunk(
    pipeline: &mut DspPipeline,
    ditherer: &mut TpdfDither,
    samples: &mut Vec<f32>,
    target: PcmFormat,
) -> Vec<u8> {
    let processed = pipeline.process(samples);
    let mut dithered = processed;
    if !matches!(target.sample_format, SampleFormat::Float) {
        ditherer.dither_for(&mut dithered, target.sample_format, target.bits_per_sample);
    }
    aurora_decoder::PcmChunk::from_bytes(target, encode_wire(&dithered, target))
        .expect("wire conversion")
        .as_bytes()
        .to_vec()
}

/// f32 interleaved -> target wire bytes (same math the engine uses).
fn encode_wire(samples: &[f32], target: PcmFormat) -> Vec<u8> {
    let bytes = usize::from(target.bits_per_sample).div_ceil(8);
    let mut out = Vec::with_capacity(samples.len() * bytes);
    for &s in samples {
        match (target.sample_format, target.bits_per_sample) {
            (SampleFormat::SignedInt, 16) => {
                let v = (s.clamp(-1.0, 1.0) * 32_767.0) as i16;
                out.extend_from_slice(&v.to_le_bytes());
            }
            (SampleFormat::SignedInt, 24) => {
                let v = (s.clamp(-1.0, 1.0) * 8_388_607.0) as i32;
                out.extend_from_slice(&v.to_le_bytes()[..3]);
            }
            (SampleFormat::SignedInt, 32) => {
                let v = (s.clamp(-1.0, 1.0) * 2_147_483_647.0) as i32;
                out.extend_from_slice(&v.to_le_bytes());
            }
            (SampleFormat::Float, 32) => out.extend_from_slice(&s.to_le_bytes()),
            _ => unreachable!("test target format"),
        }
    }
    out
}

fn flac_44k_24() -> PcmFormat {
    PcmFormat {
        sample_rate: 44_100,
        channels: 2,
        bits_per_sample: 24,
        sample_format: SampleFormat::SignedInt,
        channel_layout: aurora_core::ChannelLayout::Interleaved,
    }
}

/// seconds of audio must process in at most seconds/10 wall time.
const REALTIME_FLOOR: f64 = 10.0;

fn assert_realtime(source: PcmFormat, label: &str, audio_seconds: f64) {
    let chunk_frames = 5_880; // ~133ms of 44.1k stereo, engine-scale chunk
    let total_frames = (audio_seconds * f64::from(source.sample_rate)) as usize;
    let chunks = total_frames / chunk_frames + 1;

    // Everything on: EQ + reverb + gain + replaygain + dither; worst case.
    let mut config = DspConfig::default();
    config.eq.bands[0].gain_db = 3.0;
    config.reverb.wet = 0.3;
    config.volume_db = -2.0;
    config.replaygain = aurora_dsp::ReplayGainModeDef::Track;
    let target = PcmFormat {
        sample_rate: source.sample_rate,
        bits_per_sample: 24,
        sample_format: SampleFormat::SignedInt,
        ..source
    };

    let mut pipeline = DspPipeline::compile(&config, source, None, Some(-6.5)).expect("compile");
    let mut ditherer = TpdfDither::new(0xA10CA);

    // process() consumes the input buffer (mem::take): every chunk gets a
    // fresh synthetic decode — exactly what the worker sees per chunk.
    let make_chunk = || -> Vec<f32> {
        (0..chunk_frames * 2)
            .map(|i| ((i % 512) as f64 * 0.01).sin() as f32)
            .collect()
    };

    let start = Instant::now();
    let mut processed_bytes = 0usize;
    for _ in 0..chunks {
        let mut samples = make_chunk();
        processed_bytes += process_chunk(&mut pipeline, &mut ditherer, &mut samples, target).len();
    }
    let elapsed = start.elapsed().as_secs_f64();

    let floor_secs = audio_seconds / REALTIME_FLOOR;
    assert!(
        elapsed <= floor_secs,
        "{label}: DSP path processed {audio_seconds:.1}s of audio in {elapsed:.3}s \
         (floor {floor_secs:.3}s = {REALTIME_FLOOR}x realtime); regression in the pipeline"
    );
    // Sanity: output exists and is roughly the expected size.
    let expected = total_frames * 2 * 3;
    assert!(
        processed_bytes > expected / 2,
        "{label}: implausible output volume ({processed_bytes} bytes for {expected} expected)"
    );
}

#[test]
fn dsp_path_meets_realtime_floor_60s_44k() {
    assert_realtime(flac_44k_24(), "44.1k/24 stereo", 60.0);
}

#[test]
fn dsp_path_meets_realtime_floor_60s_96k() {
    let hi = PcmFormat {
        sample_rate: 96_000,
        ..flac_44k_24()
    };
    assert_realtime(hi, "96k/24 stereo", 60.0);
}

/// Bypassed pipeline must be (nearly) free — the bit-perfect path's
/// headroom guarantee: identity processing may never become a tax.
#[test]
fn bypassed_pipeline_is_cheap() {
    let source = flac_44k_24();
    let target = source;
    let mut pipeline =
        DspPipeline::compile(&DspConfig::default(), source, None, None).expect("compile");
    let mut ditherer = TpdfDither::new(0xA10CA);
    let make_chunk = || -> Vec<f32> {
        (0..11_760 * 2)
            .map(|i| (i as f64 * 0.01).sin() as f32)
            .collect()
    };

    let start = Instant::now();
    for _ in 0..100 {
        let mut samples = make_chunk();
        let _ = process_chunk(&mut pipeline, &mut ditherer, &mut samples, target);
    }
    let elapsed = start.elapsed().as_secs_f64();
    // 100 chunks x 266ms = 26.6s of audio; even 20x realtime is a warning.
    assert!(
        elapsed < 26.6 / 20.0,
        "bypassed pipeline took {elapsed:.3}s for 26.6s of audio"
    );
}
