//! Parametric equalizer: a cascade of biquad sections with a typed,
//! serializable configuration for the frontend.

use aurora_core::AudioTransformation;
use serde::{Deserialize, Serialize};

use crate::biquad::{Biquad, BiquadKind, BiquadParams};
use crate::error::DspError;

/// One EQ band as the frontend sees it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EqBandKind {
    Peaking,
    LowShelf,
    HighShelf,
    LowPass,
    HighPass,
}

/// One band's settings (UI-facing).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EqBand {
    pub kind: EqBandKind,
    pub freq_hz: f64,
    pub q: f64,
    pub gain_db: f64,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

impl EqBand {
    /// A neutral band: peaking at 1 kHz, 0 dB — no effect.
    pub fn neutral() -> Self {
        Self {
            kind: EqBandKind::Peaking,
            freq_hz: 1000.0,
            q: 1.0,
            gain_db: 0.0,
            enabled: true,
        }
    }

    fn biquad_params(self) -> BiquadParams {
        BiquadParams {
            kind: match self.kind {
                EqBandKind::Peaking => BiquadKind::PeakingEq,
                EqBandKind::LowShelf => BiquadKind::LowShelf,
                EqBandKind::HighShelf => BiquadKind::HighShelf,
                EqBandKind::LowPass => BiquadKind::LowPass,
                EqBandKind::HighPass => BiquadKind::HighPass,
            },
            freq_hz: self.freq_hz,
            q: self.q,
            gain_db: self.gain_db,
        }
    }

    fn is_neutral(&self) -> bool {
        match self.kind {
            EqBandKind::Peaking | EqBandKind::LowShelf | EqBandKind::HighShelf => {
                self.gain_db.abs() < 1e-9
            }
            EqBandKind::LowPass | EqBandKind::HighPass => false,
        }
    }
}

/// A 10-band parametric EQ (any band count works; ten is the UI preset).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Equalizer {
    pub bands: Vec<EqBand>,
    /// Output stage gain in dB after the chain.
    pub preamp_db: f64,
}

impl Equalizer {
    /// The classic 10-band UI: octave-spaced peaking bands, all flat.
    pub fn ten_band_flat() -> Self {
        let freqs = [
            31.25, 62.5, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0,
        ];
        Self {
            bands: freqs
                .iter()
                .map(|&f| EqBand {
                    freq_hz: f,
                    ..EqBand::neutral()
                })
                .collect(),
            preamp_db: 0.0,
        }
    }

    /// True when the configuration provably changes nothing.
    ///
    /// The DSP pipeline uses this for the honest bypass: an all-flat EQ
    /// reports no `Equalizer` transformation (ADR 0002).
    pub fn is_bypassed(&self) -> bool {
        self.preamp_db.abs() < 1e-9 && self.bands.iter().all(|b| !b.enabled || b.is_neutral())
    }

    /// Compile to a processing chain for `sample_rate`/`channels`.
    ///
    /// Neutral bands are skipped entirely: the identity property holds
    /// bit-exactly when the whole chain is bypassed.
    pub fn compile(&self, sample_rate: u32, channels: u16) -> Result<EqChain, DspError> {
        let mut sections = Vec::new();
        for band in &self.bands {
            if !band.enabled || band.is_neutral() {
                continue;
            }
            sections.push(Biquad::design(band.biquad_params(), sample_rate, channels)?);
        }
        Ok(EqChain {
            sections,
            preamp: 10f64.powf(self.preamp_db / 20.0),
        })
    }

    /// The transformation this EQ reports when active.
    pub fn transformation(&self) -> Option<AudioTransformation> {
        if self.is_bypassed() {
            None
        } else {
            Some(AudioTransformation::Equalizer)
        }
    }
}

/// The compiled, stateful processing chain (not serializable; lives on the
/// DSP worker).
pub struct EqChain {
    sections: Vec<Biquad>,
    preamp: f64,
}

impl EqChain {
    /// Process interleaved f32 samples in place.
    ///
    /// Returns true if clipping protection engaged anywhere in the chain.
    pub fn process_interleaved(&mut self, buf: &mut [f32]) -> bool {
        let mut clipped = false;
        for bq in &mut self.sections {
            clipped |= bq.process_interleaved(buf);
        }
        if (self.preamp - 1.0).abs() > 1e-12 {
            let g = self.preamp as f32;
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
        }
        clipped
    }

    pub fn reset(&mut self) {
        for bq in &mut self.sections {
            bq.reset();
        }
    }

    pub fn is_empty(&self) -> bool {
        self.sections.is_empty() && (self.preamp - 1.0).abs() < 1e-12
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_ten_band_compiles_to_empty_chain() {
        let eq = Equalizer::ten_band_flat();
        assert!(eq.is_bypassed());
        let chain = eq.compile(44_100, 2).unwrap();
        assert!(chain.is_empty());
        assert_eq!(eq.transformation(), None);
    }

    #[test]
    fn flat_chain_is_bit_identity() {
        // Bypass identity: flat EQ processes samples bit-exactly.
        let eq = Equalizer::ten_band_flat();
        let mut chain = eq.compile(44_100, 2).unwrap();
        let mut buf: Vec<f32> = (0..1000).map(|i| ((i as f32) * 0.03).sin() * 0.8).collect();
        let input = buf.clone();
        let clipped = chain.process_interleaved(&mut buf);
        assert!(!clipped);
        assert_eq!(buf, input, "flat EQ must be bit-exact identity");
    }

    #[test]
    fn disabled_bands_are_skipped() {
        let mut eq = Equalizer::ten_band_flat();
        eq.bands[3].gain_db = 15.0;
        eq.bands[3].enabled = false;
        assert!(eq.is_bypassed());
        let chain = eq.compile(44_100, 2).unwrap();
        assert!(chain.is_empty());
    }

    #[test]
    fn preamp_only_config_still_reports_transformation() {
        let mut eq = Equalizer::ten_band_flat();
        eq.preamp_db = -3.0;
        assert!(!eq.is_bypassed());
        assert_eq!(eq.transformation(), Some(AudioTransformation::Equalizer));
        let mut chain = eq.compile(44_100, 2).unwrap();
        let mut buf = vec![0.5f32; 8];
        chain.process_interleaved(&mut buf);
        // -3 dB on a 0.5 input: 0.5 * 0.7079 = 0.3540.
        assert!((buf[0] - 0.3540).abs() < 0.001, "buf[0] = {}", buf[0]);
    }

    #[test]
    fn roundtrip_serialization() {
        let mut eq = Equalizer::ten_band_flat();
        eq.bands[5].gain_db = -4.5;
        eq.bands[5].q = 2.2;
        eq.preamp_db = -1.0;
        let json = serde_json::to_string(&eq).unwrap();
        let back: Equalizer = serde_json::from_str(&json).unwrap();
        assert_eq!(eq, back);
    }
}
