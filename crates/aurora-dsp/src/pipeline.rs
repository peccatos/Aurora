//! The DSP pipeline: explicit, ordered, honestly-reported processing chain.
//!
//! Design contract (ARCHITECTURE.md invariant 4 + ADR 0002):
//! - The chain is *explicitly bypassable*: with no processors configured,
//!   `process` is a bit-exact no-op (verified by tests).
//! - Every active stage contributes its `AudioTransformation` to the
//!   diagnostics; the output format's rate/format fields reflect reality.
//! - Processing runs on the DSP worker thread — never in the audio
//!   callback.

use aurora_core::{AudioTransformation, PcmFormat, SampleFormat};

use crate::eq::Equalizer;
use crate::error::DspError;
use crate::gain::Gain;
use crate::resample::Resampler;

/// User-facing DSP configuration (serializable, UI-driven).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct DspConfig {
    pub eq: Equalizer,
    /// Master volume in dB.
    pub volume_db: f32,
    /// ReplayGain mode.
    pub replaygain: ReplayGainModeDef,
}

/// Serde-friendly ReplayGain mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReplayGainModeDef {
    #[default]
    Off,
    Track,
    Album,
}

impl Default for DspConfig {
    fn default() -> Self {
        Self {
            eq: Equalizer::ten_band_flat(),
            volume_db: 0.0,
            replaygain: ReplayGainModeDef::Off,
        }
    }
}

impl Default for Equalizer {
    fn default() -> Self {
        Equalizer::ten_band_flat()
    }
}

impl DspConfig {
    /// True when this configuration provably passes audio untouched.
    pub fn is_bypassed(&self) -> bool {
        self.eq.is_bypassed()
            && self.volume_db.abs() < 1e-9
            && self.replaygain == ReplayGainModeDef::Off
    }

    /// All transformations this configuration reports when active.
    pub fn transformations(&self) -> Vec<AudioTransformation> {
        let mut out = Vec::new();
        if let Some(t) = self.eq.transformation() {
            out.push(t);
        }
        if self.volume_db.abs() >= 1e-9 || self.replaygain != ReplayGainModeDef::Off {
            out.push(AudioTransformation::Gain);
        }
        out
    }
}

/// A compiled pipeline for one playback stream.
///
/// Built once per track (knows the stream's sample rate and channel count);
/// processing is stateful (filter history, resampler position) and must run
/// in stream order on the worker thread.
pub struct DspPipeline {
    eq_chain: Option<crate::eq::EqChain>,
    volume: Gain,
    /// ReplayGain tag value for the current track (dB), if a mode uses it.
    replaygain_db: Option<f32>,
    resampler: Option<Resampler>,
    /// In/out formats for diagnostics.
    source_format: PcmFormat,
    target_rate: Option<u32>,
    /// Clipping occurred somewhere in the chain since the last check.
    clipped: bool,
}

impl DspPipeline {
    /// Compile a pipeline for `source_format` and a configuration.
    ///
    /// When `target_rate` differs from the source rate, a resampler stage
    /// is inserted (and reported); `None` means passthrough.
    pub fn compile(
        config: &DspConfig,
        source_format: PcmFormat,
        target_rate: Option<u32>,
        replaygain_db: Option<f32>,
    ) -> Result<Self, DspError> {
        let eq_chain = if config.eq.is_bypassed() {
            None
        } else {
            Some(
                config
                    .eq
                    .compile(source_format.sample_rate, source_format.channels)?,
            )
        };

        let needs_resample = target_rate
            .map(|t| t != source_format.sample_rate)
            .unwrap_or(false);
        let resampler = if needs_resample {
            let t = target_rate.expect("checked");
            Some(Resampler::new(
                source_format.sample_rate,
                t,
                source_format.channels,
            )?)
        } else {
            None
        };

        let mut volume = Gain::from_db(config.volume_db);
        if volume.is_unity() {
            volume = Gain::unity();
        }

        Ok(Self {
            eq_chain,
            volume,
            replaygain_db: replaygain_db.filter(|_| config.replaygain != ReplayGainModeDef::Off),
            resampler,
            source_format,
            target_rate: target_rate.filter(|_t| needs_resample),
            clipped: false,
        })
    }

    /// Process one chunk of interleaved f32 samples.
    ///
    /// Returns the processed interleaved samples (length may differ from
    /// the input when resampling). The pipeline owns the returned buffer's
    /// data; callers should consume it before the next call.
    pub fn process(&mut self, interleaved: &mut Vec<f32>) -> Vec<f32> {
        if let Some(eq) = self.eq_chain.as_mut() {
            self.clipped |= eq.process_interleaved(interleaved);
        }

        // ReplayGain before volume: the tag sets loudness, volume trims it.
        if let Some(rg) = self.replaygain_db {
            let mut g = Gain::from_db(rg);
            self.clipped |= g.process_interleaved(interleaved);
        }

        self.clipped |= self.volume.process_interleaved(interleaved);

        let Some(rs) = self.resampler.as_mut() else {
            return std::mem::take(interleaved);
        };
        let input = std::mem::take(interleaved);
        match rs.process(&input) {
            Ok(out) => out,
            Err(e) => {
                // Resampling failure must not kill playback: pass the
                // unresampled chunk through and log loudly — the
                // diagnostics still report the transformation.
                tracing::error!(%e, "resampler failure, passing through");
                input
            }
        }
    }

    /// Whether clipping protection engaged since the last call.
    pub fn take_clipped(&mut self) -> bool {
        std::mem::replace(&mut self.clipped, false)
    }

    /// The transformations this pipeline applies.
    pub fn transformations(&self) -> Vec<AudioTransformation> {
        let mut out = Vec::new();
        if self.eq_chain.is_some() {
            out.push(AudioTransformation::Equalizer);
        }
        if !self.volume.is_unity() || self.replaygain_db.is_some() {
            out.push(AudioTransformation::Gain);
        }
        if self.resampler.is_some() {
            out.push(AudioTransformation::Resampling);
        }
        out
    }

    /// The effective output format after this pipeline.
    pub fn output_format(&self) -> PcmFormat {
        let mut f = self.source_format;
        if let Some(rate) = self.target_rate {
            f.sample_rate = rate;
        }
        // The DSP path's canonical internal/output representation is f32.
        f.bits_per_sample = 32;
        f.sample_format = SampleFormat::Float;
        f
    }

    pub fn is_passthrough(&self) -> bool {
        self.eq_chain.is_none()
            && self.volume.is_unity()
            && self.replaygain_db.is_none()
            && self.resampler.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aurora_core::ChannelLayout;

    fn f32_stereo(rate: u32) -> PcmFormat {
        PcmFormat {
            sample_rate: rate,
            channels: 2,
            bits_per_sample: 32,
            sample_format: SampleFormat::Float,
            channel_layout: ChannelLayout::Interleaved,
        }
    }

    fn music_like(n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| ((i as f32) * 0.017).sin() * 0.6 + ((i as f32) * 0.003).sin() * 0.3)
            .collect()
    }

    #[test]
    fn bypassed_pipeline_is_bit_identity() {
        // Phase 4 exit criteria: bypass identity.
        let cfg = DspConfig::default();
        assert!(cfg.is_bypassed());
        let mut pl = DspPipeline::compile(&cfg, f32_stereo(44_100), None, None).unwrap();
        assert!(pl.is_passthrough());
        assert!(pl.transformations().is_empty());

        let mut input = music_like(2048);
        let reference = input.clone();
        let output = pl.process(&mut input);
        assert_eq!(output, reference, "bypass must be bit-exact");
    }

    #[test]
    fn eq_only_pipeline_preserves_length_when_no_resample() {
        let mut cfg = DspConfig::default();
        cfg.eq.bands[2].gain_db = 3.0;
        let mut pl = DspPipeline::compile(&cfg, f32_stereo(44_100), None, None).unwrap();
        let mut input = music_like(4096);
        let out = pl.process(&mut input);
        assert_eq!(out.len(), 4096);
        assert_eq!(pl.transformations(), vec![AudioTransformation::Equalizer]);
    }

    #[test]
    fn resampling_pipeline_changes_rate_and_reports() {
        let cfg = DspConfig::default();
        let mut pl = DspPipeline::compile(&cfg, f32_stereo(44_100), Some(48_000), None).unwrap();
        assert_eq!(pl.transformations(), vec![AudioTransformation::Resampling]);

        // The resampler wants exactly input_frames_next() frames; feed
        // several chunks so the delay fills and steady state is reached.
        let chunk_frames = 2048;
        let chunk_samples = chunk_frames * 2; // stereo
        let mut produced = 0usize;
        for _ in 0..4 {
            let mut input = music_like(chunk_samples);
            let out = pl.process(&mut input);
            produced += out.len();
        }
        // 4 x 2048 frames @44.1k -> ~4 x 2234 frames @48k after the delay
        // settles; anything above passthrough size proves rate conversion.
        assert!(
            produced > chunk_samples * 4,
            "expected rate-converted growth, got {produced}"
        );
        assert_eq!(pl.output_format().sample_rate, 48_000);
        assert_eq!(pl.output_format().sample_format, SampleFormat::Float);
    }

    #[test]
    fn volume_only_pipeline_scales_and_reports_gain() {
        let cfg = DspConfig {
            volume_db: -6.0,
            ..DspConfig::default()
        };
        let mut pl = DspPipeline::compile(&cfg, f32_stereo(44_100), None, None).unwrap();
        let mut input = vec![0.5f32; 64];
        let out = pl.process(&mut input);
        // -6 dB: 0.5 * 10^(-6/20) = 0.25059.
        assert!((out[0] - 0.25059).abs() < 1e-4, "out {}", out[0]);
        assert_eq!(pl.transformations(), vec![AudioTransformation::Gain]);
    }

    #[test]
    fn full_config_reported_honestly() {
        let mut cfg = DspConfig::default();
        cfg.eq.bands[1].gain_db = -2.0;
        cfg.volume_db = -3.0;
        let pl = DspPipeline::compile(&cfg, f32_stereo(44_100), Some(96_000), None).unwrap();
        let ts = pl.transformations();
        assert!(ts.contains(&AudioTransformation::Equalizer));
        assert!(ts.contains(&AudioTransformation::Gain));
        assert!(ts.contains(&AudioTransformation::Resampling));
    }

    #[test]
    fn clipping_flag_surfaces_and_resets() {
        let cfg = DspConfig {
            volume_db: 12.0,
            ..DspConfig::default()
        };
        let mut pl = DspPipeline::compile(&cfg, f32_stereo(44_100), None, None).unwrap();
        let mut input = vec![0.9f32; 16];
        let _ = pl.process(&mut input);
        assert!(pl.take_clipped());
        assert!(!pl.take_clipped(), "flag must reset");
    }
}
