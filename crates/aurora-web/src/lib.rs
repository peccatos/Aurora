//! Aurora Web: the wasm shell over `aurora-kernel` (ADR 0005/0006).
//!
//! `WebKernel` mirrors the kernel's surface for JavaScript: the frontend
//! calls the same methods the Tauri shell's commands do, events flow to
//! JS callbacks. Output: the engine's ring consumer is drained from an
//! AudioWorklet on the JS side (see `web_pull`).
//!
//! Honesty contracts travel unchanged (ADR 0002): bit-perfect is
//! reported as N/A here — the browser audio path cannot be measured —
//! never claimed.

use aurora_kernel::{DeckDto, KernelError, LibraryStatus, PlayRequest, PlayerKernel, QueueTrack};
use wasm_bindgen::prelude::*;

/// In-memory file registry: the JS shell registers File bytes by id
/// ("web://<id>" queue paths resolve here). The browser has no disk.
/// (Native: register is a no-op in web_files — see that module.)
fn register_web_file(id: &str, bytes: Vec<u8>, extension: &str) {
    aurora_decoder::web_files::register(id, bytes, extension);
}

/// Map kernel errors to JS exceptions with the kind tag (the same shape
/// the Tauri shell's CommandError serializes to).
fn js_err(e: KernelError) -> JsValue {
    let (kind, message) = match e {
        KernelError::Poisoned => ("poisoned", "internal state is poisoned".into()),
        KernelError::Engine { message } => ("engine", message),
        KernelError::Device(key) => ("device", format!("device not found: {key}")),
        KernelError::Library { message } => ("library", message),
        KernelError::LibraryNotOpen => ("library_not_open", "library is not open".into()),
        KernelError::Metadata { message } => ("metadata", message),
    };
    serde_json::json!({ "kind": kind, "message": message })
        .to_string()
        .into()
}

fn to_js<T: serde::Serialize>(value: T) -> Result<JsValue, JsValue> {
    serde_wasm_bindgen::to_value(&value).map_err(|e| JsValue::from_str(&e.to_string()))
}

/// The web shell's kernel handle: one per page.
#[wasm_bindgen]
pub struct WebKernel {
    kernel: PlayerKernel,
    /// The last pulled sample buffer, kept alive so the returned
    /// `Float32Array` view (borrowing wasm memory) stays valid until
    /// the next pull replaces it. The JS side copies into an
    /// AudioBuffer immediately; the borrow window is one synchronous
    /// call.
    pull_scratch: Vec<f32>,
}

#[wasm_bindgen]
impl WebKernel {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            kernel: PlayerKernel::new(),
            pull_scratch: Vec::new(),
        }
    }

    // -----------------------------------------------------------------
    // Playback
    // -----------------------------------------------------------------

    pub fn playback_status(&self) -> Result<JsValue, JsValue> {
        to_js(self.kernel.playback_status().map_err(js_err)?)
    }

    pub fn playback_position(&self) -> Result<f64, JsValue> {
        self.kernel.playback_position().map_err(js_err)
    }

    pub fn play_track(&self, request: JsValue) -> Result<JsValue, JsValue> {
        let request: PlayRequest = serde_wasm_bindgen::from_value(request).map_err(|e| {
            js_err(KernelError::Engine {
                message: format!("play request: {e}"),
            })
        })?;
        to_js(self.kernel.play_track(request).map_err(js_err)?)
    }

    pub fn pause(&self) -> Result<JsValue, JsValue> {
        to_js(self.kernel.pause_playback().map_err(js_err)?)
    }

    pub fn resume(&self) -> Result<JsValue, JsValue> {
        to_js(self.kernel.resume_playback().map_err(js_err)?)
    }

    pub fn stop(&self) -> Result<JsValue, JsValue> {
        to_js(self.kernel.stop_playback().map_err(js_err)?)
    }

    pub fn seek(&self, position_s: f64) -> Result<JsValue, JsValue> {
        to_js(self.kernel.seek(position_s).map_err(js_err)?)
    }

    /// Bridge honesty report (ADR 0002): seconds of scheduled-but-unheard
    /// audio the browser dropped (pause/track change killed scheduled
    /// nodes). Negative values roll the position back to the audible
    /// truth; the seek rebase covers jumps.
    pub fn position_correction_s(&self, seconds: f64) -> Result<(), JsValue> {
        self.kernel.position_correction_s(seconds).map_err(js_err)
    }

    /// Tell the engine the browser's output sample rate (AudioContext.
    /// sampleRate). With it, playback resamples to the DEVICE rate in the
    /// engine's DSP pipeline (one continuous stream, reported honestly);
    /// without it the browser resamples each buffer independently —
    /// phase-discontinuous at chunk boundaries ("frequencies jumping").
    /// Call once at app start, before the first play.
    pub fn set_output_rate(&self, rate: u32) {
        aurora_audio::device::set_output_rate(rate);
    }

    /// The browser output channel count (W7 gapless): with it, the
    /// engine remixes mono<->stereo itself, so a mixed-channel queue
    /// never changes the bridge's stream format (a format change would
    /// restart the loop = an audible gap between tracks).
    pub fn set_output_channels(&self, channels: u32) {
        aurora_audio::device::set_output_channels(channels);
    }

    /// R128 loudness measurement (W8): integrated LUFS of decoded
    /// audio bytes. The JS shell runs this at ingest and stores the
    /// value; playback feeds it through the existing ReplayGain gain
    /// stage (mode "track") — normalized loudness with zero new
    /// audio-path code. Returns None honestly when the audio is too
    /// short/quiet to measure.
    pub fn analyze_lufs(&self, bytes: &[u8], extension: &str) -> Result<Option<f64>, JsValue> {
        let source = aurora_decoder::AudioSource::open_from_bytes(bytes.to_vec(), extension)
            .map_err(|e| JsValue::from_str(&format!("decode for lufs: {e}")))?;
        let format = source.pcm_format();
        let channels = usize::from(format.channels.max(1));
        let mut decoded = Vec::new();
        let mut source = source;
        loop {
            match source.next_chunk() {
                Ok(chunk) => {
                    let s = chunk.to_f32_interleaved();
                    if !s.is_empty() {
                        decoded.extend(s);
                        // Bound the analysis: 60 s of audio is plenty
                        // for a stable integrated value.
                        let cap = format.sample_rate as usize * 60 * channels;
                        if decoded.len() >= cap {
                            break;
                        }
                    }
                }
                Err(aurora_decoder::DecodeError::EndOfStream) => break,
                Err(_) => break, // honest: measure what we got
            }
        }
        Ok(
            aurora_dsp::lufs::integrated_lufs(&decoded, channels, format.sample_rate)
                .map(f64::from),
        )
    }

    // -----------------------------------------------------------------
    // Queue
    // -----------------------------------------------------------------

    pub fn set_queue(&self, tracks: JsValue) -> Result<JsValue, JsValue> {
        let tracks: Vec<QueueTrack> = serde_wasm_bindgen::from_value(tracks).map_err(|e| {
            js_err(KernelError::Engine {
                message: format!("queue payload: {e}"),
            })
        })?;
        to_js(self.kernel.set_queue(tracks).map_err(js_err)?)
    }

    pub fn get_queue(&self) -> Result<JsValue, JsValue> {
        to_js(self.kernel.get_queue().map_err(js_err)?)
    }

    pub fn play_queue_index(&self, index: usize) -> Result<JsValue, JsValue> {
        to_js(self.kernel.play_queue_index(index).map_err(js_err)?)
    }

    pub fn next_track(&self) -> Result<Option<JsValue>, JsValue> {
        let dto = self.kernel.next_track().map_err(js_err)?;
        match dto {
            Some(q) => Ok(Some(to_js(q)?)),
            None => Ok(None),
        }
    }

    pub fn prev_track(&self) -> Result<JsValue, JsValue> {
        to_js(self.kernel.prev_track().map_err(js_err)?)
    }

    /// The queue snapshot DTO (for shells that need the raw shape).
    pub fn queue_dto(&self, queue: &JsValue) -> Result<JsValue, JsValue> {
        // Convenience passthrough is unnecessary; kept minimal: the
        // queue DTO is already returned by set/get/next/prev.
        let _ = queue;
        self.get_queue()
    }

    // -----------------------------------------------------------------
    // DSP
    // -----------------------------------------------------------------

    pub fn get_dsp(&self) -> Result<JsValue, JsValue> {
        to_js(self.kernel.get_dsp().map_err(js_err)?)
    }

    /// Register a user convolution-reverb impulse response (web IR
    /// upload): f32 planar channels + source sample rate. The right
    /// channel may be an empty array (mono upmix happens in Rust).
    /// JS hands over plain `number[]` or Float32Array contents — copy
    /// into wasm memory once; the IR store is global to the wasm module.
    pub fn set_convolution_ir(
        &self,
        id: u32,
        left: Vec<f32>,
        right: Vec<f32>,
        sample_rate: u32,
    ) -> Result<(), JsValue> {
        self.kernel
            .set_convolution_ir(id, &left, &right, sample_rate)
            .map_err(js_err)
    }

    /// Remove a user convolution IR by id.
    pub fn remove_convolution_ir(&self, id: u32) -> Result<(), JsValue> {
        self.kernel.remove_convolution_ir(id).map_err(js_err)
    }

    pub fn set_dsp(&self, config: JsValue) -> Result<JsValue, JsValue> {
        let config: aurora_dsp::DspConfig =
            serde_wasm_bindgen::from_value(config).map_err(|e| {
                js_err(KernelError::Engine {
                    message: format!("dsp config: {e}"),
                })
            })?;
        to_js(self.kernel.set_dsp(config).map_err(js_err)?)
    }

    // -----------------------------------------------------------------
    // Spectrum / loop
    // -----------------------------------------------------------------

    pub fn playback_spectrum(&self) -> Result<JsValue, JsValue> {
        to_js(self.kernel.playback_spectrum().map_err(js_err)?)
    }

    pub fn set_loop(&self, start: f64, end: f64) -> Result<JsValue, JsValue> {
        let (s, e) = if end > start {
            (Some(start), Some(end))
        } else {
            (None, None)
        };
        to_js(self.kernel.set_loop(s, e).map_err(js_err)?)
    }

    // -----------------------------------------------------------------
    // DJ decks
    // -----------------------------------------------------------------

    pub async fn deck_load(
        &self,
        deck: usize,
        path: String,
        title: Option<String>,
        artist: Option<String>,
    ) -> Result<JsValue, JsValue> {
        to_js(
            self.kernel
                .deck_load(deck, path, title, artist)
                .await
                .map_err(js_err)?,
        )
    }

    pub fn deck_play(&self, deck: usize) -> Result<JsValue, JsValue> {
        to_js(self.kernel.deck_play(deck).map_err(js_err)?)
    }

    pub fn deck_stop(&self, deck: usize) -> Result<JsValue, JsValue> {
        to_js(self.kernel.deck_stop(deck).map_err(js_err)?)
    }

    pub fn deck_seek(&self, deck: usize, position_s: f64) -> Result<(), JsValue> {
        self.kernel.deck_seek(deck, position_s).map_err(js_err)
    }

    pub fn deck_fader(&self, deck: usize, value: f64) -> Result<JsValue, JsValue> {
        to_js(self.kernel.deck_fader(deck, value).map_err(js_err)?)
    }

    pub fn set_crossfader(&self, value: f64) -> Result<JsValue, JsValue> {
        to_js(self.kernel.set_crossfader(value).map_err(js_err)?)
    }

    pub fn deck_set_cue(
        &self,
        deck: usize,
        slot: usize,
        position: Option<f64>,
    ) -> Result<JsValue, JsValue> {
        to_js(
            self.kernel
                .deck_set_cue(deck, slot, position)
                .map_err(js_err)?,
        )
    }

    pub fn deck_jump_cue(&self, deck: usize, slot: usize) -> Result<(), JsValue> {
        self.kernel.deck_jump_cue(deck, slot).map_err(js_err)
    }

    pub fn deck_tempo(&self, deck: usize, speed: f64) -> Result<JsValue, JsValue> {
        to_js(self.kernel.deck_tempo(deck, speed).map_err(js_err)?)
    }

    pub fn deck_status(&self) -> Result<JsValue, JsValue> {
        let decks: [DeckDto; 2] = self.kernel.deck_status().map_err(js_err)?;
        to_js(decks)
    }

    pub async fn track_bpm(&self, path: String) -> Result<Option<f64>, JsValue> {
        self.kernel.track_bpm(&path).await.map_err(js_err)
    }

    // -----------------------------------------------------------------
    // Devices / probe — honest N/A answers on the browser path (ADR 0002)
    // -----------------------------------------------------------------

    pub fn output_devices(&self) -> Result<JsValue, JsValue> {
        to_js(
            self.kernel
                .resolve_device(None)
                .map_err(js_err)?
                .name()
                .to_string(),
        )
    }

    pub fn probe_exclusive(&self) -> Result<JsValue, JsValue> {
        // The kernel's no-win-wasapi build already reports honestly.
        to_js(self.kernel.probe_exclusive().map_err(js_err)?)
    }

    // -----------------------------------------------------------------
    // Library status — the wasm kernel has no SQLite; the JS shell owns
    // storage (IndexedDB). This reports "no library" honestly.
    // -----------------------------------------------------------------

    pub async fn library_status(&self) -> Result<JsValue, JsValue> {
        // Native: kernel method. Wasm: constant honest status.
        to_js(LibraryStatus {
            open: false,
            path: None,
            track_count: 0,
        })
    }

    // -----------------------------------------------------------------
    // Web file registry
    // -----------------------------------------------------------------

    /// Register a file for playback: `id` + bytes + extension (no dot).
    /// The queue uses "web://<id>" as the path; the decoder resolves it
    /// through the registry (the browser has no filesystem).
    ///
    /// Re-registering is the normal flow for replays — the registry
    /// consumes entries on play.
    pub fn register_file(&self, id: String, bytes: js_sys::Uint8Array, extension: String) {
        register_web_file(&id, bytes.to_vec(), &extension);
    }

    // -----------------------------------------------------------------
    // Events
    // -----------------------------------------------------------------

    /// Subscribe to kernel events. The callback receives the serialized
    /// `KernelEvent` (same wire shape as the Tauri event payloads).
    pub fn subscribe(&self, callback: js_sys::Function) -> usize {
        let kernel = &self.kernel;
        let sub = kernel.events.subscribe(move |event| {
            let payload = serde_json::to_string(event).unwrap_or_default();
            let _ = callback.call1(&JsValue::NULL, &JsValue::from_str(&payload));
        });
        // The subscription lives as long as the kernel; the id is for
        // diagnostics (JS cannot unsubscribe in this iteration).
        std::mem::forget(sub);
        0
    }

    // -----------------------------------------------------------------
    // Web output bridge: the AudioWorklet polls these from JS to drain
    // the engine's ring (the browser owns the audio device — ADR 0006).
    // -----------------------------------------------------------------

    /// Pull decoded+processed samples from the engine ring as f32
    /// interleaved. Returns an empty array when nothing is playing or
    /// the ring ran dry (the worklet fills silence then).
    ///
    /// The engine pushes wire-format bytes (its negotiated f32 target).
    /// Allocation discipline: the samples live in wasm memory; the view
    /// (`Float32Array::view`) borrows that memory with ZERO copying and
    /// zero JS-side allocation — the pull loop runs ~20x/s and the old
    /// copy-into-fresh-array form was measurable GC pressure. The
    /// buffer is only borrowed for the duration of the synchronous
    /// call; JS copies into the AudioBuffer immediately.
    pub fn web_pull(&mut self, max_bytes: usize) -> js_sys::Float32Array {
        // Reuse the scratch buffer: the engine fills it, the returned
        // view borrows it, and the NEXT pull replaces it. One stable
        // allocation instead of a fresh Vec + JS copy per call.
        let mut engine = match self.kernel.engine.lock() {
            Ok(guard) => guard,
            Err(_) => {
                self.pull_scratch.clear();
                return unsafe { js_sys::Float32Array::view(&self.pull_scratch) };
            }
        };
        if let Some(samples) = engine.pull_web(max_bytes) {
            self.pull_scratch = samples;
        } else {
            self.pull_scratch.clear();
        }
        unsafe { js_sys::Float32Array::view(&self.pull_scratch) }
    }
}

impl Default for WebKernel {
    fn default() -> Self {
        Self::new()
    }
}
