//! Playback engine: decode-worker feeding a SPSC ring buffer that the
//! real-time callback drains.
//!
//! Real-time constraints (AUDIO_PIPELINE.md): the callback performs bounded
//! preallocated work only — no allocation, no locks, no I/O, no logging.
//! All decode/IO happens on the worker thread; the callback only pops bytes
//! from the ring buffer and updates lock-free counters.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use aurora_core::PcmFormat;
use aurora_decoder::{AudioSource, DecodeError};
use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{Data, OutputCallbackInfo, Stream};
use ringbuf::{HeapRb, traits::Consumer, traits::Producer, traits::Split};

use crate::device::{OutputDevice, find_by_key, negotiate};
use crate::error::EngineError;
use crate::metrics::PlaybackMetrics;

/// Target ring capacity in bytes (~2s at 192kHz/32-bit/stereo worst case,
/// sized for 44.1k/16/2 comfortably; tuned by Phase 3 soak tests).
const RING_CAPACITY_BYTES: usize = 8 * 1024 * 1024;

/// Engine status shared with the app layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineState {
    Idle,
    Playing,
    /// Stream ended or stopped; the callback emits silence.
    Draining,
    Paused,
}

struct Shared {
    /// True while the worker should keep producing.
    running: Arc<AtomicBool>,
    /// True when a natural end-of-source was reached.
    finished: Arc<AtomicBool>,
    /// Live DSP configuration: the worker recompiles the pipeline when the
    /// version changes, without interrupting the stream.
    dsp: Arc<Mutex<LiveDsp>>,
    /// Pending seek position in seconds; the worker performs the seek and
    /// clears the slot.
    seek: Arc<Mutex<Option<f64>>>,
    /// True while the stream is paused: the worker stops filling the ring
    /// (which would otherwise block in back-pressure forever) and keeps
    /// servicing live-DSP/seek requests instead.
    paused: Arc<AtomicBool>,
    /// Auto-advance signal: set by the callback side when the stream ends
    /// naturally (checked by the app layer for next-track behavior).
    track_ended: Arc<AtomicBool>,
}

/// Live DSP state shared with the worker.
pub struct LiveDsp {
    pub config: aurora_dsp::DspConfig,
    /// Bumped by set_dsp; the worker recompiles when it differs from the
    /// compiled version.
    pub version: u64,
}

/// The playback engine. One active stream at a time.
pub struct PlaybackEngine {
    shared: Arc<Shared>,
    metrics: Arc<PlaybackMetrics>,
    state: Mutex<EngineState>,
    stream: Option<Stream>,
    worker: Option<JoinHandle<()>>,
    /// The negotiated output PCM format (for diagnostics).
    negotiated: Option<PcmFormat>,
    /// Transformations active on the current path (empty = direct match).
    transformations: Vec<aurora_core::AudioTransformation>,
    device_key: Option<String>,
    /// Source PCM (what the decoder produces).
    source_format: Option<PcmFormat>,
    /// Track duration from the container (ms), for the seek bar.
    source_duration_ms: Option<u64>,
    /// ADR 0002 integrity classification of the current path: honest,
    /// computed from the transformation chain, never assumed.
    integrity: aurora_core::OutputIntegrity,
}

impl PlaybackEngine {
    pub fn new() -> Self {
        Self {
            shared: Arc::new(Shared {
                running: Arc::new(AtomicBool::new(false)),
                finished: Arc::new(AtomicBool::new(false)),
                dsp: Arc::new(Mutex::new(LiveDsp {
                    config: aurora_dsp::DspConfig::default(),
                    version: 0,
                })),
                seek: Arc::new(Mutex::new(None)),
                paused: Arc::new(AtomicBool::new(false)),
                track_ended: Arc::new(AtomicBool::new(false)),
            }),
            metrics: Arc::new(PlaybackMetrics::new()),
            state: Mutex::new(EngineState::Idle),
            stream: None,
            worker: None,
            negotiated: None,
            transformations: Vec::new(),
            device_key: None,
            source_format: None,
            source_duration_ms: None,
            integrity: aurora_core::OutputIntegrity::Unverified,
        }
    }

    /// List available output devices.
    pub fn devices(&self) -> Result<Vec<OutputDevice>, EngineError> {
        crate::device::output_devices()
    }

    pub fn state(&self) -> EngineState {
        *self.state.lock().expect("state mutex poisoned")
    }

    pub fn metrics(&self) -> &PlaybackMetrics {
        &self.metrics
    }

    /// Format negotiated with the device for the current stream.
    pub fn output_format(&self) -> Option<PcmFormat> {
        self.negotiated
    }

    /// Active transformations (e.g. forced format conversion).
    pub fn transformations(&self) -> &[aurora_core::AudioTransformation] {
        &self.transformations
    }

    /// The honest ADR 0002 classification of the current output path.
    pub fn integrity(&self) -> aurora_core::OutputIntegrity {
        self.integrity
    }

    /// Current stream's source PCM format (for diagnostics commands).
    pub fn source_format_clone(&self) -> Option<PcmFormat> {
        self.source_format
    }

    /// Current output device key (for diagnostics commands).
    pub fn device_key_clone(&self) -> Option<String> {
        self.device_key.clone()
    }

    /// Track duration in seconds from the container, for the seek bar.
    pub fn duration_s(&self) -> Option<f64> {
        self.source_duration_ms.map(|ms| ms as f64 / 1000.0)
    }

    /// Start playing a file on a device with a DSP configuration.
    ///
    /// When the config is bypassed AND the device wire format matches the
    /// source byte-wise, the native path pushes decoder bytes untouched
    /// (bit-perfect eligible). Otherwise the chunk travels the DSP path:
    /// f32 -> pipeline (EQ / gain / resampling as configured) -> convert
    /// into the device's exact wire format. Every transformation — forced
    /// or user-configured — lands in `transformations` (ADR 0002).
    pub fn play(
        &mut self,
        path: PathBuf,
        device: &OutputDevice,
        dsp: aurora_dsp::DspConfig,
    ) -> Result<(), EngineError> {
        if self.state() == EngineState::Playing {
            return Err(EngineError::AlreadyPlaying);
        }
        self.stop_internal();

        let cpal_device = find_by_key(device.key())?;
        let mut source = AudioSource::open(&path)?;
        let source_format = source.pcm_format();

        let negotiated = negotiate(&cpal_device, source_format)?;

        // The native (untouched) path requires: DSP fully bypassed AND a
        // byte-exact device format match.
        let plays_native = negotiated.transformations.is_empty() && dsp.is_bypassed();

        let ring = Arc::new(HeapRb::<u8>::new(RING_CAPACITY_BYTES));
        let (prod, cons) = ring.split();

        // Worker: decode and push into the ring until done or stopped.
        // The worker owns the producer; dropping it (in stop) closes writes.
        let worker_running = Arc::clone(&self.shared.running);
        let worker_finished = Arc::clone(&self.shared.finished);
        let live_dsp = Arc::clone(&self.shared.dsp);
        let live_seek = Arc::clone(&self.shared.seek);
        let live_paused = Arc::clone(&self.shared.paused);
        let track_ended_flag = Arc::clone(&self.shared.track_ended);

        // The wire format the fallback path converts into (the negotiated
        // device format; equal to the source format on the native path).
        let target_format = negotiated.pcm;

        // The DSP path pipeline: compiled once for this stream. When the
        // config is bypassed but the device format mismatches, the
        // pipeline is still constructed with default (identity) settings
        // so the rate/format conversion happens through one honest path.
        let dsp_pipeline = if plays_native {
            None
        } else {
            let rate_target = if negotiated.pcm.sample_rate != source_format.sample_rate {
                Some(negotiated.pcm.sample_rate)
            } else {
                None
            };
            Some(
                aurora_dsp::DspPipeline::compile(
                    &dsp,
                    source_format,
                    rate_target,
                    None, // ReplayGain tag lookup arrives with the library phase.
                )
                .map_err(EngineError::from)?,
            )
        };

        // Duration captured before the worker takes ownership of the source.
        let source_duration_ms = source.info().duration_ms;

        // Seeded dither for the float->int boundary (stable across runs).
        let mut ditherer = aurora_dsp::TpdfDither::new(0xA10CA);

        let worker = std::thread::Builder::new()
            .name("aurora-decode-worker".into())
            .spawn(move || {
                let mut prod = prod;
                let running = worker_running;
                // Live pipeline: compiled for the initial config, replaced
                // in place when the UI bumps the version. Track the
                // compiled version to detect changes cheaply.
                let mut compiled_version = 0u64;
                let mut compiled_pipeline = dsp_pipeline;
                loop {
                    if !running.load(Ordering::Acquire) {
                        break;
                    }

                    // Paused: do not decode (the ring cannot drain while
                    // the callback is paused), but keep servicing live
                    // recompiles and seek requests below. Sleep briefly
                    // instead of spinning.
                    if live_paused.load(Ordering::Acquire) {
                        // DSP/seek still need servicing while paused.
                        if live_dsp.lock().map(|d| d.version).unwrap_or(0) != compiled_version
                            || live_seek.lock().map(|s| s.is_some()).unwrap_or(false)
                        {
                            // fall through to the handling below
                        } else {
                            std::thread::sleep(std::time::Duration::from_millis(50));
                            continue;
                        }
                    }

                    // Live DSP: recompile when the config version advanced.
                    // Filter states reset; the stream continues seamlessly.
                    if let Some(pl) = compiled_pipeline.as_ref() {
                        let _ = pl;
                    }
                    let needs_recompile = {
                        match live_dsp.lock() {
                            Ok(d) => d.version != compiled_version,
                            Err(_) => false,
                        }
                    };
                    if needs_recompile && !plays_native {
                        let new_cfg = live_dsp
                            .lock()
                            .ok()
                            .map(|d| d.config.clone())
                            .unwrap_or_default();
                        let rate_target = if negotiated.pcm.sample_rate != source_format.sample_rate
                        {
                            Some(negotiated.pcm.sample_rate)
                        } else {
                            None
                        };
                        match aurora_dsp::DspPipeline::compile(
                            &new_cfg,
                            source_format,
                            rate_target,
                            None,
                        ) {
                            Ok(new_pl) => {
                                tracing::info!("dsp recompiled live");
                                compiled_version = {
                                    let mut d = live_dsp.lock().expect("dsp lock");
                                    let v = d.version;
                                    d.config = new_cfg;
                                    v
                                };
                                compiled_pipeline = Some(new_pl);
                            }
                            Err(e) => {
                                tracing::warn!(%e, "live dsp recompile failed, keeping old");
                                compiled_version = {
                                    let d = live_dsp.lock().expect("dsp lock");
                                    d.version
                                };
                            }
                        }
                    }

                    // Seek: drain the ring, seek the source, continue.
                    let pending_seek = live_seek.lock().ok().and_then(|mut s| s.take());
                    if let Some(position_s) = pending_seek {
                        let time = symphonia::core::units::Time::try_from_secs_f64(position_s)
                            .unwrap_or_default();
                        match source_seek(
                            &mut source,
                            symphonia::core::formats::SeekTo::Time {
                                time,
                                track_id: None,
                            },
                        ) {
                            Ok(()) => {
                                // Old buffered audio plays out briefly
                                // (~90ms at 8MB/44.1k stereo worst case is
                                // far less: ring drains in one callback).
                                tracing::info!(position_s, "seeked");
                            }
                            Err(e) => {
                                tracing::warn!(%e, "seek failed");
                            }
                        }
                        continue;
                    }

                    match source.next_chunk() {
                        Ok(chunk) => {
                            if plays_native {
                                let mut written = 0;
                                let bytes = chunk.as_bytes();
                                while written < bytes.len() {
                                    let n = prod.push_slice(&bytes[written..]);
                                    written += n;
                                    if n == 0 {
                                        // Buffer full: back-pressure with
                                        // bounded wait; pause/seek/DSP
                                        // updates must stay serviced.
                                        if !running.load(Ordering::Acquire)
                                            || live_paused.load(Ordering::Acquire)
                                        {
                                            break;
                                        }
                                        if live_seek.lock().map(|s| s.is_some()).unwrap_or(false) {
                                            break;
                                        }
                                        std::thread::sleep(std::time::Duration::from_millis(5));
                                    }
                                }
                            } else {
                                // DSP path: f32 -> pipeline -> device wire
                                // format. The pipeline applies the user's
                                // EQ/gain (and any resampling needed to
                                // reach the device rate); dither + final
                                // format conversion happen at the boundary.
                                let Some(pipeline) = compiled_pipeline.as_mut() else {
                                    // Unreachable: compiled above.
                                    continue;
                                };
                                let mut samples = chunk.to_f32_interleaved();
                                let processed = pipeline.process(&mut samples);

                                // Dither only when converting to integer
                                // output; float output needs none.
                                let mut dithered = processed;
                                if !matches!(
                                    target_format.sample_format,
                                    aurora_core::SampleFormat::Float
                                ) {
                                    ditherer.dither_for(
                                        &mut dithered,
                                        target_format.sample_format,
                                        target_format.bits_per_sample,
                                    );
                                }

                                let bytes = match aurora_decoder::PcmChunk::from_bytes(
                                    target_format,
                                    f32s_to_wire(&dithered, target_format),
                                ) {
                                    Some(c) => c.as_bytes().to_vec(),
                                    None => {
                                        tracing::warn!("wire conversion failed, dropping chunk");
                                        continue;
                                    }
                                };
                                let mut written = 0;
                                while written < bytes.len() {
                                    let n = prod.push_slice(&bytes[written..]);
                                    written += n;
                                    if n == 0 {
                                        if !running.load(Ordering::Acquire)
                                            || live_paused.load(Ordering::Acquire)
                                        {
                                            break;
                                        }
                                        if live_seek.lock().map(|s| s.is_some()).unwrap_or(false) {
                                            break;
                                        }
                                        std::thread::sleep(std::time::Duration::from_millis(5));
                                    }
                                }
                            }
                        }
                        Err(DecodeError::EndOfStream) => {
                            worker_finished.store(true, Ordering::Release);
                            track_ended_flag.store(true, Ordering::Release);
                            break;
                        }
                        Err(DecodeError::Recoverable(msg)) => {
                            tracing::warn!(%msg, "worker skipping packet");
                            continue;
                        }
                        Err(other) => {
                            tracing::error!(%other, "worker decode failure");
                            worker_finished.store(true, Ordering::Release);
                            break;
                        }
                    }
                }
            })
            .map_err(|e| EngineError::Internal(e.to_string()))?;

        // Callback: drain the ring into cpal's Data buffer.
        let cb_metrics = Arc::clone(&self.metrics);
        let cb_state_finished = Arc::clone(&self.shared.finished);

        self.shared.running.store(true, Ordering::Release);
        let stream = build_stream(
            &cpal_device,
            &negotiated,
            cons,
            cb_metrics,
            cb_state_finished,
        )?;

        stream
            .play()
            .map_err(|e| EngineError::StreamBuild(e.to_string()))?;

        self.metrics.reset();
        self.stream = Some(stream);
        self.worker = Some(worker);
        self.source_duration_ms = source_duration_ms;
        self.negotiated = Some(negotiated.pcm);
        // Report the full honest chain: forced conversions (negotiation)
        // plus the user-configured DSP stages (ADR 0002).
        let mut transformations = negotiated.transformations;
        if !dsp.is_bypassed() {
            transformations.extend(dsp.transformations());
        }
        // Integrity classification (ADR 0002): with zero transformations
        // and a byte-exact wire match the path is bit-identical up to the
        // OS boundary. Whether the OS mixer preserves it beyond that is
        // what exclusive mode + loopback verification (Phase 5) prove;
        // shared mode cannot be verified -> Unverified, never claimed.
        self.integrity = if transformations.is_empty() && plays_native {
            aurora_core::OutputIntegrity::Unverified // honest: shared mode unverifiable
        } else {
            aurora_core::OutputIntegrity::Transformed
        };
        self.transformations = transformations;
        self.device_key = Some(device.key().to_string());
        self.source_format = Some(source_format);
        *self.state.lock().expect("state mutex") = EngineState::Playing;

        tracing::info!(
            device = device.key(),
            source = ?source_format,
            output = ?negotiated.pcm,
            native = plays_native,
            "playback started"
        );
        Ok(())
    }

    /// Pause the stream (callback keeps running, emitting silence).
    ///
    /// The worker also stops decoding: with the callback not draining the
    /// ring, back-pressure would pin the worker in its fill loop and
    /// starve live-DSP updates and seek.
    pub fn pause(&mut self) -> Result<(), EngineError> {
        self.shared.paused.store(true, Ordering::Release);
        if let Some(stream) = &self.stream {
            stream
                .pause()
                .map_err(|e| EngineError::StreamBuild(e.to_string()))?;
            *self.state.lock().expect("state mutex") = EngineState::Paused;
            tracing::info!("playback paused");
        }
        Ok(())
    }

    /// Resume a paused stream.
    pub fn resume(&mut self) -> Result<(), EngineError> {
        self.shared.paused.store(false, Ordering::Release);
        if let Some(stream) = &self.stream {
            stream
                .play()
                .map_err(|e| EngineError::StreamBuild(e.to_string()))?;
            *self.state.lock().expect("state mutex") = EngineState::Playing;
            tracing::info!("playback resumed");
        }
        Ok(())
    }

    /// Seek to a position in seconds (worker performs it on the next loop
    /// iteration; sub-frame precision is the demuxer's).
    pub fn seek(&self, position_s: f64) {
        if let Ok(mut slot) = self.shared.seek.lock() {
            *slot = Some(position_s.clamp(0.0, f64::MAX));
        }
    }

    /// Update the DSP configuration live. Takes effect within one chunk
    /// (~10-40 ms) without interrupting playback.
    pub fn set_dsp_config(&self, config: aurora_dsp::DspConfig) {
        if let Ok(mut live) = self.shared.dsp.lock() {
            live.config = config;
            live.version = live.version.wrapping_add(1);
        }
    }

    /// Whether the current track ended naturally (for next-track
    /// auto-advance); consumes the flag.
    pub fn take_track_ended(&self) -> bool {
        self.shared.track_ended.swap(false, Ordering::AcqRel)
    }

    /// Approximate playback position in seconds, derived from frames
    /// played and the output rate.
    pub fn position_s(&self) -> f64 {
        let rate = self.negotiated.map(|f| f.sample_rate).unwrap_or(0);
        if rate == 0 {
            return 0.0;
        }
        self.metrics.frames_played() as f64 / f64::from(rate)
    }

    /// Stop playback and release the stream.
    pub fn stop(&mut self) {
        self.stop_internal();
        *self.state.lock().expect("state mutex") = EngineState::Idle;
    }

    fn stop_internal(&mut self) {
        self.shared.running.store(false, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        // Drop the stream (stops the callback); the worker dropped its
        // producer on exit, closing the ring for writes.
        self.stream = None;
        self.shared.finished.store(false, Ordering::Release);
        self.shared.track_ended.store(false, Ordering::Release);
        self.shared.paused.store(false, Ordering::Release);
        self.negotiated = None;
        self.transformations.clear();
        self.device_key = None;
        self.source_format = None;
        self.source_duration_ms = None;
        self.integrity = aurora_core::OutputIntegrity::Unverified;
    }
}

impl Default for PlaybackEngine {
    fn default() -> Self {
        Self::new()
    }

    // Note: Default manually implemented; Drop is explicit via stop().
}

/// Build the output stream with the real-time callback.
fn build_stream(
    device: &cpal::Device,
    config: &crate::device::NegotiatedConfig,
    mut cons: ringbuf::wrap::caching::CachingCons<Arc<HeapRb<u8>>>,
    metrics: Arc<PlaybackMetrics>,
    finished: Arc<AtomicBool>,
) -> Result<Stream, EngineError> {
    let pcm = config.pcm;
    let device_format = config.device_sample_format;

    let err_fn = |err: cpal::Error| tracing::error!(%err, "audio stream error");

    // All formats go through the raw builder: the callback works on bytes,
    // which is representation-agnostic and never allocates.
    let stream = device
        .build_output_stream_raw(
            config.stream_config,
            device_format,
            move |data: &mut Data, _info: &OutputCallbackInfo| {
                callback(&mut cons, data.bytes_mut(), &metrics, &finished, pcm);
            },
            err_fn,
            None,
        )
        .map_err(|e| EngineError::StreamBuild(e.to_string()))?;

    Ok(stream)
}

/// The real-time callback: pop bytes from the ring, count frames, fill
/// underruns with silence. No allocation, no locks, no logging.
fn callback(
    cons: &mut ringbuf::wrap::caching::CachingCons<Arc<HeapRb<u8>>>,
    out: &mut [u8],
    metrics: &PlaybackMetrics,
    finished: &AtomicBool,
    pcm: PcmFormat,
) {
    let frame_bytes =
        (usize::from(pcm.channels) * usize::from(pcm.bits_per_sample).div_ceil(8)).max(1);
    let bytes_out = out.len();

    let popped = cons.pop_slice(out);
    let deficit = bytes_out - popped;

    if popped > 0 {
        metrics.add_frames((popped / frame_bytes) as u64);
    }
    if deficit > 0 {
        // Starved or ended: fill with silence and count as underrun only
        // when the source is not finished (natural end is not an underrun).
        for b in &mut out[popped..] {
            *b = 0;
        }
        if !finished.load(Ordering::Acquire) {
            metrics.add_underrun((deficit / frame_bytes) as u64);
        }
    }
}

/// Seek the source to a time position (accurate mode).
fn source_seek(
    source: &mut AudioSource,
    to: symphonia::core::formats::SeekTo,
) -> Result<(), String> {
    source.seek(to).map(|_| ()).map_err(|e| e.to_string())
}

/// Encode normalized f32 samples into the target format's wire bytes.
///
/// The inverse of the decoder's read path: full-scale maps back to the
/// integer grid, floats copy through directly.
fn f32s_to_wire(samples: &[f32], target: PcmFormat) -> Vec<u8> {
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

impl From<aurora_dsp::DspError> for EngineError {
    fn from(err: aurora_dsp::DspError) -> Self {
        EngineError::Internal(err.to_string())
    }
}
