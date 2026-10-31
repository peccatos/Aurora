//! Gain stage: linear gain, ReplayGain application, and TPDF dithering for
//! integer output paths.
//!
//! Dither decorrelates quantization error from the signal — the difference
//! between "sounds digital" and "sounds analog" on quiet fades. TPDF
//! (triangular-PDF) noise at 2 LSB peak-to-peak is the audibly transparent
//! standard; applied only when the path converts float to integer output,
//! never on float-to-float.

use aurora_core::SampleFormat;

/// A gain stage with clipping telemetry.
#[derive(Debug, Clone)]
pub struct Gain {
    linear: f32,
}

impl Gain {
    pub fn from_db(db: f32) -> Self {
        Self {
            linear: 10f32.powf(db / 20.0),
        }
    }

    pub fn unity() -> Self {
        Self { linear: 1.0 }
    }

    pub fn is_unity(&self) -> bool {
        (self.linear - 1.0).abs() < 1e-9
    }

    /// Apply gain in place. Returns true if any sample clipped.
    pub fn process_interleaved(&mut self, buf: &mut [f32]) -> bool {
        if self.is_unity() {
            return false;
        }
        let g = self.linear;
        let mut clipped = false;
        for s in buf.iter_mut() {
            let v = *s * g;
            if v > 1.0 {
                *s = 1.0;
                clipped = true;
            } else if v < -1.0 {
                *s = -1.0;
                clipped = true;
            } else {
                *s = v;
            }
        }
        clipped
    }
}

/// ReplayGain mode: how loudness normalization is applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayGainMode {
    Off,
    /// Apply the track's own gain tag.
    Track,
    /// Apply album gain (preserves intra-album dynamics).
    Album,
}

/// TPDF dither generator with an internal xorshift state.
///
/// Deterministic and seedable for reproducible tests; quality is
/// statistically identical to white TPDF for the audio band.
#[derive(Debug, Clone)]
pub struct TpdfDither {
    state: u64,
}

impl TpdfDither {
    pub fn new(seed: u64) -> Self {
        Self { state: seed.max(1) }
    }

    fn next_u32(&mut self) -> u32 {
        // xorshift64* — fast, adequate statistical quality for dither.
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        (x.wrapping_mul(0x2545F4914F6CDD1D) >> 32) as u32
    }

    /// One TPDF sample in [-1, 1) scaled to 1 LSB of `bits` output.
    fn tpdf_lsb(&mut self, bits: u16) -> f32 {
        // Sum of two uniforms -> triangular distribution.
        let u1 = self.next_u32() as f64 / u32::MAX as f64;
        let u2 = self.next_u32() as f64 / u32::MAX as f64;
        let tri = (u1 + u2) - 1.0; // [-1, 1), triangular PDF
        let lsb = 2f64.powi(-(bits as i32 - 1));
        (tri * lsb) as f32
    }

    /// Dither `buf` in place for a subsequent conversion to `target`
    /// integer format. No-op for float targets.
    pub fn dither_for(&mut self, buf: &mut [f32], target: SampleFormat, bits: u16) {
        if matches!(target, SampleFormat::Float) {
            return;
        }
        if bits == 0 || bits > 32 {
            return;
        }
        for s in buf.iter_mut() {
            *s += self.tpdf_lsb(bits);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unity_gain_is_bit_identity() {
        let mut g = Gain::unity();
        let mut buf: Vec<f32> = (0..512).map(|i| ((i as f32) * 0.05).sin() * 0.9).collect();
        let input = buf.clone();
        assert!(!g.process_interleaved(&mut buf));
        assert_eq!(buf, input);
    }

    #[test]
    fn gain_minus_6db_halves() {
        let mut g = Gain::from_db(-6.0206); // exact -6.02 dB = /2
        let mut buf = vec![0.8f32; 4];
        g.process_interleaved(&mut buf);
        assert!((buf[0] - 0.4).abs() < 1e-6);
    }

    #[test]
    fn clipping_reported_and_limited() {
        let mut g = Gain::from_db(12.0);
        let mut buf = vec![0.9f32, -0.9, 0.1, 0.0];
        let clipped = g.process_interleaved(&mut buf);
        assert!(clipped);
        assert_eq!(buf[0], 1.0);
        assert_eq!(buf[1], -1.0);
        assert!((buf[2] - 0.39811).abs() < 0.001, "buf[2] = {}", buf[2]);
    }

    #[test]
    fn dither_is_noop_for_float_targets() {
        let mut d = TpdfDither::new(42);
        let mut buf = vec![0.5f32; 16];
        let input = buf.clone();
        d.dither_for(&mut buf, SampleFormat::Float, 32);
        assert_eq!(buf, input);
    }

    #[test]
    fn dither_noise_stays_within_one_lsb() {
        let mut d = TpdfDither::new(7);
        let mut buf = vec![0.0f32; 10_000];
        d.dither_for(&mut buf, SampleFormat::SignedInt, 16);
        let lsb = 2f32.powi(-15);
        for &s in &buf {
            assert!(s.abs() <= lsb, "dither exceeds 1 LSB: {s}");
        }
        // Mean is near zero (triangular, symmetric).
        let mean = buf.iter().sum::<f32>() / buf.len() as f32;
        assert!(mean.abs() < lsb * 0.05, "dither mean too high: {mean}");
    }

    #[test]
    fn dither_is_deterministic_from_seed() {
        let mut a = TpdfDither::new(123);
        let mut b = TpdfDither::new(123);
        let mut buf_a = vec![0.0f32; 64];
        let mut buf_b = vec![0.0f32; 64];
        a.dither_for(&mut buf_a, SampleFormat::SignedInt, 16);
        b.dither_for(&mut buf_b, SampleFormat::SignedInt, 16);
        assert_eq!(buf_a, buf_b);
    }
}
