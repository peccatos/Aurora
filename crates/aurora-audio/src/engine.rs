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
}

impl PlaybackEngine {
    pub fn new() -> Self {
        Self {
            shared: Arc::new(Shared {
                running: Arc::new(AtomicBool::new(false)),
                finished: Arc::new(AtomicBool::new(false)),
            }),
            metrics: Arc::new(PlaybackMetrics::new()),
            state: Mutex::new(EngineState::Idle),
            stream: None,
            worker: None,
            negotiated: None,
            transformations: Vec::new(),
            device_key: None,
            source_format: None,
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

    /// Start playing a file on a device.
    pub fn play(&mut self, path: PathBuf, device: &OutputDevice) -> Result<(), EngineError> {
        if self.state() == EngineState::Playing {
            return Err(EngineError::AlreadyPlaying);
        }
        self.stop_internal();

        let cpal_device = find_by_key(device.key())?;
        let mut source = AudioSource::open(&path)?;
        let source_format = source.pcm_format();

        let negotiated = negotiate(&cpal_device, source_format)?;

        // Phase 3 plays the source representation when the device format
        // matches byte-wise; a mismatched negotiation is reported, and the
        // fallback path converts s16 sources through f32 (Phase 4 DSP will
        // own proper conversion; here it is a visible, reported transform).
        let plays_native = negotiated.transformations.is_empty();

        let ring = Arc::new(HeapRb::<u8>::new(RING_CAPACITY_BYTES));
        let (prod, cons) = ring.split();

        // Worker: decode and push into the ring until done or stopped.
        // The worker owns the producer; dropping it (in stop) closes writes.
        let worker_running = Arc::clone(&self.shared.running);
        let worker_finished = Arc::clone(&self.shared.finished);

        let worker = std::thread::Builder::new()
            .name("aurora-decode-worker".into())
            .spawn(move || {
                let mut prod = prod;
                let running = worker_running;
                loop {
                    if !running.load(Ordering::Acquire) {
                        break;
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
                                        // Buffer full: back-pressure by
                                        // yielding; the callback drains us.
                                        if !running.load(Ordering::Acquire) {
                                            break;
                                        }
                                        std::thread::yield_now();
                                    }
                                }
                            } else {
                                // Fallback: convert to f32 and push as f32
                                // bytes (the stream is F32 in this path).
                                let f32s = chunk.to_f32_interleaved();
                                let bytes = f32_bytes(&f32s);
                                let mut written = 0;
                                while written < bytes.len() {
                                    let n = prod.push_slice(&bytes[written..]);
                                    written += n;
                                    if n == 0 {
                                        if !running.load(Ordering::Acquire) {
                                            break;
                                        }
                                        std::thread::yield_now();
                                    }
                                }
                            }
                        }
                        Err(DecodeError::EndOfStream) => {
                            worker_finished.store(true, Ordering::Release);
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
        self.negotiated = Some(negotiated.pcm);
        self.transformations = negotiated.transformations;
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
    pub fn pause(&mut self) -> Result<(), EngineError> {
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
        if let Some(stream) = &self.stream {
            stream
                .play()
                .map_err(|e| EngineError::StreamBuild(e.to_string()))?;
            *self.state.lock().expect("state mutex") = EngineState::Playing;
            tracing::info!("playback resumed");
        }
        Ok(())
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
        self.negotiated = None;
        self.transformations.clear();
        self.device_key = None;
        self.source_format = None;
    }
}

impl Default for PlaybackEngine {
    fn default() -> Self {
        Self::new()
    }

    // Note: Default manually implemented; Drop is explicit via stop().
}

fn f32_bytes(samples: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 4);
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
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
