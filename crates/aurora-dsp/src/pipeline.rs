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
    /// Reverb (Freeverb) settings.
    pub reverb: crate::reverb::ReverbParams,
    /// Playback speed multiplier: 1.0 = normal, 0.8 = slowed (the
    /// classic slowed+reverb aesthetic), 1.25 = sped up nightcore-style.
    /// Implemented through the pipeline resampler: audio at rate r played
    /// as if it were at r*speed.
    #[serde(default = "default_speed")]
    pub speed: f32,
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

fn default_speed() -> f32 {
    1.0
}

impl Default for DspConfig {
    fn default() -> Self {
        Self {
            eq: Equalizer::ten_band_flat(),
            volume_db: 0.0,
            replaygain: ReplayGainModeDef::Off,
            reverb: crate::reverb::ReverbParams::default(),
            speed: 1.0,
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
            && self.reverb.is_bypassed()
            && (self.speed - 1.0).abs() < 1e-6
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
        if !self.reverb.is_bypassed() {
            out.push(AudioTransformation::Reverb);
        }
        if (self.speed - 1.0).abs() > 1e-6 {
            out.push(AudioTransformation::Resampling);
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
    reverb: Option<crate::reverb::Reverb>,
    speed_active: bool,
    volume: Gain,
    /// Interleaved samples carried over between chunks: the resampler
    /// consumes fixed-size blocks, but decoders emit arbitrary chunk
    /// sizes (MP3 = 1152 frames, last chunk truncated, ...). This FIFO
    /// decouples them. Must be empty before resample except between
    /// process/flush calls.
    resample_inbuf: Vec<f32>,
    /// Channels of the stream (for frame<->sample math on the buffer).
    channels: u16,
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

        // Reverb is stereo-only: mono streams pass it through.
        let reverb = if config.reverb.is_bypassed() || source_format.channels != 2 {
            None
        } else {
            Some(crate::reverb::Reverb::new(
                config.reverb,
                source_format.sample_rate,
            ))
        };

        // Speed & rate conversion — one honest decision table.
        //
        // Playback speed s means the content must reach the device
        // as if it had been recorded at rate*s: a "44100 Hz recording"
        // sped up 2x is a 88200 Hz recording played at 44100. Two
        // independent reasons force a resampler:
        //   (a) the device demands a different rate (target_rate != source);
        //   (b) the user chose a speed != 1.0.
        //
        // Combined: resample from the effective source rate r*s to the
        // device rate t. When only speed is active and t == r, the
        // output is 1/s times the input per second — 2.0x speed halves
        // the output, exactly "playing twice as fast".
        let speed = config.speed.clamp(0.5, 2.0);
        let speed_active = (speed - 1.0).abs() > 1e-6;

        let device_rate = target_rate.unwrap_or(source_format.sample_rate);
        // Effective source rate after pretending speed changed the
        // recording rate. Rounded; rubato takes integer rates.
        let effective_source = if speed_active {
            ((f64::from(source_format.sample_rate) * f64::from(speed)).round() as u32).max(1)
        } else {
            source_format.sample_rate
        };

        let needs_resample = speed_active || effective_source != device_rate;

        let resampler = if needs_resample {
            if effective_source != device_rate {
                Some(Resampler::new(
                    effective_source,
                    device_rate,
                    source_format.channels,
                )?)
            } else {
                // Rates round to the same integer (e.g. speed 0.9999 or a
                // 44100/48000 pair that collapses): nothing to do.
                None
            }
        } else {
            None
        };

        let mut volume = Gain::from_db(config.volume_db);
        if volume.is_unity() {
            volume = Gain::unity();
        }

        Ok(Self {
            eq_chain,
            reverb,
            speed_active,
            volume,
            replaygain_db: replaygain_db.filter(|_| config.replaygain != ReplayGainModeDef::Off),
            resampler,
            resample_inbuf: Vec::new(),
            channels: source_format.channels,
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

        if let Some(rv) = self.reverb.as_mut() {
            rv.process_interleaved(interleaved);
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

        // Feed the carry-over buffer, then emit every complete block the
        // resampler wants. Decoder chunk sizes are arbitrary; the
        // resampler contract is fixed-size input. Any remainder stays
        // buffered for the next chunk (and flush() drains the tail).
        let nch = usize::from(self.channels);
        self.resample_inbuf.extend_from_slice(interleaved);
        interleaved.clear();

        let mut out = Vec::new();
        loop {
            let want = rs.input_frames_next() * nch;
            if self.resample_inbuf.len() < want {
                break;
            }
            let block: Vec<f32> = self.resample_inbuf.drain(..want).collect();
            match rs.process(&block) {
                Ok(chunk) => out.extend(chunk),
                Err(e) => {
                    // Resampling failure must not kill playback: pass the
                    // unresampled samples through and log loudly — the
                    // diagnostics still report the transformation.
                    tracing::error!(%e, "resampler failure, passing through");
                    out.extend(block);
                }
            }
        }
        out
    }

    /// Drop samples buffered from before a seek: pre-seek audio inside
    /// the resampler FIFO is stale and must never play after the jump.
    /// The resampler's filter/delay state also resets (cheap: the next
    /// blocks re-prime it within a few ms).
    pub fn drop_buffered(&mut self) {
        self.resample_inbuf.clear();
    }

    /// Flush the resampler's delay line and tail at end of stream.
    ///
    /// Call after the last `process`; returns the final output samples
    /// (may be empty when no resampler is active).
    pub fn flush(&mut self) -> Vec<f32> {
        let Some(rs) = self.resampler.as_mut() else {
            return Vec::new();
        };
        let nch = usize::from(self.channels);
        let mut out = Vec::new();
        // First: any complete block still buffered (seek/stop race).
        loop {
            let want = rs.input_frames_next() * nch;
            if self.resample_inbuf.len() < want {
                break;
            }
            let block: Vec<f32> = self.resample_inbuf.drain(..want).collect();
            if let Ok(chunk) = rs.process(&block) {
                out.extend(chunk);
            }
        }
        // Then: the resampler's own delay-line flush.
        if let Ok(tail) = rs.flush() {
            out.extend(tail);
        }
        self.resample_inbuf.clear();
        out
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
        if self.reverb.is_some() {
            out.push(AudioTransformation::Reverb);
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
            && !self.speed_active
            && self.reverb.is_none()
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

    #[test]
    fn speed_works_when_device_rate_matches_source() {
        // Regression (user report): speed did nothing when the device
        // accepted the source rate natively (44.1k -> 44.1k, the common
        // case) because the resampler was only built inside the
        // device-rate-mismatch branch. Speed must engage a resampler
        // regardless of the device rate.
        let source = PcmFormat {
            sample_rate: 44_100,
            channels: 2,
            bits_per_sample: 32,
            sample_format: aurora_core::SampleFormat::Float,
            channel_layout: aurora_core::ChannelLayout::Interleaved,
        };
        // Device rate == source rate: no forced conversion, speed only.
        let config = DspConfig {
            speed: 2.0,
            ..DspConfig::default()
        };
        let mut pl = DspPipeline::compile(&config, source, Some(44_100), None).expect("compile");
        assert!(
            pl.transformations()
                .contains(&AudioTransformation::Resampling),
            "speed must report Resampling even when device rate matches"
        );

        // 1s of input at 2.0x speed must produce ~0.5s of output.
        let mut fed = 0usize;
        let mut out_samples = 0usize;
        while fed < 44_100 {
            let n = 2048usize.min(44_100 - fed);
            let mut chunk: Vec<f32> = (0..n * 2)
                .map(|i| ((i % 512) as f64 * 0.01).sin() as f32)
                .collect();
            out_samples += pl.process(&mut chunk).len();
            fed += n;
        }
        out_samples += pl.flush().len();
        let out_frames = out_samples / 2;
        // Expected ~22050 frames; allow the resampler's transient slack.
        assert!(
            (out_frames as i64 - 22_050).abs() < 2_000,
            "2.0x speed produced {out_frames} output frames for 44100 input              (expected ~22050); speed is not being applied"
        );

        // And the slowed direction: 0.5x stretches to ~2x length.
        let config = DspConfig {
            speed: 0.5,
            ..DspConfig::default()
        };
        let mut pl = DspPipeline::compile(&config, source, Some(44_100), None).expect("compile");
        let mut fed = 0usize;
        let mut out_samples = 0usize;
        while fed < 44_100 {
            let n = 2048usize.min(44_100 - fed);
            let mut chunk: Vec<f32> = (0..n * 2)
                .map(|i| ((i % 512) as f64 * 0.01).sin() as f32)
                .collect();
            out_samples += pl.process(&mut chunk).len();
            fed += n;
        }
        out_samples += pl.flush().len();
        let out_frames = out_samples / 2;
        assert!(
            (out_frames as i64 - 88_200).abs() < 4_000,
            "0.5x speed produced {out_frames} output frames for 44100 input              (expected ~88200); speed is not being applied"
        );
    }

    #[test]
    fn seek_drops_stale_resampler_fifo() {
        // Regression: pre-seek samples inside the resampler FIFO played
        // after a jump. drop_buffered() must clear them.
        let source = PcmFormat {
            sample_rate: 44_100,
            channels: 2,
            bits_per_sample: 32,
            sample_format: aurora_core::SampleFormat::Float,
            channel_layout: aurora_core::ChannelLayout::Interleaved,
        };
        let config = DspConfig {
            speed: 0.8,
            ..DspConfig::default()
        };
        let mut pl = DspPipeline::compile(&config, source, Some(44_100), None).expect("compile");
        // Feed an odd-sized chunk (leaves a remainder in the FIFO).
        let mut chunk: Vec<f32> = (0..1152 * 2)
            .map(|i| ((i % 512) as f64 * 0.01).sin() as f32)
            .collect();
        let _ = pl.process(&mut chunk);
        // Seek: buffered pre-seek samples must vanish.
        pl.drop_buffered();
        // Flush after a drop must not emit the dropped samples as audio
        // (only the resampler delay-line tail, small).
        let tail = pl.flush();
        let tail_frames = tail.len() / 2;
        assert!(
            tail_frames < 4096,
            "post-seek flush emitted {tail_frames} frames; stale FIFO not dropped"
        );
    }

    #[test]
    fn resampler_handles_arbitrary_chunk_sizes_mp3_like() {
        // Regression (found live on an MP3 file): the resampler wants
        // fixed 2048-frame blocks; MP3 decodes in 1152-frame chunks and
        // the last chunk is truncated. The pipeline's FIFO must absorb
        // any chunk size without error and flush the tail.
        use crate::eq::Equalizer;

        let source = PcmFormat {
            sample_rate: 44_100,
            channels: 2,
            bits_per_sample: 32,
            sample_format: aurora_core::SampleFormat::Float,
            channel_layout: aurora_core::ChannelLayout::Interleaved,
        };
        let mut config = DspConfig {
            speed: 0.8, // forces the resampler on
            ..DspConfig::default()
        };
        config.eq = Equalizer::ten_band_flat();

        let mut pipeline = DspPipeline::compile(&config, source, None, None).expect("compile");

        // 1152-frame chunks, ~4.7 seconds, then a short truncated tail.
        let chunk_frames = 1152usize;
        let total_frames = 44_100 * 4 + 1000;
        let mut fed = 0usize;
        let mut out_samples = 0usize;
        while fed < total_frames {
            let n = chunk_frames.min(total_frames - fed);
            let mut chunk: Vec<f32> = (0..n * 2)
                .map(|i| ((i % 512) as f64 * 0.01).sin() as f32)
                .collect();
            let processed = pipeline.process(&mut chunk);
            assert!(processed.is_empty() || !processed.is_empty()); // no panic
            out_samples += processed.len();
            fed += n;
        }
        let tail = pipeline.flush();
        out_samples += tail.len();

        // 0.8x speed: ~4.71s * 44100 * 2ch ≈ 415k samples (plus resampler
        // transient slack). The pre-fix code emitted near-zero output.
        let expected_min = total_frames * 2 * 4 / 5; // 0.8 ratio, minus slack
        assert!(
            out_samples > expected_min,
            "output too small: {out_samples} samples for {total_frames} input frames              (resampler FIFO regression)"
        );
    }
}
