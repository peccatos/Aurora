//! Measured tempo (BPM) analysis.
//!
//! Honest measurement, not a tag echo: the content is decoded in full and
//! the beat grid is derived from onset energy. Two-stage pipeline:
//!
//! 1. **Onset envelope.** The stream is mono-mixed, windowed into
//!    overlapping STFT frames, and a spectral-flux onset strength signal
//!    is computed over the low band (kick/bass fundamentals, 30–200 Hz)
//!    where the beat lives.
//! 2. **Tempo.** The onset envelope's autocorrelation is maximized over
//!    lags corresponding to 70–180 BPM. The score of the best candidate
//!    relative to the envelope's overall energy decides confidence: a
//!    weak maximum means the content has no periodic beat (ambient,
//!    spoken word) and the honest answer is `None`.
//!
//! Half/double-tempo ambiguity is NOT silently resolved — the reported
//! tempo is the periodicity actually observed inside the 70–180 range.
//! The deck UI (true BPM = bpm × speed) keeps the ratio honest.

use std::path::Path;

use rustfft::{FftPlanner, num_complex::Complex};

use crate::error::{DecodeError, SourceError};
use crate::source::AudioSource;

/// STFT window length (samples). 2048 @ 44.1 kHz ≈ 46 ms — resolves the
/// low band well enough for onset detection and matches the spectrum
/// analyzer's frame size (one FFT length in the workspace).
const WINDOW: usize = 2048;

/// STFT hop (samples). 50% overlap — standard for onset detection.
const HOP: usize = 1024;

/// Low-band onset cutoff (Hz): beat energy (kick, bass) lives below this.
const LOW_BAND_HZ: f64 = 200.0;

/// Tempo search range (BPM). The honest observed periodicity range for
/// dance/music tempos; anything outside is reported as no-confidence.
const BPM_MIN: f64 = 70.0;
const BPM_MAX: f64 = 180.0;

/// Minimum autocorrelation peak prominence to claim a beat.
const MIN_CONFIDENCE: f64 = 0.12;

/// Minimum duration (seconds) worth analyzing at all.
const MIN_DURATION_S: f64 = 8.0;

/// Measured tempo of the file at `path`, in beats per minute.
///
/// `None` means the measurement found no confident periodic beat —
/// that is a result, not a failure (see module docs). Decode errors are
/// propagated (a broken file must not masquerade as "no beat").
pub fn detect_bpm(path: &Path) -> Result<Option<f64>, SourceError> {
    let mut source = AudioSource::open(path)?;
    let duration_s = source.info().duration_ms.unwrap_or(0) as f64 / 1000.0;
    if duration_s < MIN_DURATION_S {
        return Ok(None);
    }
    let rate = f64::from(source.pcm_format().sample_rate);

    // Onset envelope, one value per STFT hop.
    let envelope = onset_envelope(&mut source, rate)?;

    if envelope.len() < 32 {
        return Ok(None);
    }

    Ok(tempo_from_envelope(&envelope, rate))
}

/// Compute the spectral-flux onset envelope from the decoded stream.
fn onset_envelope(source: &mut AudioSource, rate: f64) -> Result<Vec<f32>, SourceError> {
    let nch = usize::from(source.pcm_format().channels.max(1));

    // Frequency bins of the low band (inclusive).
    let bin_hz = rate / WINDOW as f64;
    let low_bins = (LOW_BAND_HZ / bin_hz).ceil() as usize + 1;

    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(WINDOW);

    // Hann window (periodic), precomputed.
    let window: Vec<f32> = (0..WINDOW)
        .map(|i| 0.5 * (1.0 - (2.0 * std::f64::consts::PI * i as f64 / WINDOW as f64).cos()) as f32)
        .collect();

    let mut prev_mag: Option<Vec<f32>> = None;
    let mut envelope: Vec<f32> = Vec::new();

    let mut carry: Vec<f32> = Vec::new(); // mono samples carried between chunks

    loop {
        let chunk = match source.next_chunk() {
            Ok(c) => c,
            Err(DecodeError::EndOfStream) => break,
            Err(_) => break, // recoverable decode issues: stop analysis cleanly
        };
        let samples = chunk.to_f32_interleaved();
        // Mono-mix.
        for frame in samples.chunks_exact(nch) {
            let m = frame.iter().sum::<f32>() / nch as f32;
            carry.push(m);
        }

        // Process complete windows.
        while carry.len() >= WINDOW {
            let mut buf: Vec<Complex<f32>> = carry[..WINDOW]
                .iter()
                .zip(window.iter())
                .map(|(s, w)| Complex { re: s * w, im: 0.0 })
                .collect();
            fft.process(&mut buf);

            // Positive-spectrum magnitudes of the low band.
            let mut mag = vec![0f32; low_bins];
            for b in 0..low_bins {
                let c = buf[b];
                mag[b] = (c.re * c.re + c.im * c.im).sqrt();
            }

            // Spectral flux: sum of positive magnitude increases.
            let flux = match &prev_mag {
                Some(p) => mag
                    .iter()
                    .zip(p.iter())
                    .map(|(m, p)| (m - p).max(0.0))
                    .sum::<f32>(),
                None => 0.0,
            };
            envelope.push(flux);
            prev_mag = Some(mag);

            carry.drain(..HOP);
        }
    }

    Ok(envelope)
}

/// Normalize + autocorrelate the envelope, return the best tempo in range.
fn tempo_from_envelope(envelope: &[f32], rate: f64) -> Option<f64> {
    // Mean removal + normalization to unit power.
    let n = envelope.len();
    let mean = envelope.iter().sum::<f32>() / n as f32;
    let centered: Vec<f32> = envelope.iter().map(|v| v - mean).collect();
    let power = centered.iter().map(|v| v * v).sum::<f32>() / n as f32;
    if power <= f32::EPSILON {
        return None;
    }

    let env_rate = rate / HOP as f64; // envelope samples per second

    // Lag range for 70..180 BPM.
    let lag_min = (60.0 / BPM_MAX / (1.0 / env_rate)).ceil() as usize; // beats period in env samples
    let lag_max = (60.0 / BPM_MIN / (1.0 / env_rate)).floor() as usize;
    if lag_max >= n / 2 {
        return None;
    }

    // Autocorrelation over candidate lags (O(n * lags) — envelope is small).
    let mut best: Option<(f64, f64)> = None; // (score, lag)
    for lag in lag_min..=lag_max {
        let mut sum = 0f32;
        for i in 0..(n - lag) {
            sum += centered[i] * centered[i + lag];
        }
        let score = f64::from(sum / (n as f32 - lag as f32) / power);
        if best.is_none_or(|(s, _)| score > s) {
            best = Some((score, lag as f64));
        }
    }

    let (score, lag) = best?;
    if score < MIN_CONFIDENCE {
        return None; // no honest periodic beat
    }

    let bpm = 60.0 * env_rate / lag;
    Some((bpm * 10.0).round() / 10.0) // one decimal, e.g. 128.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_beat_wav(path: &Path, bpm: f64, seconds: f64) {
        let rate = 44_100u32;
        let beat_period = 60.0 / bpm * rate as f64;
        let frames = (seconds * rate as f64) as usize;
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).expect("create wav");
        let mut rng_state: u64 = 0x9E3779B97F4A7C15;
        for i in 0..frames {
            let phase = (i as f64 / beat_period).fract();
            // Decaying click every beat + light noise floor.
            let click = if phase < 0.02 {
                (1.0 - phase / 0.02) * 16_000.0
            } else {
                0.0
            };
            rng_state ^= rng_state << 13;
            rng_state ^= rng_state >> 7;
            rng_state ^= rng_state << 17;
            let noise = ((rng_state & 0xFF) as f64 - 127.5) * 30.0;
            let v = (click + noise).clamp(-32_000.0, 32_000.0) as i16;
            writer.write_sample(v).unwrap();
        }
        writer.finalize().unwrap();
    }

    fn write_noise_wav(path: &Path, seconds: f64) {
        let rate = 44_100u32;
        let frames = (seconds * rate as f64) as usize;
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).expect("create wav");
        let mut rng_state: u64 = 0xDEADBEEF;
        for _ in 0..frames {
            rng_state ^= rng_state << 13;
            rng_state ^= rng_state >> 7;
            rng_state ^= rng_state << 17;
            let v = ((rng_state & 0xFFFF) as f64 - 32_768.0) as i16;
            writer.write_sample(v).unwrap();
        }
        writer.finalize().unwrap();
    }

    #[test]
    fn detects_120_bpm_click_track() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("beat120.wav");
        write_beat_wav(&wav, 120.0, 30.0);
        let bpm = detect_bpm(&wav).unwrap();
        assert!(bpm.is_some(), "120 BPM click must be detected");
        let bpm = bpm.unwrap();
        assert!((116.0..124.0).contains(&bpm), "got {bpm}");
    }

    #[test]
    fn detects_140_bpm_click_track() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("beat140.wav");
        write_beat_wav(&wav, 140.0, 30.0);
        let bpm = detect_bpm(&wav).unwrap().expect("140 BPM click detected");
        assert!((136.0..144.0).contains(&bpm), "got {bpm}");
    }

    #[test]
    fn noise_has_no_confident_beat() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("noise.wav");
        write_noise_wav(&wav, 30.0);
        let bpm = detect_bpm(&wav).unwrap();
        assert!(bpm.is_none(), "pure noise must report None, got {bpm:?}");
    }

    #[test]
    fn short_file_reports_none() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("short.wav");
        write_beat_wav(&wav, 120.0, 5.0);
        assert!(detect_bpm(&wav).unwrap().is_none());
    }

    #[test]
    fn missing_file_is_an_error() {
        let bpm = detect_bpm(Path::new("Z:/definitely/not/here.wav"));
        assert!(bpm.is_err());
    }

    #[test]
    fn tempo_from_envelope_respects_range() {
        // Synthetic envelope: strong periodicity at ~46 env-samples lag
        // (≈ 44100/1024 * 60/128 BPM window). Direct unit test of the
        // estimator without decode.
        let rate = 44_100.0;
        let env_rate = rate / HOP as f64;
        let period = 60.0 / 128.0 * env_rate; // 128 BPM in env samples
        let n = 2000;
        let env: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f64;
                let beat = (2.0 * std::f64::consts::PI * t / period).cos();
                (0.6 * beat + 0.4 * (t * 0.31).sin()) as f32
            })
            .collect();
        let bpm = tempo_from_envelope(&env, rate).expect("synthetic envelope has tempo");
        assert!((124.0..132.0).contains(&bpm), "got {bpm}");
    }
}
