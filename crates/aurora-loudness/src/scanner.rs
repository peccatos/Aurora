//! BS.1770-4 integrated loudness scanner + file-level scan entry points.
//!
//! K-weighting: the spec defines exact biquad coefficients at 48 kHz; at
//! every other rate the coefficients are computed analytically (the
//! spec derivation, the same construction libebur128 uses), so the
//! measurement is valid at any Aurora source rate.
//!
//! Gating: 400 ms blocks, 100 ms hop, absolute gate -70 LUFS, relative
//! gate -10 LU below the absolute-gated mean (BS.1770-4 / EBU R128).
//!
//! Channel weights: front pair 1.0, Ls/Rs 1.41 (BS.1770-4).

use aurora_core::PcmFormat;
use aurora_decoder::{AudioSource, DecodeError, SourceError};

/// One honest loudness measurement of a decoded stream.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LoudnessScan {
    /// Integrated loudness in LUFS (BS.1770-4, gated).
    pub integrated_lufs: Option<f64>,
    /// The stream the measurement was made on (rate/channels as decoded).
    pub sample_rate: u32,
    pub channels: u16,
}

/// Integrated loudness (LUFS) of interleaved f32 PCM.
///
/// `None` when the audio is empty, shorter than one 400 ms gating block,
/// or entirely gated out (silence) - an honest result, never
/// a fabricated number.
pub fn integrated_lufs(samples: &[f32], channels: usize, sample_rate: u32) -> Option<f64> {
    if channels == 0 || samples.is_empty() || sample_rate == 0 {
        return None;
    }
    let nch = channels.max(1);
    let frames = samples.len() / nch;
    let block_len = (0.4 * f64::from(sample_rate)).round() as usize;
    if frames < block_len || block_len == 0 {
        return None; // shorter than one gating block
    }

    let rate = f64::from(sample_rate);
    let stage1 = k_weight_stage1(rate);
    let stage2 = k_weight_stage2(rate);
    let mut z1: Vec<[f64; 2]> = vec![[0.0; 2]; nch];
    let mut z2: Vec<[f64; 2]> = vec![[0.0; 2]; nch];

    let hop = (0.1 * rate).round() as usize;
    let to_lufs = |p: f64| -0.691 + 10.0 * p.log10();

    // K-weighted mean-square per 400 ms block, 100 ms hops.
    let mut block_powers: Vec<f64> = Vec::new();
    let mut start = 0usize;
    while start + block_len <= frames {
        let mut power = 0f64;
        for ch in 0..nch {
            // BS.1770 channel weights: front pair 1.0, surround 1.41.
            let weight = if nch <= 2 || ch < 2 { 1.0 } else { 1.41 };
            let mut ch_power = 0f64;
            let (c1, z1s) = (&stage1, &mut z1[ch]);
            let (c2, z2s) = (&stage2, &mut z2[ch]);
            for fr in start..start + block_len {
                let x = f64::from(samples[fr * nch + ch]);
                // Stage 1: high shelf (pre-filter).
                let y = c1[0] * x + z1s[0];
                z1s[0] = c1[1] * x - c1[3] * y + z1s[1];
                z1s[1] = c1[2] * x - c1[4] * y;
                // Stage 2: 38 Hz high-pass (RLB weighting).
                let y2 = c2[0] * y + z2s[0];
                z2s[0] = c2[1] * y - c2[3] * y2 + z2s[1];
                z2s[1] = c2[2] * y - c2[4] * y2;
                ch_power += y2 * y2;
            }
            power += weight * ch_power;
        }
        block_powers.push(power / block_len as f64);
        start += hop;
    }
    if block_powers.is_empty() {
        return None;
    }

    // Absolute gate (-70 LUFS), then relative (-10 LU below the
    // absolute-gated mean).
    let abs_gated: Vec<f64> = block_powers
        .iter()
        .copied()
        .filter(|&p| p > 0.0 && to_lufs(p) > -70.0)
        .collect();
    if abs_gated.is_empty() {
        return None;
    }
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
    let rel_threshold = to_lufs(mean(&abs_gated)) - 10.0;
    let rel_gated: Vec<f64> = abs_gated
        .into_iter()
        .filter(|p| to_lufs(*p) > rel_threshold)
        .collect();
    if rel_gated.is_empty() {
        return None;
    }
    Some(to_lufs(mean(&rel_gated)))
}

/// K-weighting stage 1 (high shelf, +4 dB), RBJ-designed from the
/// BS.1770 parameters (f0 1681 Hz, Q 0.7072, +4 dB). The analytic form
/// reproduces the spec table at 48 kHz and stays correct at every other
/// rate, so it is used unconditionally (no per-rate special case: one
/// code path, one behavior).
fn k_weight_stage1(rate: f64) -> [f64; 5] {
    analytic_high_shelf(rate)
}

/// K-weighting stage 2 (RLB high-pass, f0 ~38 Hz, Q 0.5003), RBJ-designed
/// (same construction as stage 1 - see its doc comment).
fn k_weight_stage2(rate: f64) -> [f64; 5] {
    analytic_high_pass(rate)
}

/// RBJ high-shelf coefficients for the K pre-filter.
fn analytic_high_shelf(rate: f64) -> [f64; 5] {
    let f0 = 1681.0;
    let q = 0.7071752369554196;
    let gain_db = 3.99984385397;
    let a = 10f64.powf(gain_db / 40.0); // sqrt of linear gain
    let w0 = 2.0 * std::f64::consts::PI * f0 / rate;
    let (cw, sw) = (w0.cos(), w0.sin());
    let alpha = sw / (2.0 * q);
    let two_sqrt_a_alpha = 2.0 * a.sqrt() * alpha;
    let b0 = a * ((a + 1.0) + (a - 1.0) * cw + two_sqrt_a_alpha);
    let b1 = -2.0 * a * ((a - 1.0) + (a + 1.0) * cw);
    let b2 = a * ((a + 1.0) + (a - 1.0) * cw - two_sqrt_a_alpha);
    let a0 = (a + 1.0) - (a - 1.0) * cw + two_sqrt_a_alpha;
    let a1 = 2.0 * ((a - 1.0) - (a + 1.0) * cw);
    let a2 = (a + 1.0) - (a - 1.0) * cw - two_sqrt_a_alpha;
    [b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0]
}

/// RBJ high-pass coefficients for the RLB weighting filter.
fn analytic_high_pass(rate: f64) -> [f64; 5] {
    let f0 = 38.13547087602444;
    let q = 0.5003270373238773;
    let w0 = 2.0 * std::f64::consts::PI * f0 / rate;
    let (cw, sw) = (w0.cos(), w0.sin());
    let alpha = sw / (2.0 * q);
    let b0 = (1.0 + cw) / 2.0;
    let b1 = -(1.0 + cw);
    let b2 = (1.0 + cw) / 2.0;
    let a0 = 1.0 + alpha;
    let a1 = -2.0 * cw;
    let a2 = 1.0 - alpha;
    [b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0]
}

/// Decoding/streaming error surfaced by the file-level scan.
#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    #[error("source open failed: {0}")]
    Source(#[from] SourceError),
    #[error("decode failed: {0}")]
    Decode(#[from] DecodeError),
}

/// Scan a decoder source to (partial) completion and measure integrated
/// loudness. Streams chunk-by-chunk (no full-file buffer); bounded by
/// `max_seconds` of audio (60 s is plenty for a stable gated mean).
/// `None` loudness = honestly unmeasurable, not an error.
pub fn analyze_source(
    source: &mut AudioSource,
    max_seconds: f64,
) -> Result<LoudnessScan, ScanError> {
    let format: PcmFormat = source.pcm_format();
    let channels = usize::from(format.channels.max(1));
    let rate = format.sample_rate;

    // Accumulate decoded f32 up to the cap, then measure once. Memory:
    // 60 s stereo f32 at 44.1 kHz is ~21 MB worst case - acceptable for
    // an offline analysis pass (same shape as the previous web path).
    let cap_samples = (max_seconds * f64::from(rate)).round() as usize * channels;
    let mut decoded: Vec<f32> = Vec::new();
    loop {
        match source.next_chunk() {
            Ok(chunk) => {
                let s = chunk.to_f32_interleaved();
                if !s.is_empty() {
                    decoded.extend(s);
                    if decoded.len() >= cap_samples {
                        break;
                    }
                }
            }
            Err(DecodeError::EndOfStream) => break,
            Err(e) => return Err(e.into()),
        }
    }
    Ok(LoudnessScan {
        integrated_lufs: integrated_lufs(&decoded, channels, rate),
        sample_rate: rate,
        channels: format.channels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn sine(amp: f64, secs: f64, freq: f64) -> Vec<f32> {
        let n = (secs * RATE as f64) as usize;
        let mut v = Vec::with_capacity(n * 2);
        for i in 0..n {
            let x = amp * (2.0 * std::f64::consts::PI * freq * i as f64 / RATE as f64).sin();
            v.push(x as f32);
            v.push(x as f32);
        }
        v
    }

    /// Full-scale stereo 1 kHz sine: the R128/BS.1770-4 reference value
    /// is ~-0.3 LUFS (2 ch at MS 1.0 -> -0.691, K-weighting ~+0.44 dB
    /// at 1 kHz). A full-scale MONO tone is ~-3.3.
    #[test]
    fn full_scale_sine_measures_reference_lufs() {
        let s = sine(1.0, 3.0, 1000.0);
        let lufs = integrated_lufs(&s, 2, RATE).expect("measurable");
        assert!(
            (lufs - (-0.3)).abs() < 0.5,
            "full-scale stereo sine: expected ~-0.3 LUFS, got {lufs}"
        );
    }

    #[test]
    fn half_scale_sine_measures_reference_minus_6() {
        // -6 dB amplitude = -6 LU: ~-6.3 (the full-scale value minus 6).
        let s = sine(0.5, 3.0, 1000.0);
        let lufs = integrated_lufs(&s, 2, RATE).expect("measurable");
        assert!(
            (lufs - (-6.3)).abs() < 0.5,
            "half-scale stereo sine: expected ~-6.3 LUFS, got {lufs}"
        );
    }

    /// R128 calibration point: a 1 kHz sine at -23 dB level per channel
    /// measures -23 LUFS (stereo) by definition.
    #[test]
    fn minus_23_level_sine_is_reference_minus_23() {
        let s = sine(10f64.powf(-23.0 / 20.0), 3.0, 1000.0);
        let lufs = integrated_lufs(&s, 2, RATE).expect("measurable");
        assert!(
            (lufs - (-23.0)).abs() < 0.5,
            "-23 level stereo sine: expected ~-23.0 LUFS, got {lufs}"
        );
    }

    #[test]
    fn silence_is_none_not_minus_infinity() {
        assert!(integrated_lufs(&[0.0; RATE as usize * 2], 2, RATE).is_none());
    }

    #[test]
    fn too_short_input_is_none() {
        // 300 ms < one 400 ms gating block.
        assert!(integrated_lufs(&sine(0.5, 0.3, 1000.0), 2, RATE).is_none());
    }

    /// Two-level signal exercises the relative gate: 5 s loud, then 5 s
    /// 30 dB quieter. The quiet tail is below the -10 LU relative
    /// threshold, so the integrated value must follow the LOUD part.
    #[test]
    fn two_level_signal_relative_gating_follows_loud_part() {
        let mut s = sine(0.5, 5.0, 1000.0);
        let mut quiet = sine(0.5 * 10f64.powf(-30.0 / 20.0), 5.0, 1000.0);
        s.append(&mut quiet);
        let lufs = integrated_lufs(&s, 2, RATE).expect("measurable");
        let loud_ref = integrated_lufs(&sine(0.5, 5.0, 1000.0), 2, RATE).unwrap();
        assert!(
            (lufs - loud_ref).abs() < 1.0,
            "relative gate must exclude the quiet tail: {lufs} vs loud {loud_ref}"
        );
    }

    /// Alternative sample rate (44.1 kHz) must agree with the 48 kHz
    /// measurement of the same signal - the analytic coefficient path is
    /// the one real files take.
    #[test]
    fn rate_44100_agrees_with_48000() {
        let s48 = sine(0.5, 3.0, 1000.0);
        let l48 = integrated_lufs(&s48, 2, RATE).expect("measurable");
        let n = 3 * 44_100usize;
        let mut s44: Vec<f32> = Vec::with_capacity(n * 2);
        for i in 0..n {
            let x = 0.5 * (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / 44_100.0).sin();
            s44.push(x as f32);
            s44.push(x as f32);
        }
        let l44 = integrated_lufs(&s44, 2, 44_100).expect("measurable");
        assert!((l48 - l44).abs() < 0.2, "rate mismatch: {l48} vs {l44}");
    }

    #[test]
    fn mono_measures_3_lu_below_stereo() {
        let stereo = sine(0.5, 3.0, 1000.0);
        let mono: Vec<f32> = stereo.iter().step_by(2).copied().collect();
        let ls = integrated_lufs(&stereo, 2, RATE).unwrap();
        let lm = integrated_lufs(&mono, 1, RATE).unwrap();
        assert!((ls - lm - 3.01).abs() < 0.1, "stereo {ls} vs mono {lm}");
    }
}
