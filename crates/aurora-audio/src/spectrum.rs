//! Real-time spectrum analysis (decode-worker side, never the callback).
//!
//! 16 log-spaced bands (40 Hz .. 16 kHz) from a 2048-point real FFT over
//! the mono mix of the most recent decoded audio. Display-only: coarse,
//! ~11 Hz frame resolution is plenty for a VU bar and costs the worker
//! microseconds per chunk.

use rustfft::{FftPlanner, num_complex::Complex};

/// Number of display bands.
pub const BANDS: usize = 16;

/// FFT size (real samples, mono mix).
const FFT_LEN: usize = 2048;

/// Low/high band edges (Hz).
const F_LOW: f64 = 40.0;
const F_HIGH: f64 = 16_000.0;

/// Rolling analysis window: the newest FFT_LEN mono samples.
pub struct SpectrumAnalyzer {
    window: Vec<f32>,
    bands: Vec<f32>,
    // Precomputed band bin ranges for the current sample rate.
    band_bins: Vec<(usize, usize)>,
    sample_rate: u32,
}

impl SpectrumAnalyzer {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            window: Vec::with_capacity(FFT_LEN),
            bands: vec![0.0; BANDS],
            band_bins: Self::compute_band_bins(sample_rate),
            sample_rate,
        }
    }

    /// Recompute bin ranges when the rate changes.
    pub fn set_rate(&mut self, sample_rate: u32) {
        if sample_rate != self.sample_rate {
            self.band_bins = Self::compute_band_bins(sample_rate);
            self.sample_rate = sample_rate;
        }
    }

    fn compute_band_bins(rate: u32) -> Vec<(usize, usize)> {
        let nyquist = f64::from(rate) / 2.0;
        let high = F_HIGH.min(nyquist * 0.95);
        let log_low = F_LOW.ln();
        let log_high = high.ln();
        let bin_hz = f64::from(rate) / FFT_LEN as f64;
        (0..BANDS)
            .map(|i| {
                let f0 =
                    (log_low + (log_high - log_low) * f64::from(i as u32) / BANDS as f64).exp();
                let f1 =
                    (log_low + (log_high - log_low) * f64::from(i as u32 + 1) / BANDS as f64).exp();
                let b0 = ((f0 / bin_hz).floor() as usize).max(1);
                let b1 = ((f1 / bin_hz).ceil() as usize).max(b0 + 1).min(FFT_LEN / 2);
                (b0, b1)
            })
            .collect()
    }

    /// Feed interleaved f32 samples (any channel count; mono-mixed).
    /// Maintains the rolling window; recomputes bands at most every
    /// FFT_LEN/2 samples (50% overlap).
    pub fn push(&mut self, interleaved: &[f32], channels: usize) {
        let nch = channels.max(1);
        for frame in interleaved.chunks(nch) {
            let mono = frame.iter().sum::<f32>() / frame.len() as f32;
            self.window.push(mono);
        }
        if self.window.len() > FFT_LEN {
            let excess = self.window.len() - FFT_LEN;
            self.window.drain(..excess);
        }
    }

    /// Recompute the band energies from the current window. Call from the
    /// worker between chunks.
    pub fn analyze(&mut self) -> &[f32] {
        if self.window.len() < FFT_LEN {
            return &self.bands;
        }
        // Hann window + real FFT.
        let mut planner = FftPlanner::new();
        let fft = planner.plan_fft_forward(FFT_LEN);
        let mut input: Vec<Complex<f32>> = self
            .window
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let w =
                    0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / FFT_LEN as f32).cos());
                Complex::new(s * w, 0.0)
            })
            .collect();
        fft.process(&mut input);
        for (i, (b0, b1)) in self.band_bins.iter().enumerate() {
            let mut acc = 0f32;
            for c in &input[*b0..*b1] {
                acc += c.norm_sqr();
            }
            let n = (b1 - b0).max(1) as f32;
            // Magnitude scale: sqrt of mean power, log-ish display curve.
            let v = (acc / n).sqrt();
            // dB-ish mapping: 1e-4 floor .. ~1.0 ceil.
            self.bands[i] = ((v.log10() + 4.0) / 4.0).clamp(0.0, 1.0);
        }
        &self.bands
    }
}
