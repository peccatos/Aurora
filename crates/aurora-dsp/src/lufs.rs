//! R128 loudness measurement (ITU-R BS.1770-4): integrated LUFS.
//!
//! Used at web ingest to give every track an honest loudness number
//! even without a ReplayGain tag: the value feeds the pipeline's
//! existing replaygain gain stage (mode "track") — normalized playback
//! with zero new audio-path code.
//!
//! K-weighting is the standard two-stage biquad (high-shelf +4 dB,
//! then 38 Hz high-pass), coefficients computed per rate with the
//! RBJ cookbook formulas the rest of the crate uses. Gating: 400 ms
//! blocks at 100 ms hops, -70 LUFS absolute + -10 LU relative.
//!
//! Honesty (ADR 0002): the number is a MEASUREMENT of the decoded
//! audio — computed identically every time, not a tag echo.

/// One direct-form-2-transposed biquad (f64) for a single channel.
#[derive(Clone)]
struct OnePolePair {
    // Stage 1: high shelf. Stage 2: high pass.
    s: ([f64; 5], [f64; 5]), // (b0,b1,b2,a1,a2) coefficients
    z: ([f64; 2], [f64; 2]), // DF2T state per stage
}

impl OnePolePair {
    fn process(&mut self, x: f64) -> f64 {
        let (c, st) = (&self.s.0, &mut self.z.0);
        let y = c[0] * x + st[0];
        st[0] = c[1] * x - c[3] * y + st[1];
        st[1] = c[2] * x - c[4] * y;
        let (c2, st2) = (&self.s.1, &mut self.z.1);
        let y2 = c2[0] * y + st2[0];
        st2[0] = c2[1] * y - c2[3] * y2 + st2[1];
        st2[1] = c2[2] * y - c2[4] * y2;
        y2
    }
}

/// RBJ high-shelf coefficients (b0..a2).
fn high_shelf(rate: f64) -> [f64; 5] {
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

/// RBJ high-pass coefficients.
fn high_pass(rate: f64) -> [f64; 5] {
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

/// Measured integrated loudness in LUFS (negative, e.g. -12.3).
/// None when the input is too short/empty to measure honestly.
pub fn integrated_lufs(samples: &[f32], channels: usize, sample_rate: u32) -> Option<f32> {
    if channels == 0 || samples.is_empty() || sample_rate == 0 {
        return None;
    }
    let nch = channels.max(1);
    let frames = samples.len() / nch;
    if frames < (sample_rate as usize / 4) {
        return None; // shorter than one gating block
    }

    let rate = f64::from(sample_rate);
    let mut filters: Vec<OnePolePair> = (0..nch)
        .map(|_| OnePolePair {
            s: (high_shelf(rate), high_pass(rate)),
            z: ([0.0; 2], [0.0; 2]),
        })
        .collect();

    let block_len = (0.4 * rate).round() as usize;
    let hop = (0.1 * rate).round() as usize;
    let to_lufs = |p: f64| -0.691 + 10.0 * p.log10();

    // Block mean squares (K-weighted, channel-weighted).
    let mut block_powers: Vec<f64> = Vec::new();
    let mut start = 0usize;
    while start + block_len <= frames {
        let mut power = 0f64;
        for (ch, f) in filters.iter_mut().enumerate() {
            // BS.1770 channel weights: front pair 1.0, surround 1.41.
            let weight = if nch <= 2 || ch < 2 { 1.0 } else { 1.41 };
            let mut ch_power = 0f64;
            for fr in start..start + block_len {
                let x = f64::from(samples[fr * nch + ch]);
                let y = f.process(x);
                ch_power += y * y;
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
    Some(to_lufs(mean(&rel_gated)) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_measures_none() {
        assert!(integrated_lufs(&[0.0; 48_000 * 2], 2, 48_000).is_none());
    }

    #[test]
    fn loud_tone_measures_near_its_level() {
        // Full-scale 1 kHz stereo sine: ~-3.0 LUFS expected ballpark
        // (a full-scale square/sine measures around -3 .. -6 LUFS).
        let n = 48_000;
        let mut samples = Vec::with_capacity(n * 2);
        for i in 0..n {
            let v = 0.9 * (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / 48_000.0).sin();
            samples.push(v as f32);
            samples.push(v as f32);
        }
        let lufs = integrated_lufs(&samples, 2, 48_000).expect("measurable");
        assert!((-8.0..-1.0).contains(&lufs), "unexpected loudness: {lufs}");
    }

    #[test]
    fn quieter_tone_measures_lower() {
        let n = 48_000;
        let loud: Vec<f32> = (0..n * 2)
            .map(|i| 0.9 * (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / 48_000.0).sin() as f32)
            .collect();
        let quiet: Vec<f32> = loud.iter().map(|s| s * 0.1).collect();
        let l1 = integrated_lufs(&loud, 2, 48_000).unwrap();
        let l2 = integrated_lufs(&quiet, 2, 48_000).unwrap();
        assert!(
            l2 < l1 - 15.0,
            "10x quieter must be ~20 LU quieter: {l1} vs {l2}"
        );
    }
}
