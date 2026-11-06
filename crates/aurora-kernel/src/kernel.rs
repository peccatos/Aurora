//! `PlayerKernel`: the music player, shell-free (ADR 0005).
//!
//! Everything a shell (Tauri desktop, web, third-party player) does to
//! music goes through methods on this struct. The kernel owns:
//! the main engine, the DJ decks + crossfader, the queue, DSP, the
//! VST chain, the library, output device selection and settings.
//!
//! Honesty contracts (ADR 0002) live here: bit-perfect verdicts and
//! transformation reports are kernel properties; a shell can only
//! display them.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use aurora_audio::{EngineState, PlaybackEngine};

use crate::decks::Deck;
use crate::dtos::{
    BitPerfectVerdict, DeckDto, ExclusivePlayResult, ExclusiveProbe, PlayRequest, PlaybackStatus,
    QueueDto, VstSlotDto,
};
// Storage-backed DTOs: the wasm kernel has no SQLite, these services
// are native-only — import them only where the code is compiled.
#[cfg(not(target_family = "wasm"))]
use crate::dtos::{ImportReport, LibraryStatus, TrackDto};
use crate::error::KernelError;
use crate::events::EventSubscribers;
use crate::queue::{QueueTrack, TrackQueue};

/// Managed application state (the former `AppState`, shell-free).
pub struct PlayerKernel {
    /// Event subscribers: shells listen here for kernel events.
    pub events: Arc<EventSubscribers>,
    pub engine: Mutex<PlaybackEngine>,
    /// Persisted DSP configuration: the UI edits this and every new
    /// playback picks it up.
    pub dsp: Mutex<aurora_dsp::DspConfig>,
    pub track_dsp: Mutex<crate::dsp::DspProfileResolver>,
    /// Playback queue with the current index.
    pub queue: Mutex<TrackQueue>,
    /// Track library (SQLite + scanner). None until `open_library` runs.
    /// Native only: the wasm build has no SQLite (JS shell owns storage).
    #[cfg(not(target_family = "wasm"))]
    pub library: tokio::sync::Mutex<Option<aurora_library::LibraryService>>,
    /// Library database path (None = library not opened yet).
    pub library_path_slot: Mutex<Option<PathBuf>>,
    /// Selected output device key ("" = default). Persisted to settings.
    pub device_key: Mutex<Option<String>>,
    /// Open plugin editor GUI session (one at a time; the window owns
    /// its own GUI thread with a Win32 message pump). Native only.
    #[cfg(not(target_family = "wasm"))]
    pub vst_gui: Mutex<Option<aurora_vst::PluginGui>>,
    /// DJ decks (Pioneer DDJ-1000-style): each deck is a fully
    /// independent playback stream — own engine, own DSP, own cue
    /// points. Deck 0 doubles as the main player (queue commands target
    /// it), deck 1 exists for the /decks page. Crossfader: 0 = full A,
    /// 1 = full B, equal-power curve applied to per-deck channel gain.
    pub decks: Mutex<[Deck; 2]>,
    /// Crossfader position 0..1 (0.5 = both decks equally loud).
    pub crossfader: Mutex<f64>,
}

impl Default for PlayerKernel {
    fn default() -> Self {
        Self::new()
    }
}

impl PlayerKernel {
    pub fn new() -> Self {
        Self {
            events: Arc::new(EventSubscribers::new()),
            engine: Mutex::new(PlaybackEngine::new()),
            dsp: Mutex::new(aurora_dsp::DspConfig::default()),
            track_dsp: Mutex::new(crate::dsp::DspProfileResolver::default()),
            queue: Mutex::new(TrackQueue::default()),
            #[cfg(not(target_family = "wasm"))]
            library: tokio::sync::Mutex::new(None),
            library_path_slot: Mutex::new(None),
            device_key: Mutex::new(None),
            #[cfg(not(target_family = "wasm"))]
            vst_gui: Mutex::new(None),
            decks: Mutex::new([Deck::new(), Deck::new()]),
            crossfader: Mutex::new(0.0),
        }
    }

    // -----------------------------------------------------------------
    // Playback
    // -----------------------------------------------------------------

    /// Status snapshot (one-shot faults consumed on read, ADR 0002 fields).
    pub fn status_of(&self, engine: &PlaybackEngine) -> PlaybackStatus {
        let format = engine.output_format();
        let fault = if engine.take_stream_error() {
            Some("output stream error (device lost?)".to_string())
        } else if engine.take_worker_panicked() {
            Some("decode worker crashed; playback stopped".to_string())
        } else {
            None
        };
        PlaybackStatus {
            state: engine.state(),
            fault,
            sample_rate: format.map(|f| f.sample_rate),
            channels: format.map(|f| f.channels),
            bits_per_sample: format.map(|f| f.bits_per_sample),
            transformations: engine.transformations().to_vec(),
            integrity: engine.integrity(),
            duration_s: engine.duration_s(),
            frames_played: engine.metrics().frames_played(),
            underruns: engine.metrics().underruns(),
        }
    }

    pub fn playback_status(&self) -> Result<PlaybackStatus, KernelError> {
        let engine = self.engine.lock().map_err(|_| KernelError::Poisoned)?;
        Ok(self.status_of(&engine))
    }

    pub fn playback_position(&self) -> Result<f64, KernelError> {
        let engine = self.engine.lock().map_err(|_| KernelError::Poisoned)?;
        Ok(engine.position_s())
    }

    /// Wasm bridge honesty (ADR 0002): report seconds of scheduled-but-
    /// unheard audio the browser dropped (pause/track change). The
    /// engine subtracts them so the reported position matches what was
    /// audible. Web-only in practice; native no-op (the stream clock
    /// never runs ahead of the device).
    pub fn position_correction_s(&self, seconds: f64) -> Result<(), KernelError> {
        let mut engine = self.engine.lock().map_err(|_| KernelError::Poisoned)?;
        engine.position_correction_s(seconds);
        Ok(())
    }

    pub fn pause_playback(&self) -> Result<PlaybackStatus, KernelError> {
        let mut engine = self.engine.lock().map_err(|_| KernelError::Poisoned)?;
        engine.pause()?;
        Ok(self.status_of(&engine))
    }

    pub fn resume_playback(&self) -> Result<PlaybackStatus, KernelError> {
        let mut engine = self.engine.lock().map_err(|_| KernelError::Poisoned)?;
        engine.resume()?;
        Ok(self.status_of(&engine))
    }

    pub fn stop_playback(&self) -> Result<PlaybackStatus, KernelError> {
        let mut engine = self.engine.lock().map_err(|_| KernelError::Poisoned)?;
        engine.stop();
        Ok(self.status_of(&engine))
    }

    pub fn seek(&self, position_s: f64) -> Result<PlaybackStatus, KernelError> {
        let mut engine = self.engine.lock().map_err(|_| KernelError::Poisoned)?;
        engine.seek(position_s);
        Ok(self.status_of(&engine))
    }

    /// Play a track (DSP override per request; persisted config when default).
    pub fn play_track(&self, request: PlayRequest) -> Result<PlaybackStatus, KernelError> {
        let device = self.resolve_device(request.device.as_deref())?;
        let dsp = if request.dsp == aurora_dsp::DspConfig::default() {
            let global = self.dsp.lock().map_err(|_| KernelError::Poisoned)?.clone();
            self.track_dsp
                .lock()
                .map_err(|_| KernelError::Poisoned)?
                .resolve(&request.path, &global)
                .clone()
        } else {
            request.dsp
        };
        let rg = request
            .replaygain_track_db
            .map(|t| (t, request.replaygain_album_db));
        let mut engine = self.engine.lock().map_err(|_| KernelError::Poisoned)?;
        engine.play_with_replaygain(PathBuf::from(&request.path), &device, dsp, rg)?;
        Ok(self.status_of(&engine))
    }

    // -----------------------------------------------------------------
    // Queue
    // -----------------------------------------------------------------

    pub fn set_queue(&self, tracks: Vec<QueueTrack>) -> Result<QueueDto, KernelError> {
        let mut queue = self.queue.lock().map_err(|_| KernelError::Poisoned)?;
        let keep = queue.current.filter(|&c| c < tracks.len());
        queue.tracks = tracks;
        // Preserve the current index when it still points INTO the new
        // list: add/reorder flows mutate the queue mid-playback, and
        // losing `current` there broke auto-advance (advance() from
        // None returns None -> stop) and the UI's now-playing pointer.
        // Replacing the whole queue with a fresh list (a NEW play flow)
        // passes current=None and resets as before.
        queue.current = keep;
        Ok(self.queue_dto(&queue))
    }

    /// Replace the queue unconditionally: current resets to None (the
    /// strict old set_queue semantics for flows that swap the queue).
    pub fn reset_queue(&self, tracks: Vec<QueueTrack>) -> Result<QueueDto, KernelError> {
        let mut queue = self.queue.lock().map_err(|_| KernelError::Poisoned)?;
        queue.tracks = tracks;
        queue.current = None;
        Ok(self.queue_dto(&queue))
    }

    pub fn get_queue(&self) -> Result<QueueDto, KernelError> {
        let queue = self.queue.lock().map_err(|_| KernelError::Poisoned)?;
        Ok(self.queue_dto(&queue))
    }

    pub fn queue_dto(&self, queue: &TrackQueue) -> QueueDto {
        QueueDto {
            display: queue.tracks.iter().map(|t| t.display_line()).collect(),
            tracks: queue.tracks.clone(),
            current: queue.current,
        }
    }

    fn start_track_at(&self, track: &QueueTrack) -> Result<PlaybackStatus, KernelError> {
        let device = self.resolve_device(
            self.device_key
                .lock()
                .map_err(|_| KernelError::Poisoned)?
                .as_deref(),
        )?;
        let global = self.dsp.lock().map_err(|_| KernelError::Poisoned)?.clone();
        let dsp = self
            .track_dsp
            .lock()
            .map_err(|_| KernelError::Poisoned)?
            .resolve(&track.path, &global)
            .clone();
        let mut engine = self.engine.lock().map_err(|_| KernelError::Poisoned)?;
        engine.stop();
        let rg = track
            .replaygain_track_db
            .map(|t| (t, track.replaygain_album_db));
        engine.play_with_replaygain(PathBuf::from(&track.path), &device, dsp, rg)?;
        Ok(self.status_of(&engine))
    }

    pub fn play_queue_index(&self, index: usize) -> Result<PlaybackStatus, KernelError> {
        let track = {
            let mut queue = self.queue.lock().map_err(|_| KernelError::Poisoned)?;
            let track =
                queue.tracks.get(index).cloned().ok_or_else(|| {
                    KernelError::Device(format!("queue index {index} out of range"))
                })?;
            queue.current = Some(index);
            track
        };
        self.start_track_at(&track)
    }

    pub fn next_track(&self) -> Result<Option<QueueDto>, KernelError> {
        let track = {
            let mut queue = self.queue.lock().map_err(|_| KernelError::Poisoned)?;
            queue.advance()
        };
        match track {
            Some(t) => {
                self.start_track_at(&t)?;
                let queue = self.queue.lock().map_err(|_| KernelError::Poisoned)?;
                Ok(Some(self.queue_dto(&queue)))
            }
            None => {
                let mut engine = self.engine.lock().map_err(|_| KernelError::Poisoned)?;
                engine.stop();
                Ok(None)
            }
        }
    }

    pub fn prev_track(&self) -> Result<QueueDto, KernelError> {
        let track = {
            let mut queue = self.queue.lock().map_err(|_| KernelError::Poisoned)?;
            queue.rewind()
        };
        let track = track.ok_or_else(|| KernelError::Device("empty queue".into()))?;
        self.start_track_at(&track)?;
        let queue = self.queue.lock().map_err(|_| KernelError::Poisoned)?;
        Ok(self.queue_dto(&queue))
    }

    // -----------------------------------------------------------------
    // DSP
    // -----------------------------------------------------------------

    pub fn get_dsp(&self) -> Result<aurora_dsp::DspConfig, KernelError> {
        let dsp = self.dsp.lock().map_err(|_| KernelError::Poisoned)?;
        Ok(dsp.clone())
    }

    pub fn set_track_dsp(
        &self,
        track_path: String,
        config: aurora_dsp::DspConfig,
    ) -> Result<(), KernelError> {
        self.track_dsp
            .lock()
            .map_err(|_| KernelError::Poisoned)?
            .set(track_path, config);
        Ok(())
    }

    pub fn clear_track_dsp(&self, track_path: &str) -> Result<bool, KernelError> {
        Ok(self
            .track_dsp
            .lock()
            .map_err(|_| KernelError::Poisoned)?
            .remove(track_path)
            .is_some())
    }

    #[cfg(not(target_family = "wasm"))]
    pub async fn load_dsp_settings(&self) -> Result<(), KernelError> {
        let library = self.library.lock().await;
        let service = library.as_ref().ok_or(KernelError::LibraryNotOpen)?;
        let mut loaded_any = false;
        if let Some(raw) = aurora_storage::get_setting(service.pool(), "dsp.global.v1")
            .await
            .map_err(KernelError::from)?
        {
            if let Ok(config) = serde_json::from_str(&raw) {
                *self.dsp.lock().map_err(|_| KernelError::Poisoned)? = config;
                loaded_any = true;
            }
        }
        if let Some(raw) = aurora_storage::get_setting(service.pool(), "dsp.track.v1")
            .await
            .map_err(KernelError::from)?
        {
            if let Ok(profiles) = serde_json::from_str(&raw) {
                self.track_dsp
                    .lock()
                    .map_err(|_| KernelError::Poisoned)?
                    .replace(profiles);
                loaded_any = true;
            }
        }
        drop(library);
        if !loaded_any {
            self.persist_dsp_settings().await?;
        }
        Ok(())
    }

    #[cfg(not(target_family = "wasm"))]
    pub async fn persist_dsp_settings(&self) -> Result<(), KernelError> {
        let library = self.library.lock().await;
        let Some(service) = library.as_ref() else {
            // Playback remains usable while the asynchronous desktop library
            // bootstrap is still opening SQLite.
            return Ok(());
        };
        let global = self.dsp.lock().map_err(|_| KernelError::Poisoned)?.clone();
        let tracks = self
            .track_dsp
            .lock()
            .map_err(|_| KernelError::Poisoned)?
            .snapshot()
            .clone();
        aurora_storage::set_setting(
            service.pool(),
            "dsp.global.v1",
            &serde_json::to_string(&global).map_err(|e| KernelError::Library {
                message: e.to_string(),
            })?,
        )
        .await
        .map_err(KernelError::from)?;
        aurora_storage::set_setting(
            service.pool(),
            "dsp.track.v1",
            &serde_json::to_string(&tracks).map_err(|e| KernelError::Library {
                message: e.to_string(),
            })?,
        )
        .await
        .map_err(KernelError::from)?;
        Ok(())
    }

    pub fn set_dsp(
        &self,
        config: aurora_dsp::DspConfig,
    ) -> Result<aurora_dsp::DspConfig, KernelError> {
        {
            let mut dsp = self.dsp.lock().map_err(|_| KernelError::Poisoned)?;
            *dsp = config.clone();
        }
        let mut engine = self.engine.lock().map_err(|_| KernelError::Poisoned)?;
        engine.set_dsp_config(config.clone());
        if engine.take_dsp_transition() {
            tracing::info!("restarting stream through the DSP path");
            let _ = engine.restart_for_dsp();
        }
        Ok(config)
    }

    // -----------------------------------------------------------------
    // Output devices
    // -----------------------------------------------------------------

    /// Device resolution with honest fallback (invariant 8): a selected
    /// device that vanished falls back to the default, logged.
    pub fn resolve_device(
        &self,
        key: Option<&str>,
    ) -> Result<aurora_audio::OutputDevice, KernelError> {
        match key {
            None | Some("") => {
                aurora_audio::default_output_device().map_err(|e| KernelError::Engine {
                    message: e.to_string(),
                })
            }
            Some(k) => aurora_audio::output_devices()
                .map_err(|e| KernelError::Engine {
                    message: e.to_string(),
                })?
                .into_iter()
                .find(|d| d.key() == k)
                .ok_or_else(|| {
                    tracing::warn!(key = k, "requested device missing, using default");
                    KernelError::Device(k.to_string())
                })
                .or_else(|e| aurora_audio::default_output_device().map_err(|_| e)),
        }
    }

    // -----------------------------------------------------------------
    // Recovery / exclusive / verification (ADR 0002)
    // -----------------------------------------------------------------

    /// Stop a faulted stream and optionally replay the current queue
    /// track on the (new) default device — the recovery path for device
    /// loss. The engine itself never restarts on its own: recovery is an
    /// explicit, honest action.
    pub fn recover_playback(&self, replay: bool) -> Result<PlaybackStatus, KernelError> {
        {
            let mut engine = self.engine.lock().map_err(|_| KernelError::Poisoned)?;
            engine.stop();
        }
        if replay {
            let track = {
                let queue = self.queue.lock().map_err(|_| KernelError::Poisoned)?;
                queue.current.and_then(|i| queue.tracks.get(i).cloned())
            };
            if let Some(track) = track {
                return self.start_track_at(&track);
            }
        }
        let engine = self.engine.lock().map_err(|_| KernelError::Poisoned)?;
        Ok(self.status_of(&engine))
    }

    /// Probe WASAPI exclusive-mode support for the playing format.
    /// The honest ADR 0002 classification.
    #[cfg(feature = "win-wasapi")]
    pub fn probe_exclusive(&self) -> Result<ExclusiveProbe, KernelError> {
        let engine = self.engine.lock().map_err(|_| KernelError::Poisoned)?;
        let (Some(source), Some(device_key)) =
            (engine.source_format_clone(), engine.device_key_clone())
        else {
            return Ok(ExclusiveProbe {
                playing: false,
                exclusive_capable: None,
                integrity: "unverified".into(),
                note: "nothing playing".into(),
            });
        };
        let transformations = engine.transformations().to_vec();
        let integrity = aurora_audio::exclusive_integrity(&device_key, source, &transformations)?;
        Ok(ExclusiveProbe {
            playing: true,
            exclusive_capable: Some(matches!(
                integrity,
                aurora_core::OutputIntegrity::Unverified
            )),
            integrity: match integrity {
                aurora_core::OutputIntegrity::Verified => "verified",
                aurora_core::OutputIntegrity::Transformed => "transformed",
                aurora_core::OutputIntegrity::Unverified => "unverified",
            }
            .into(),
            note: if transformations.is_empty() {
                "no transformations; software chain is bit-exact".into()
            } else {
                "active transformations make the path non-identical".into()
            },
        })
    }

    /// Exclusive path probe without the WASAPI build: honest
    /// "unsupported", never a claim (ADR 0002).
    #[cfg(not(feature = "win-wasapi"))]
    pub fn probe_exclusive(&self) -> Result<ExclusiveProbe, KernelError> {
        Ok(ExclusiveProbe {
            playing: false,
            exclusive_capable: None,
            integrity: "unverified".into(),
            note: "exclusive output requires the win-wasapi build (Windows)".into(),
        })
    }

    /// Play a track through WASAPI exclusive mode: no mixer, no DSP, the
    /// source's exact bytes to the endpoint. Honest fallback on refusal.
    #[cfg(feature = "win-wasapi")]
    pub fn play_exclusive_track(
        &self,
        request: PlayRequest,
    ) -> Result<ExclusivePlayResult, KernelError> {
        let device = match request.device.as_deref() {
            None | Some("") => aurora_audio::default_output_device()?,
            Some(key) => aurora_audio::output_devices()?
                .into_iter()
                .find(|d| d.key() == key)
                .ok_or_else(|| KernelError::Device(key.to_string()))?,
        };

        let mut engine = self.engine.lock().map_err(|_| KernelError::Poisoned)?;
        let path = PathBuf::from(&request.path);
        match engine.play_exclusive(path, &device) {
            Ok(()) => Ok(ExclusivePlayResult {
                exclusive: true,
                fallback_reason: None,
                status: self.status_of(&engine),
            }),
            Err(aurora_audio::EngineError::FormatUnsupported(reason)) => {
                // Honest fallback: the device cannot take this format
                // exclusively; play the normal way and report it.
                tracing::warn!(%reason, "exclusive refused, falling back to shared");
                engine.play_with_replaygain(
                    PathBuf::from(&request.path),
                    &device,
                    aurora_dsp::DspConfig::default(),
                    request
                        .replaygain_track_db
                        .map(|t| (t, request.replaygain_album_db)),
                )?;
                Ok(ExclusivePlayResult {
                    exclusive: false,
                    fallback_reason: Some(reason),
                    status: self.status_of(&engine),
                })
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Exclusive play without the WASAPI build: honest "unsupported"
    /// (the shared path remains the caller's option).
    #[cfg(not(feature = "win-wasapi"))]
    pub fn play_exclusive_track(
        &self,
        request: PlayRequest,
    ) -> Result<ExclusivePlayResult, KernelError> {
        // Honest fallback: this build has no exclusive path at all; play
        // the shared way and say so.
        let status = self.play_track(request)?;
        Ok(ExclusivePlayResult {
            exclusive: false,
            fallback_reason: Some(
                "exclusive output requires the win-wasapi build (Windows)".into(),
            ),
            status,
        })
    }

    /// Run the physical loopback verification on the playing exclusive
    /// stream: capture what the endpoint rendered, compare byte-for-byte
    /// with the reference the engine pushed.
    #[cfg(feature = "win-wasapi")]
    pub fn verify_bit_perfect(
        &self,
        seconds: Option<f64>,
    ) -> Result<BitPerfectVerdict, KernelError> {
        let engine = self.engine.lock().map_err(|_| KernelError::Poisoned)?;
        let duration = std::time::Duration::from_secs_f64(seconds.unwrap_or(2.0).clamp(0.5, 5.0));
        match engine.verify_loopback(duration) {
            Ok(v) => Ok(BitPerfectVerdict {
                exclusive_active: true,
                identity: Some(v.identity),
                frames_compared: Some(v.frames_compared),
                integrity: match v.integrity {
                    aurora_core::OutputIntegrity::Verified => "verified".into(),
                    aurora_core::OutputIntegrity::Transformed => "transformed".into(),
                    aurora_core::OutputIntegrity::Unverified => "unverified".into(),
                },
                note: format!(
                    "{:.2}% byte-identical frames over {} frames measured",
                    v.identity * 100.0,
                    v.frames_compared
                ),
            }),
            Err(e) => Ok(BitPerfectVerdict {
                exclusive_active: false,
                identity: None,
                frames_compared: None,
                integrity: "unverified".into(),
                note: e.to_string(),
            }),
        }
    }

    /// Bit-perfect verdict without the WASAPI build: honestly
    /// "unverified" — the measurement cannot run, so it is never claimed.
    #[cfg(not(feature = "win-wasapi"))]
    pub fn verify_bit_perfect(
        &self,
        _seconds: Option<f64>,
    ) -> Result<BitPerfectVerdict, KernelError> {
        Ok(BitPerfectVerdict {
            exclusive_active: false,
            identity: None,
            frames_compared: None,
            integrity: "unverified".into(),
            note: "loopback verification requires the win-wasapi build (Windows)".into(),
        })
    }

    // -----------------------------------------------------------------
    // Spectrum / waveform / loop / VST on the main engine
    // -----------------------------------------------------------------

    /// Live spectrum: 16 log-spaced band energies (0..1), post-DSP/post-VST.
    pub fn playback_spectrum(&self) -> Result<Vec<f32>, KernelError> {
        let engine = self.engine.lock().map_err(|_| KernelError::Poisoned)?;
        Ok(engine.metrics().spectrum())
    }

    /// Set / clear the A/B loop region (seconds). None clears it.
    pub fn set_loop(
        &self,
        loop_start: Option<f64>,
        loop_end: Option<f64>,
    ) -> Result<Option<(f64, f64)>, KernelError> {
        let engine = self.engine.lock().map_err(|_| KernelError::Poisoned)?;
        let region = match (loop_start, loop_end) {
            (Some(s), Some(e)) if e > s => Some((s, e)),
            _ => None,
        };
        engine.set_loop(region);
        Ok(engine.loop_region())
    }

    pub fn set_vst_chain(&self, slots: Vec<VstSlotDto>) -> Result<PlaybackStatus, KernelError> {
        // The engine's VST types come from the real host natively and from
        // the no-op stub on wasm; the wire shape (slots) is identical.
        let list = aurora_audio::VstPluginList {
            slots: slots.into_iter().map(|s| (s.path, s.class_id)).collect(),
        };
        let engine = self.engine.lock().map_err(|_| KernelError::Poisoned)?;
        engine.set_vst(list);
        Ok(self.status_of(&engine))
    }

    pub fn get_vst_chain(&self) -> Result<Vec<VstSlotDto>, KernelError> {
        let engine = self.engine.lock().map_err(|_| KernelError::Poisoned)?;
        let list = engine.vst_chain();
        Ok(list
            .slots
            .into_iter()
            .map(|(path, class_id)| VstSlotDto { path, class_id })
            .collect())
    }

    pub fn set_vst_slot_bypass(&self, slot: usize, bypassed: bool) -> Result<(), KernelError> {
        let engine = self.engine.lock().map_err(|_| KernelError::Poisoned)?;
        engine.set_vst_slot_bypass(slot, bypassed);
        Ok(())
    }

    pub fn vst_bypassed_slots(&self) -> Result<Vec<usize>, KernelError> {
        let engine = self.engine.lock().map_err(|_| KernelError::Poisoned)?;
        Ok(engine.vst_bypassed_slots())
    }

    /// Set one parameter on a chain slot from a generic panel (normalized 0..1).
    pub fn set_vst_param(
        &self,
        path: &str,
        class_id: &str,
        id: u32,
        value: f64,
    ) -> Result<(), KernelError> {
        let engine = self.engine.lock().map_err(|_| KernelError::Poisoned)?;
        let slot = engine
            .vst_chain()
            .slots
            .iter()
            .position(|(p, c)| p == path && c == class_id)
            .unwrap_or(0);
        engine.set_vst_param_value(slot, id, value);
        Ok(())
    }

    /// Apply a preset file to a chain slot (live, no restart).
    #[cfg(not(target_family = "wasm"))]
    pub fn load_vst_preset(
        &self,
        path: &str,
        class_id: &str,
        preset_path: &str,
    ) -> Result<(), KernelError> {
        let params = aurora_vst::load_preset_and_params(path, class_id, preset_path)
            .map_err(|reason| KernelError::Engine { message: reason })?;
        let engine = self.engine.lock().map_err(|_| KernelError::Poisoned)?;
        let slot = engine
            .vst_chain()
            .slots
            .iter()
            .position(|(p, c)| p == path && c == class_id)
            .unwrap_or(0);
        let mut map = engine.vst_param_map(slot);
        for p in params {
            map.insert(p.id, p.value);
        }
        engine.set_vst_params(slot, map);
        Ok(())
    }

    /// Open the plugin's editor window (one at a time).
    ///
    /// SAFETY note (carried over from the adapter): the engine pointer
    /// is captured for the GUI callback thread. The engine lives in the
    /// kernel for the whole app lifetime; the callback only calls
    /// `set_vst_params`, which locks the shared mutex.
    #[cfg(not(target_family = "wasm"))]
    pub fn open_vst_editor(&self, path: &str, class_id: &str) -> Result<(), KernelError> {
        let mut gui_slot = self.vst_gui.lock().map_err(|_| KernelError::Poisoned)?;
        if let Some(mut old) = gui_slot.take() {
            old.close();
        }
        let engine: &PlaybackEngine = {
            let guard = self.engine.lock().map_err(|_| KernelError::Poisoned)?;
            let ptr: *const PlaybackEngine = &*guard;
            unsafe { &*ptr }
        };
        let slot = self
            .engine
            .lock()
            .map_err(|_| KernelError::Poisoned)?
            .vst_chain()
            .slots
            .iter()
            .position(|(p, c)| p == path && c == class_id)
            .unwrap_or(0);
        let gui = aurora_vst::open_gui(
            path,
            class_id,
            Box::new(move |params| {
                engine.set_vst_params(slot, params.into_iter().collect());
            }),
        )
        .map_err(|reason| KernelError::Engine { message: reason })?;
        *gui_slot = Some(gui);
        tracing::info!(path, slot, "vst editor window opened");
        Ok(())
    }

    #[cfg(not(target_family = "wasm"))]
    pub fn close_vst_editor(&self) -> Result<(), KernelError> {
        let mut gui_slot = self.vst_gui.lock().map_err(|_| KernelError::Poisoned)?;
        if let Some(mut gui) = gui_slot.take() {
            gui.close();
        }
        Ok(())
    }

    // -----------------------------------------------------------------
    // DJ decks
    // -----------------------------------------------------------------

    /// Deck status snapshot (both decks).
    pub fn deck_status(&self) -> Result<[DeckDto; 2], KernelError> {
        let crossfader = *self.crossfader.lock().map_err(|_| KernelError::Poisoned)?;
        let decks = self.decks.lock().map_err(|_| KernelError::Poisoned)?;
        Ok([
            self.deck_dto(0, &decks[0], crossfader),
            self.deck_dto(1, &decks[1], crossfader),
        ])
    }

    pub fn deck_dto(&self, index: usize, deck: &Deck, crossfader: f64) -> DeckDto {
        let state = match deck.engine.state() {
            EngineState::Idle => "idle",
            EngineState::Playing => "playing",
            EngineState::Paused => "paused",
            EngineState::Draining => "draining",
        };
        DeckDto {
            index,
            name: if index == 0 { "A".into() } else { "B".into() },
            loaded: deck.loaded.clone(),
            state: state.into(),
            position_s: deck.engine.position_s(),
            duration_s: deck.engine.duration_s(),
            channel_fader: deck.channel_fader,
            gain_db: Deck::gain_db(deck.channel_fader, crossfader, index == 1),
            cues: deck.cues.to_vec(),
            speed: f64::from(deck.dsp.speed),
            bpm: deck.loaded.as_ref().and_then(|t| t.bpm),
        }
    }

    /// Load a track onto a deck (does not start it — like pressing LOAD
    /// on a CDJ). The deck's DSP (EQ/tempo/fader gain) applies from the
    /// start. BPM is measured lazily here (first measurement persisted).
    pub async fn deck_load(
        &self,
        deck: usize,
        path: String,
        title: Option<String>,
        artist: Option<String>,
    ) -> Result<DeckDto, KernelError> {
        let idx = deck.min(1);
        let crossfader = *self.crossfader.lock().map_err(|_| KernelError::Poisoned)?;
        let device_key = self
            .device_key
            .lock()
            .map_err(|_| KernelError::Poisoned)?
            .clone();
        let device = self.resolve_device(device_key.as_deref())?;
        let bpm = self.measure_track_bpm(&path).await;
        let track = QueueTrack {
            path,
            title,
            artist,
            replaygain_track_db: None,
            replaygain_album_db: None,
            bpm,
        };
        let mut decks = self.decks.lock().map_err(|_| KernelError::Poisoned)?;
        let d = &mut decks[idx];
        d.engine.stop();
        d.cues = std::array::from_fn(|_| None);
        let rg = track
            .replaygain_track_db
            .map(|t| (t, track.replaygain_album_db));
        d.engine
            .play_with_replaygain(PathBuf::from(&track.path), &device, d.dsp.clone(), rg)?;
        // Loading pauses at position 0 (CUE standby): pause immediately
        // so the DJ starts it on purpose.
        let _ = d.engine.pause();
        d.loaded = Some(track);
        Ok(self.deck_dto(idx, d, crossfader))
    }

    /// Press PLAY/PAUSE on a deck.
    pub fn deck_play(&self, deck: usize) -> Result<DeckDto, KernelError> {
        let idx = deck.min(1);
        let crossfader = *self.crossfader.lock().map_err(|_| KernelError::Poisoned)?;
        let mut decks = self.decks.lock().map_err(|_| KernelError::Poisoned)?;
        let d = &mut decks[idx];
        match d.engine.state() {
            EngineState::Playing => {
                d.engine.pause()?;
            }
            EngineState::Paused => {
                d.engine.resume()?;
            }
            _ => {
                // Idle: re-play the loaded track.
                if let Some(t) = d.loaded.clone() {
                    let device = self.resolve_device(
                        self.device_key
                            .lock()
                            .map_err(|_| KernelError::Poisoned)?
                            .as_deref(),
                    )?;
                    let rg = t.replaygain_track_db.map(|v| (v, t.replaygain_album_db));
                    d.engine.play_with_replaygain(
                        PathBuf::from(&t.path),
                        &device,
                        d.dsp.clone(),
                        rg,
                    )?;
                }
            }
        }
        Ok(self.deck_dto(idx, d, crossfader))
    }

    pub fn deck_stop(&self, deck: usize) -> Result<DeckDto, KernelError> {
        let idx = deck.min(1);
        let crossfader = *self.crossfader.lock().map_err(|_| KernelError::Poisoned)?;
        let mut decks = self.decks.lock().map_err(|_| KernelError::Poisoned)?;
        decks[idx].engine.stop();
        Ok(self.deck_dto(idx, &decks[idx], crossfader))
    }

    pub fn deck_seek(&self, deck: usize, position_s: f64) -> Result<(), KernelError> {
        let idx = deck.min(1);
        let mut decks = self.decks.lock().map_err(|_| KernelError::Poisoned)?;
        decks[idx].engine.seek(position_s.max(0.0));
        Ok(())
    }

    pub fn deck_fader(&self, deck: usize, value: f64) -> Result<DeckDto, KernelError> {
        let idx = deck.min(1);
        let crossfader = *self.crossfader.lock().map_err(|_| KernelError::Poisoned)?;
        let mut decks = self.decks.lock().map_err(|_| KernelError::Poisoned)?;
        let d = &mut decks[idx];
        d.channel_fader = value.clamp(0.0, 1.0);
        let gain = Deck::gain_db(d.channel_fader, crossfader, idx == 1);
        d.apply_volume(gain);
        Ok(self.deck_dto(idx, d, crossfader))
    }

    /// Move the crossfader (0 = deck A, 1 = deck B). Both decks' gains
    /// update live (equal-power curve).
    pub fn set_crossfader(&self, value: f64) -> Result<[DeckDto; 2], KernelError> {
        let x = value.clamp(0.0, 1.0);
        let mut decks = self.decks.lock().map_err(|_| KernelError::Poisoned)?;
        let (fa, fb) = (decks[0].channel_fader, decks[1].channel_fader);
        let ga = Deck::gain_db(fa, x, false);
        let gb = Deck::gain_db(fb, x, true);
        decks[0].apply_volume(ga);
        decks[1].apply_volume(gb);
        *self.crossfader.lock().map_err(|_| KernelError::Poisoned)? = x;
        Ok([
            self.deck_dto(0, &decks[0], x),
            self.deck_dto(1, &decks[1], x),
        ])
    }

    /// Set a hot cue (slot 0..7) at a position; `None` clears it.
    pub fn deck_set_cue(
        &self,
        deck: usize,
        slot: usize,
        position: Option<f64>,
    ) -> Result<Vec<Option<f64>>, KernelError> {
        let idx = deck.min(1);
        let mut decks = self.decks.lock().map_err(|_| KernelError::Poisoned)?;
        let d = &mut decks[idx];
        if slot < 8 {
            d.cues[slot] = position;
        }
        Ok(d.cues.to_vec())
    }

    /// Jump to a hot cue: seek there and play (the CDJ behavior).
    pub fn deck_jump_cue(&self, deck: usize, slot: usize) -> Result<(), KernelError> {
        let idx = deck.min(1);
        let device_key = self
            .device_key
            .lock()
            .map_err(|_| KernelError::Poisoned)?
            .clone();
        let mut decks = self.decks.lock().map_err(|_| KernelError::Poisoned)?;
        let d = &mut decks[idx];
        let Some(&Some(pos)) = d.cues.get(slot) else {
            return Ok(());
        };
        // Idle deck: start the track first, then seek.
        if d.engine.state() == EngineState::Idle
            && let Some(t) = d.loaded.clone()
        {
            let device = self.resolve_device(device_key.as_deref())?;
            let rg = t.replaygain_track_db.map(|v| (v, t.replaygain_album_db));
            d.engine
                .play_with_replaygain(PathBuf::from(&t.path), &device, d.dsp.clone(), rg)?;
            let _ = d.engine.pause();
        }
        d.engine.seek(pos);
        let _ = d.engine.resume();
        Ok(())
    }

    /// Set the deck's tempo (pitch) multiplier. Live DSP change.
    pub fn deck_tempo(&self, deck: usize, speed: f64) -> Result<DeckDto, KernelError> {
        let idx = deck.min(1);
        let crossfader = *self.crossfader.lock().map_err(|_| KernelError::Poisoned)?;
        let mut decks = self.decks.lock().map_err(|_| KernelError::Poisoned)?;
        let d = &mut decks[idx];
        d.dsp.speed = speed.clamp(0.5, 2.0) as f32;
        d.engine.set_dsp_config(d.dsp.clone());
        Ok(self.deck_dto(idx, d, crossfader))
    }

    // -----------------------------------------------------------------
    // BPM (measured, never fabricated)
    // -----------------------------------------------------------------

    /// Look up the measured BPM for `path`; if not yet measured, run the
    /// analyzer (spawn_blocking — it decodes the file) and persist the
    /// measurement. `None` both when the track has no confident beat and
    /// when the library is not open (decks work without a library too).
    #[cfg(not(target_family = "wasm"))]
    pub async fn measure_track_bpm(&self, path: &str) -> Option<f64> {
        let library = self.library.lock().await;
        let service = library.as_ref()?;
        let existing = aurora_storage::track_by_path(service.pool(), path)
            .await
            .ok()
            .flatten();
        if let Some(bpm) = existing.as_ref().and_then(|t| t.bpm) {
            return Some(bpm);
        }
        let path_owned = path.to_string();
        let measured = tokio::task::spawn_blocking(move || {
            aurora_decoder::detect_bpm(std::path::Path::new(&path_owned))
                .ok()
                .flatten()
        })
        .await
        .ok()
        .flatten();
        if let Some(bpm) = measured {
            let _ = aurora_storage::set_track_bpm(service.pool(), path, Some(bpm)).await;
            Some(bpm)
        } else {
            // Known track, no confident beat: persist nothing — a failed
            // measurement is re-tried on the next load (cheap, honest).
            None
        }
    }

    /// Wasm: measure without the SQLite cache — the analyzer runs (the
    /// decoder is pure Rust, works in the browser); persistence is the
    /// JS shell's business (IndexedDB).
    #[cfg(target_family = "wasm")]
    pub async fn measure_track_bpm(&self, path: &str) -> Option<f64> {
        let path_owned = path.to_string();
        // spawn_blocking is unavailable on single-threaded wasm; the
        // analyzer runs inline (it is CPU-bound but bounded by duration).
        aurora_decoder::detect_bpm(std::path::Path::new(&path_owned))
            .ok()
            .flatten()
    }

    /// Measured tempo of a track by library path (cache-first; runs the
    /// analyzer when not yet measured). `None` = no confident periodic
    /// beat, which is a measurement result, not an error.
    pub async fn track_bpm(&self, path: &str) -> Result<Option<f64>, KernelError> {
        Ok(self.measure_track_bpm(path).await)
    }

    // -----------------------------------------------------------------
    // Library
    // -----------------------------------------------------------------

    #[cfg(not(target_family = "wasm"))]
    pub async fn library_status(&self) -> Result<LibraryStatus, KernelError> {
        let count = {
            let slot = self.library.lock().await;
            match slot.as_ref() {
                Some(lib) => aurora_storage::track_count(lib.pool())
                    .await
                    .map_err(KernelError::from)?,
                None => 0,
            }
        };
        let path = self
            .library_path_slot
            .lock()
            .map_err(|_| KernelError::Poisoned)?
            .as_ref()
            .map(|p| p.display().to_string());
        Ok(LibraryStatus {
            open: path.is_some(),
            path,
            track_count: count,
        })
    }

    /// Open (or create) the library database at `path`.
    #[cfg(not(target_family = "wasm"))]
    pub async fn open_library(&self, path: &str) -> Result<LibraryStatus, KernelError> {
        let service = aurora_library::LibraryService::open(std::path::Path::new(path))
            .await
            .map_err(|e| KernelError::Library {
                message: e.to_string(),
            })?;
        let count = aurora_storage::track_count(service.pool())
            .await
            .map_err(KernelError::from)?;
        let mut slot = self.library.lock().await;
        *slot = Some(service);
        *self
            .library_path_slot
            .lock()
            .map_err(|_| KernelError::Poisoned)? = Some(PathBuf::from(path));
        Ok(LibraryStatus {
            open: true,
            path: Some(path.to_string()),
            track_count: count,
        })
    }

    /// Scan a folder into the library; remembers the root.
    #[cfg(not(target_family = "wasm"))]
    pub async fn scan_folder(
        &self,
        root: &str,
    ) -> Result<aurora_library::ScanSummary, KernelError> {
        let library = self.library.lock().await;
        let service = library.as_ref().ok_or(KernelError::LibraryNotOpen)?;
        let summary = service
            .scan_folder(std::path::Path::new(root))
            .await
            .map_err(|e| KernelError::Library {
                message: e.to_string(),
            })?;
        aurora_storage::add_library_root(service.pool(), root)
            .await
            .map_err(KernelError::from)?;
        drop(library);
        Ok(summary)
    }

    /// Rescan every known library root.
    #[cfg(not(target_family = "wasm"))]
    pub async fn rescan_all(&self) -> Result<aurora_library::ScanSummary, KernelError> {
        let library = self.library.lock().await;
        let service = library.as_ref().ok_or(KernelError::LibraryNotOpen)?;
        let roots = aurora_storage::library_roots(service.pool())
            .await
            .map_err(KernelError::from)?;
        let mut total = aurora_library::ScanSummary::default();
        for root in roots {
            let summary = service
                .scan_folder(std::path::Path::new(&root))
                .await
                .map_err(|e| KernelError::Library {
                    message: e.to_string(),
                })?;
            total.discovered += summary.discovered;
            total.added += summary.added;
            total.updated += summary.updated;
            total.pruned += summary.pruned;
            total.failed += summary.failed;
        }
        drop(library);
        Ok(total)
    }

    #[cfg(not(target_family = "wasm"))]
    pub async fn list_tracks(&self) -> Result<Vec<TrackDto>, KernelError> {
        let library = self.library.lock().await;
        let tracks = library
            .as_ref()
            .ok_or(KernelError::LibraryNotOpen)?
            .tracks()
            .await
            .map_err(|e| KernelError::Library {
                message: e.to_string(),
            })?;
        drop(library);
        Ok(tracks.iter().map(TrackDto::from_track).collect())
    }

    /// Persist user-supplied lyrics, translation, artwork, source and video metadata.
    #[cfg(not(target_family = "wasm"))]
    pub async fn set_track_enrichment(
        &self,
        track_id: &str,
        enrichment: &aurora_storage::TrackEnrichment,
    ) -> Result<(), KernelError> {
        let library = self.library.lock().await;
        let service = library.as_ref().ok_or(KernelError::LibraryNotOpen)?;
        aurora_storage::set_track_enrichment(service.pool(), track_id, enrichment)
            .await
            .map_err(|e| KernelError::Library {
                message: e.to_string(),
            })
    }

    #[cfg(not(target_family = "wasm"))]
    pub async fn search_library(&self, query: &str) -> Result<Vec<TrackDto>, KernelError> {
        let library = self.library.lock().await;
        let tracks = library
            .as_ref()
            .ok_or(KernelError::LibraryNotOpen)?
            .search(query)
            .await
            .map_err(|e| KernelError::Library {
                message: e.to_string(),
            })?;
        drop(library);
        Ok(tracks.iter().map(TrackDto::from_track).collect())
    }

    // -----------------------------------------------------------------
    // Settings
    // -----------------------------------------------------------------

    #[cfg(not(target_family = "wasm"))]
    pub async fn get_setting(&self, key: &str) -> Result<Option<String>, KernelError> {
        let library = self.library.lock().await;
        let service = library.as_ref().ok_or(KernelError::LibraryNotOpen)?;
        let value = aurora_storage::get_setting(service.pool(), key)
            .await
            .map_err(KernelError::from)?;
        drop(library);
        Ok(value)
    }

    #[cfg(not(target_family = "wasm"))]
    pub async fn set_setting(&self, key: &str, value: Option<&str>) -> Result<(), KernelError> {
        let library = self.library.lock().await;
        let service = library.as_ref().ok_or(KernelError::LibraryNotOpen)?;
        match value {
            Some(v) => {
                aurora_storage::set_setting(service.pool(), key, v)
                    .await
                    .map_err(KernelError::from)?;
            }
            None => {
                aurora_storage::delete_setting(service.pool(), key)
                    .await
                    .map_err(KernelError::from)?;
            }
        }
        drop(library);
        Ok(())
    }

    /// The library pool (for watcher startup; None when not open).
    #[cfg(not(target_family = "wasm"))]
    pub async fn library_pool(&self) -> Option<aurora_storage::SqlitePool> {
        self.library.lock().await.as_ref().map(|s| s.pool().clone())
    }

    /// Library roots as PathBufs (watcher startup).
    #[cfg(not(target_family = "wasm"))]
    pub async fn library_roots(&self) -> Result<Vec<PathBuf>, KernelError> {
        let library = self.library.lock().await;
        let service = library.as_ref().ok_or(KernelError::LibraryNotOpen)?;
        let roots = aurora_storage::library_roots(service.pool())
            .await
            .map_err(KernelError::from)?;
        drop(library);
        Ok(roots.into_iter().map(PathBuf::from).collect())
    }

    // -----------------------------------------------------------------
    // Playlists: CRUD + import / export
    // -----------------------------------------------------------------

    /// Create a playlist; returns its id.
    #[cfg(not(target_family = "wasm"))]
    pub async fn create_playlist(&self, name: &str) -> Result<String, KernelError> {
        let library = self.library.lock().await;
        let lib = library.as_ref().ok_or(KernelError::LibraryNotOpen)?;
        let id = aurora_storage::create_playlist(lib.pool(), name)
            .await
            .map_err(|e| KernelError::Library {
                message: e.to_string(),
            })?;
        drop(library);
        Ok(id)
    }

    #[cfg(not(target_family = "wasm"))]
    pub async fn rename_playlist(
        &self,
        playlist_id: &str,
        new_name: &str,
    ) -> Result<(), KernelError> {
        let library = self.library.lock().await;
        let lib = library.as_ref().ok_or(KernelError::LibraryNotOpen)?;
        let res = aurora_storage::rename_playlist(lib.pool(), playlist_id, new_name)
            .await
            .map_err(|e| KernelError::Library {
                message: e.to_string(),
            });
        drop(library);
        res
    }

    #[cfg(not(target_family = "wasm"))]
    pub async fn delete_playlist(&self, playlist_id: &str) -> Result<(), KernelError> {
        let library = self.library.lock().await;
        let lib = library.as_ref().ok_or(KernelError::LibraryNotOpen)?;
        let res = aurora_storage::delete_playlist(lib.pool(), playlist_id)
            .await
            .map_err(|e| KernelError::Library {
                message: e.to_string(),
            });
        drop(library);
        res
    }

    #[cfg(not(target_family = "wasm"))]
    pub async fn list_playlists(&self) -> Result<Vec<aurora_core::Playlist>, KernelError> {
        let library = self.library.lock().await;
        let playlists = {
            let lib = library.as_ref().ok_or(KernelError::LibraryNotOpen)?;
            aurora_storage::all_playlists(lib.pool())
                .await
                .map_err(|e| KernelError::Library {
                    message: e.to_string(),
                })?
        };
        drop(library);
        Ok(playlists)
    }

    /// Set playlist contents (track ids, order preserved).
    #[cfg(not(target_family = "wasm"))]
    pub async fn set_playlist_tracks(
        &self,
        playlist_id: &str,
        track_ids: &[String],
    ) -> Result<(), KernelError> {
        let ids: Vec<aurora_core::TrackId> = track_ids
            .iter()
            .filter_map(|s| uuid::Uuid::parse_str(s).ok())
            .map(aurora_core::TrackId)
            .collect();
        let library = self.library.lock().await;
        let lib = library.as_ref().ok_or(KernelError::LibraryNotOpen)?;
        let res = aurora_storage::set_playlist_tracks(lib.pool(), playlist_id, &ids)
            .await
            .map_err(|e| KernelError::Library {
                message: e.to_string(),
            });
        drop(library);
        res
    }

    /// Playlist tracks as full DTOs, in saved order.
    #[cfg(not(target_family = "wasm"))]
    pub async fn playlist_tracks(&self, playlist_id: &str) -> Result<Vec<TrackDto>, KernelError> {
        let library = self.library.lock().await;
        let tracks = {
            let lib = library.as_ref().ok_or(KernelError::LibraryNotOpen)?;
            aurora_storage::playlist_tracks(lib.pool(), playlist_id)
                .await
                .map_err(|e| KernelError::Library {
                    message: e.to_string(),
                })?
        };
        drop(library);
        Ok(tracks.iter().map(TrackDto::from_track).collect())
    }

    /// Import a playlist file into the library.
    ///
    /// Formats: M3U/M3U8, PLS (paths), Spotify JSON, CSV (title/artist,
    /// see `playlist_io`). Path entries resolve against the playlist's
    /// own folder; service entries match the local library by (title,
    /// artist), case-insensitive on both. Unmatched entries are
    /// reported, never fabricated as tracks.
    #[cfg(not(target_family = "wasm"))]
    pub async fn import_playlist(&self, path: &str) -> Result<ImportReport, KernelError> {
        let path_buf = PathBuf::from(path);
        let raw = tokio::fs::read_to_string(&path_buf)
            .await
            .map_err(|e| KernelError::Library {
                message: format!("cannot read {}: {e}", path_buf.display()),
            })?;
        let parsed = crate::playlist_io::parse_playlist(&path_buf, &raw)
            .map_err(|e| KernelError::Library { message: e })?;

        let library = self.library.lock().await;
        let lib = library.as_ref().ok_or(KernelError::LibraryNotOpen)?;
        let pool = lib.pool();

        // Pass 0: if any path entry points at a file that exists on disk
        // but is not in the library yet, scan its folder first
        // (idempotent) so the entry can resolve honestly.
        let mut resolved_by_scan = false;
        let needs_scan = parsed.entries.iter().find_map(|entry| match entry {
            crate::playlist_io::ImportedEntry::Path(p) => {
                std::path::Path::new(p).is_file().then(|| {
                    std::path::Path::new(p)
                        .parent()
                        .map(|d| d.display().to_string())
                        .unwrap_or_default()
                })
            }
            crate::playlist_io::ImportedEntry::Meta { .. } => None,
        });
        if let Some(folder) = needs_scan.filter(|f| !f.is_empty()) {
            let service = aurora_library::LibraryService::from_pool(pool.clone());
            if service
                .scan_folder(std::path::Path::new(&folder))
                .await
                .is_ok()
            {
                resolved_by_scan = true;
            }
        }

        // Local library snapshot for matching (lowercase keys).
        let all = aurora_storage::all_tracks(pool)
            .await
            .map_err(KernelError::from)?;
        let mut by_meta: std::collections::HashMap<(String, Option<String>), aurora_core::TrackId> =
            std::collections::HashMap::with_capacity(all.len());
        let mut by_title: std::collections::HashMap<String, aurora_core::TrackId> =
            std::collections::HashMap::with_capacity(all.len());
        let mut by_path: std::collections::HashMap<String, aurora_core::TrackId> =
            std::collections::HashMap::with_capacity(all.len());
        for t in &all {
            if let Some(title) = &t.title {
                let key = title.to_lowercase();
                if let Some(artist) = t.artist.as_ref().map(|a| a.name.to_lowercase()) {
                    by_meta.entry((key.clone(), Some(artist))).or_insert(t.id);
                }
                by_title.entry(key).or_insert(t.id);
            }
            by_path.entry(t.path.to_lowercase()).or_insert(t.id);
        }

        // Single pass, entry order preserved (order is the playlist).
        let mut matched_ids: Vec<aurora_core::TrackId> = Vec::new();
        let mut unmatched: u64 = 0;
        let mut unmatched_names: Vec<String> = Vec::new();
        for entry in &parsed.entries {
            match entry {
                crate::playlist_io::ImportedEntry::Path(p) => {
                    match by_path.get(&p.to_lowercase()) {
                        Some(id) => matched_ids.push(*id),
                        None => {
                            unmatched += 1;
                            unmatched_names.push(p.clone());
                        }
                    }
                }
                crate::playlist_io::ImportedEntry::Meta { title, artist } => {
                    let tkey = title.to_lowercase();
                    let matched = artist
                        .as_ref()
                        .and_then(|a| {
                            by_meta
                                .get(&(tkey.clone(), Some(a.to_lowercase())))
                                .copied()
                        })
                        .or_else(|| by_title.get(&tkey).copied());
                    match matched {
                        Some(id) => matched_ids.push(id),
                        None => {
                            unmatched += 1;
                            unmatched_names.push(match artist {
                                Some(a) => format!("{a} — {title}"),
                                None => title.clone(),
                            });
                        }
                    }
                }
            }
        }

        let playlist_name = if parsed.name.is_empty() {
            "Imported playlist".to_string()
        } else {
            parsed.name
        };
        let playlist_id = aurora_storage::create_playlist(pool, &playlist_name)
            .await
            .map_err(|e| KernelError::Library {
                message: e.to_string(),
            })?;
        aurora_storage::set_playlist_tracks(pool, &playlist_id, &matched_ids)
            .await
            .map_err(|e| KernelError::Library {
                message: e.to_string(),
            })?;
        drop(library);

        Ok(ImportReport {
            playlist_id,
            playlist_name,
            matched: matched_ids.len() as u64,
            unmatched,
            resolved_by_scan,
            unmatched_names: unmatched_names.into_iter().take(8).collect(),
        })
    }

    /// Export a playlist as M3U8 to `dest` (absolute paths + EXTINF).
    #[cfg(not(target_family = "wasm"))]
    pub async fn export_playlist(&self, playlist_id: &str, dest: &str) -> Result<(), KernelError> {
        let library = self.library.lock().await;
        let tracks = {
            let lib = library.as_ref().ok_or(KernelError::LibraryNotOpen)?;
            aurora_storage::playlist_tracks(lib.pool(), playlist_id)
                .await
                .map_err(|e| KernelError::Library {
                    message: e.to_string(),
                })?
        };
        drop(library);
        let export: Vec<crate::playlist_io::ExportTrack> = tracks
            .iter()
            .map(|t| crate::playlist_io::ExportTrack {
                path: t.path.clone(),
                title: t.title.clone(),
                artist: t.artist.as_ref().map(|a| a.name.clone()),
                duration_s: t.duration_ms.map(|ms| ms as f64 / 1000.0),
            })
            .collect();
        let content = crate::playlist_io::to_m3u8("", &export); // M3U8 carries no playlist name
        tokio::fs::write(PathBuf::from(dest), content)
            .await
            .map_err(|e| KernelError::Library {
                message: format!("cannot write {dest}: {e}"),
            })?;
        Ok(())
    }

    // -----------------------------------------------------------------
    // VST chain persistence + probe helpers
    // -----------------------------------------------------------------

    /// Persist the VST chain (slots) into the library settings so it
    /// survives restarts. `None` clears the stored chain.
    #[cfg(not(target_family = "wasm"))]
    pub async fn save_vst_chain_setting(
        &self,
        slots: Option<Vec<VstSlotDto>>,
    ) -> Result<(), KernelError> {
        let library = self.library.lock().await;
        let service = library.as_ref().ok_or(KernelError::LibraryNotOpen)?;
        let value = slots.map(|s| serde_json::to_string(&s).unwrap_or_else(|_| "[]".to_string()));
        let res = match value {
            Some(v) => aurora_storage::set_setting(service.pool(), "vst_chain", &v)
                .await
                .map_err(KernelError::from),
            None => aurora_storage::delete_setting(service.pool(), "vst_chain")
                .await
                .map_err(KernelError::from),
        };
        drop(library);
        res
    }

    /// The persisted VST chain, restored at app start.
    #[cfg(not(target_family = "wasm"))]
    pub async fn load_vst_chain_setting(&self) -> Result<Option<Vec<VstSlotDto>>, KernelError> {
        let library = self.library.lock().await;
        let service = library.as_ref().ok_or(KernelError::LibraryNotOpen)?;
        let value = aurora_storage::get_setting(service.pool(), "vst_chain")
            .await
            .map_err(KernelError::from)?;
        drop(library);
        Ok(value.and_then(|v| serde_json::from_str(&v).ok()))
    }
}
