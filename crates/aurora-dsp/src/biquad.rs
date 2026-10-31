//! Biquad filter section with double-precision coefficients.
//!
//! Coefficients follow the "Audio EQ Cookbook" (R. Bristow-Johnson)
//! convention, processed in f64 regardless of the sample stream type:
//! filter state accumulation stays precise across long playback sessions.

use crate::error::DspError;

/// Filter shapes supported by one biquad section.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BiquadKind {
    PeakingEq,
    LowShelf,
    HighShelf,
    LowPass,
    HighPass,
}

/// Design parameters for one section.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BiquadParams {
    pub kind: BiquadKind,
    /// Corner/center frequency in Hz.
    pub freq_hz: f64,
    /// Q (bandwidth) for peaking and pass filters.
    pub q: f64,
    /// Gain in dB (shelf and peaking only; ignored by pass filters).
    pub gain_db: f64,
}

/// One biquad section: coefficients + per-channel state (direct form 1,
/// chosen for stability under coefficient drift and f64 rounding).
#[derive(Debug, Clone)]
pub struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    // Direct form 1 state per channel.
    x1: Vec<f64>,
    x2: Vec<f64>,
    y1: Vec<f64>,
    y2: Vec<f64>,
}

impl Biquad {
    /// Design a section for `sample_rate` and `channels`.
    pub fn design(params: BiquadParams, sample_rate: u32, channels: u16) -> Result<Self, DspError> {
        if sample_rate == 0 {
            return Err(DspError::InvalidSampleRate(sample_rate));
        }
        let nyquist = f64::from(sample_rate) / 2.0;
        if params.freq_hz <= 0.0 || params.freq_hz >= nyquist {
            return Err(DspError::FrequencyAboveNyquist(params.freq_hz, sample_rate));
        }

        let w0 = 2.0 * std::f64::consts::PI * params.freq_hz / f64::from(sample_rate);
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let q = params.q.max(1e-6);
        let alpha = match params.kind {
            BiquadKind::PeakingEq => sin_w0 / (2.0 * q),
            BiquadKind::LowShelf | BiquadKind::HighShelf => {
                sin_w0 / 2.0 * (10f64.powf(params.gain_db / 40.0)).sqrt()
            }
            BiquadKind::LowPass | BiquadKind::HighPass => sin_w0 / (2.0 * q),
        };

        let a = 10f64.powf(params.gain_db / 40.0); // sqrt of linear gain

        let (b0, b1, b2, a0, a1, a2) = match params.kind {
            BiquadKind::PeakingEq => (
                1.0 + alpha * a,
                -2.0 * cos_w0,
                1.0 - alpha * a,
                1.0 + alpha / a,
                -2.0 * cos_w0,
                1.0 - alpha / a,
            ),
            BiquadKind::LowShelf => (
                a * ((a + 1.0) - (a - 1.0) * cos_w0 + 2.0 * a.sqrt() * alpha),
                2.0 * a * ((a - 1.0) - (a + 1.0) * cos_w0),
                a * ((a + 1.0) - (a - 1.0) * cos_w0 - 2.0 * a.sqrt() * alpha),
                (a + 1.0) + (a - 1.0) * cos_w0 + 2.0 * a.sqrt() * alpha,
                -2.0 * ((a - 1.0) + (a + 1.0) * cos_w0),
                (a + 1.0) + (a - 1.0) * cos_w0 - 2.0 * a.sqrt() * alpha,
            ),
            BiquadKind::HighShelf => (
                a * ((a + 1.0) + (a - 1.0) * cos_w0 + 2.0 * a.sqrt() * alpha),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * cos_w0),
                a * ((a + 1.0) + (a - 1.0) * cos_w0 - 2.0 * a.sqrt() * alpha),
                (a + 1.0) - (a - 1.0) * cos_w0 + 2.0 * a.sqrt() * alpha,
                2.0 * ((a - 1.0) - (a + 1.0) * cos_w0),
                (a + 1.0) - (a - 1.0) * cos_w0 - 2.0 * a.sqrt() * alpha,
            ),
            BiquadKind::LowPass => (
                (1.0 - cos_w0) / 2.0,
                1.0 - cos_w0,
                (1.0 - cos_w0) / 2.0,
                1.0 + alpha,
                -2.0 * cos_w0,
                1.0 - alpha,
            ),
            BiquadKind::HighPass => (
                (1.0 + cos_w0) / 2.0,
                -(1.0 + cos_w0),
                (1.0 + cos_w0) / 2.0,
                1.0 + alpha,
                -2.0 * cos_w0,
                1.0 - alpha,
            ),
        };

        // Normalize by a0.
        let n = 1.0 / a0;
        let nch = usize::from(channels.max(1));
        Ok(Self {
            b0: b0 * n,
            b1: b1 * n,
            b2: b2 * n,
            a1: a1 * n,
            a2: a2 * n,
            x1: vec![0.0; nch],
            x2: vec![0.0; nch],
            y1: vec![0.0; nch],
            y2: vec![0.0; nch],
        })
    }

    /// Reset filter state (all channels to silence history).
    pub fn reset(&mut self) {
        self.x1.fill(0.0);
        self.x2.fill(0.0);
        self.y1.fill(0.0);
        self.y2.fill(0.0);
    }

    /// Process interleaved f32 samples in place; internal math in f64.
    ///
    /// Returns true if any output sample was clipped to the [-1, 1] range.
    pub fn process_interleaved(&mut self, buf: &mut [f32]) -> bool {
        let nch = self.x1.len();
        let mut clipped = false;
        for (i, s) in buf.iter_mut().enumerate() {
            let ch = i % nch;
            let x = f64::from(*s);
            let y = self.b0 * x + self.b1 * self.x1[ch] + self.b2 * self.x2[ch]
                - self.a1 * self.y1[ch]
                - self.a2 * self.y2[ch];
            self.x2[ch] = self.x1[ch];
            self.x1[ch] = x;
            self.y2[ch] = self.y1[ch];
            self.y1[ch] = y;
            if y > 1.0 {
                *s = 1.0;
                clipped = true;
            } else if y < -1.0 {
                *s = -1.0;
                clipped = true;
            } else {
                *s = y as f32;
            }
        }
        clipped
    }

    /// The magnitude response of the filter at `freq_hz` (linear gain).
    ///
    /// Used for regression tests and UI graphs; not on the audio path.
    pub fn magnitude_at(&self, freq_hz: f64, sample_rate: u32) -> f64 {
        let z_r = (-2.0 * std::f64::consts::PI * freq_hz / f64::from(sample_rate)).cos();
        // Evaluate H(z) at z = e^{jw}.
        let z_i = (1.0 - z_r * z_r).sqrt();
        let (b_re, b_im) = self.poly_response(self.b0, self.b1, self.b2, z_r, z_i);
        let (a_re, a_im) = self.poly_response(1.0, self.a1, self.a2, z_r, z_i);
        let num = (b_re * b_re + b_im * b_im).sqrt();
        let den = (a_re * a_re + a_im * a_im).sqrt();
        num / den
    }

    fn poly_response(&self, c0: f64, c1: f64, c2: f64, z_r: f64, z_i: f64) -> (f64, f64) {
        // p(z) = c0 + c1 z^-1 + c2 z^-2 with z^-1 = (z_r - j z_i)
        let p1_r = c1 * z_r;
        let p1_i = -c1 * z_i;
        let p2_r = c2 * (z_r * z_r - z_i * z_i);
        let p2_i = -2.0 * c2 * z_r * z_i;
        (c0 + p1_r + p2_r, p1_i + p2_i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peak(f: f64, gain_db: f64, q: f64) -> BiquadParams {
        BiquadParams {
            kind: BiquadKind::PeakingEq,
            freq_hz: f,
            q,
            gain_db,
        }
    }

    #[test]
    fn flat_gain_section_is_identity() {
        // 0 dB peaking EQ: linear gain 1 -> numerator == denominator.
        let mut bq = Biquad::design(peak(1000.0, 0.0, 1.0), 44_100, 2).unwrap();
        let mut buf: Vec<f32> = (0..64)
            .map(|i| ((i as f32) / 64.0 * 2.0 - 1.0) * 0.5)
            .collect();
        let input: Vec<f32> = buf.clone();
        bq.process_interleaved(&mut buf);
        for (a, b) in input.iter().zip(&buf) {
            assert!((a - b).abs() < 1e-9, "0dB EQ must be identity: {a} vs {b}");
        }
    }

    #[test]
    fn boost_actually_boosts_target_band() {
        let bq = Biquad::design(peak(1000.0, 12.0, 2.0), 44_100, 2).unwrap();
        let at_center = bq.magnitude_at(1000.0, 44_100);
        let at_low = bq.magnitude_at(20.0, 44_100);
        let at_high = bq.magnitude_at(20_000.0, 44_100);
        // 12 dB boost -> strong lift at the band center; a Q=2 peak sits
        // slightly below the nominal 4x at the exact center frequency.
        // Assert the shape: clear center lift, unity far away.
        assert!(
            at_center > 3.5 && at_center < 5.5,
            "center gain {at_center} out of expected band"
        );
        assert!((at_low - 1.0).abs() < 0.15, "low gain {at_low}");
        assert!((at_high - 1.0).abs() < 0.15, "high gain {at_high}");
    }

    #[test]
    fn nyquist_and_zero_freq_rejected() {
        assert!(Biquad::design(peak(22_050.0, 0.0, 1.0), 44_100, 2).is_err());
        assert!(Biquad::design(peak(0.0, 0.0, 1.0), 44_100, 2).is_err());
    }

    #[test]
    fn state_reset_returns_to_identity_of_first_chunk() {
        let mut bq = Biquad::design(peak(5000.0, 6.0, 1.4), 48_000, 1).unwrap();
        let input: Vec<f32> = (0..256).map(|i| (i as f32 * 0.01).sin() * 0.3).collect();

        let mut run1 = input.clone();
        bq.process_interleaved(&mut run1);
        bq.reset();
        let mut run2 = input;
        bq.process_interleaved(&mut run2);
        assert_eq!(run1, run2);
    }
}
