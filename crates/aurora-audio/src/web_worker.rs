//! Wasm cooperative decode worker.
//!
//! On wasm32-unknown-unknown there is no `std::thread`: `spawn` fails with
//! `io::ErrorKind::Unsupported` — the "internal engine failure: operation
//! not supported on this platform" the web shell reported. The decode loop
//! therefore runs cooperatively: `web_pull` (driven by the JS pull loop)
//! pumps the worker before reading the ring, so decoding advances exactly
//! as fast as the browser consumes audio.
//!
//! Back-pressure is implicit: `pump` returns once the ring holds enough
//! audio, so decode never outruns the output clock by more than the
//! ring's worth of audio. Where the native loop would sleep and retry,
//! the cooperative version simply returns — the next pull continues.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use aurora_core::PcmFormat;
use aurora_decoder::{AudioSource, DecodeError};
use ringbuf::HeapRb;
use ringbuf::traits::{Observer, Producer};
use ringbuf::wrap::caching::CachingProd;

use crate::engine::Shared;
use crate::metrics::PlaybackMetrics;

/// Decode-loop state the native worker keeps on its thread, owned here
/// and advanced one iteration at a time by [`WebDecodeWorker::pump`].
pub struct WebDecodeWorker {
    source: AudioSource,
    pipeline: Option<aurora_dsp::DspPipeline>,
    ditherer: aurora_dsp::TpdfDither,
    shared: Arc<Shared>,
    metrics: Arc<PlaybackMetrics>,
    prod: CachingProd<Arc<HeapRb<u8>>>,
    target_format: PcmFormat,
    compiled_version: u64,
    spectrum: crate::spectrum::SpectrumAnalyzer,
    /// True once the source hit end-of-stream and its tail was flushed.
    finished: bool,
}

impl WebDecodeWorker {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        source: AudioSource,
        pipeline: Option<aurora_dsp::DspPipeline>,
        ditherer: aurora_dsp::TpdfDither,
        shared: Arc<Shared>,
        metrics: Arc<PlaybackMetrics>,
        prod: CachingProd<Arc<HeapRb<u8>>>,
        target_format: PcmFormat,
    ) -> Self {
        let spectrum = crate::spectrum::SpectrumAnalyzer::new(target_format.sample_rate);
        // Start in sync with the LIVE dsp version: a fresh worker
        // starting at 0 would look like a config change on its first
        // pump, triggering a spurious recompile+drain that discards up
        // to a ring's worth of freshly decoded audio (audible skip,
        // position metrics racing ahead of the heard audio).
        let compiled_version = shared.dsp.lock().map(|d| d.version).unwrap_or(0);
        Self {
            source,
            pipeline,
            ditherer,
            shared,
            metrics,
            prod,
            target_format,
            compiled_version,
            spectrum,
            finished: false,
        }
    }

    fn prod_len(&self) -> usize {
        self.prod.occupied_len()
    }

    /// Run decode iterations until the ring holds at least
    /// `target_bytes` (or the stream ends/stops). Bounded by `max_iters`
    /// loop iterations so one pull can never monopolize the JS thread.
    pub fn pump(&mut self, target_bytes: usize, max_iters: usize) {
        for _ in 0..max_iters {
            if !self.shared.running.load(Ordering::Acquire) || self.finished {
                return;
            }
            if self.prod_len() >= target_bytes && !self.shared.paused.load(Ordering::Acquire) {
                return;
            }
            self.step();
        }
    }

    /// One iteration of the native worker loop, minus the blocking parts:
    /// where the native loop sleeps and retries, this returns — the next
    /// pump continues after the consumer drains.
    fn step(&mut self) {
        let shared = &self.shared;

        let paused = shared.paused.load(Ordering::Acquire);

        // Live DSP: recompile the pipeline when the config version
        // advanced. (ReplayGain tags are not carried on the web path.)
        let needs_recompile = match shared.dsp.lock() {
            Ok(d) => d.version != self.compiled_version,
            Err(_) => false,
        };
        if needs_recompile {
            let new_cfg = shared
                .dsp
                .lock()
                .ok()
                .map(|d| d.config.clone())
                .unwrap_or_default();
            let source_format = self.source.pcm_format();
            let rate_target = if self.target_format.sample_rate != source_format.sample_rate {
                Some(self.target_format.sample_rate)
            } else {
                None
            };
            let compiled =
                aurora_dsp::DspPipeline::compile(&new_cfg, source_format, rate_target, None).ok();
            self.compiled_version = shared.dsp.lock().map(|d| d.version).unwrap_or_default();
            if let Some(new_pl) = compiled {
                self.pipeline = Some(new_pl);
                shared.drain.store(true, Ordering::Release);
            }
        }

        // Seek: perform between chunks (no clicks beyond in-flight audio).
        let pending_seek = shared.seek.lock().ok().and_then(|mut s| s.take());
        if let Some(position_s) = pending_seek {
            let time =
                symphonia::core::units::Time::try_from_secs_f64(position_s).unwrap_or_default();
            if self
                .source
                .seek(symphonia::core::formats::SeekTo::Time {
                    time,
                    track_id: None,
                })
                .is_ok()
            {
                let played = self.metrics.frames_played();
                if let Ok(mut rb) = shared.seek_rebase.lock() {
                    *rb = Some((played, position_s));
                }
                if let Some(pl) = self.pipeline.as_mut() {
                    pl.drop_buffered();
                }
            }
            return;
        }

        if paused {
            return;
        }

        match self.source.next_chunk() {
            Ok(chunk) => {
                let bytes: Vec<u8> = if let Some(pipeline) = self.pipeline.as_mut() {
                    let mut samples = chunk.to_f32_interleaved();
                    let processed = pipeline.process(&mut samples);
                    self.spectrum
                        .push(&processed, usize::from(self.target_format.channels.max(1)));
                    let bands = self.spectrum.analyze();
                    self.metrics.publish_spectrum(bands);
                    let mut dithered = processed;
                    if !matches!(
                        self.target_format.sample_format,
                        aurora_core::SampleFormat::Float
                    ) {
                        self.ditherer.dither_for(
                            &mut dithered,
                            self.target_format.sample_format,
                            self.target_format.bits_per_sample,
                        );
                    }
                    wire_bytes(&dithered, self.target_format)
                } else {
                    // No pipeline (bit-exact analogue): the chunk's own
                    // wire bytes — the ring already carries the
                    // negotiated f32 format via pull_web's frame math.
                    chunk.as_bytes().to_vec()
                };
                let mut written = 0;
                while written < bytes.len() {
                    let n = self.prod.push_slice(&bytes[written..]);
                    written += n;
                    if n == 0 {
                        // Ring full: return, the next pump continues once
                        // the consumer drains. Never spin.
                        return;
                    }
                }
            }
            Err(DecodeError::EndOfStream) => {
                // Flush the pipeline tail so the last ~90ms plays out.
                if let Some(pipeline) = self.pipeline.as_mut() {
                    let tail = pipeline.flush();
                    if !tail.is_empty() {
                        let bytes = wire_bytes(&tail, self.target_format);
                        let mut written = 0;
                        while written < bytes.len() {
                            let n = self.prod.push_slice(&bytes[written..]);
                            written += n;
                            if n == 0 {
                                return;
                            }
                        }
                    }
                }
                self.finished = true;
                shared.finished.store(true, Ordering::Release);
                shared.track_ended.store(true, Ordering::Release);
            }
            Err(DecodeError::Recoverable(msg)) => {
                let _ = msg;
            }
            Err(_) => {
                self.finished = true;
                shared.finished.store(true, Ordering::Release);
            }
        }
    }
}

/// Encode normalized f32 samples into the target's wire bytes (same
/// encoding as the engine's `f32s_to_wire`).
fn wire_bytes(samples: &[f32], target: PcmFormat) -> Vec<u8> {
    let bytes = usize::from(target.bits_per_sample).div_ceil(8);
    let mut out = Vec::with_capacity(samples.len() * bytes);
    for &s in samples {
        let clamped = s.clamp(-1.0, 1.0);
        match target.sample_format {
            aurora_core::SampleFormat::SignedInt => {
                let full = (1u64 << (target.bits_per_sample - 1)) - 1;
                let v = (f64::from(clamped) * full as f64).round() as i64;
                let v = v.clamp(-(full as i64 + 1), full as i64) as u64;
                out.extend_from_slice(&v.to_le_bytes()[..bytes]);
            }
            aurora_core::SampleFormat::UnsignedInt => {
                let half = 1u64 << (target.bits_per_sample - 1);
                let v = (f64::from(clamped) * half as f64).round() as i64 + half as i64;
                let v = v.clamp(0, ((1u64 << target.bits_per_sample) - 1) as i64) as u64;
                out.extend_from_slice(&v.to_le_bytes()[..bytes]);
            }
            aurora_core::SampleFormat::Float => {
                if target.bits_per_sample == 64 {
                    out.extend_from_slice(&f64::from(clamped).to_le_bytes());
                } else {
                    out.extend_from_slice(&clamped.to_le_bytes());
                }
            }
        }
    }
    out
}
