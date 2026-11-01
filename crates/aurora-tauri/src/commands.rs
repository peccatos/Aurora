//! Tauri command surface: thin wrappers over the application services.
//!
//! The engine lives in managed state; commands only translate between the
//! frontend's JSON payloads and backend calls. No business logic here.

use std::path::PathBuf;
use std::sync::Mutex;

use aurora_audio::{EngineState, PlaybackEngine};
use aurora_core::PlaybackMode;
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::error::CommandError;

/// Managed application state.
pub struct AppState {
    pub engine: Mutex<PlaybackEngine>,
    /// Persisted DSP configuration: the UI edits this and every new
    /// playback picks it up.
    pub dsp: std::sync::Mutex<aurora_dsp::DspConfig>,
    /// Playback queue with the current index.
    pub queue: std::sync::Mutex<TrackQueue>,
}

/// The user's playback queue.
#[derive(Default)]
pub struct TrackQueue {
    pub tracks: Vec<String>,
    pub current: Option<usize>,
}

impl TrackQueue {
    fn next(&mut self) -> Option<String> {
        let idx = self.current? + 1;
        if idx >= self.tracks.len() {
            return None;
        }
        self.current = Some(idx);
        self.tracks.get(idx).cloned()
    }

    fn prev(&mut self) -> Option<String> {
        let idx = self.current?;
        if idx == 0 {
            return self.tracks.first().cloned();
        }
        self.current = Some(idx - 1);
        self.tracks.get(idx - 1).cloned()
    }
}

impl AppState {
    pub fn new() -> Self {
        Self {
            engine: Mutex::new(PlaybackEngine::new()),
            dsp: std::sync::Mutex::new(aurora_dsp::DspConfig::default()),
            queue: std::sync::Mutex::new(TrackQueue::default()),
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

#[tauri::command]
pub fn app_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[tauri::command]
pub fn app_name() -> &'static str {
    "Aurora"
}

#[tauri::command]
pub fn playback_modes() -> Vec<PlaybackMode> {
    vec![
        PlaybackMode::BitPerfect,
        PlaybackMode::Dsp,
        PlaybackMode::Auto,
    ]
}

// ---------------------------------------------------------------------------
// Device + playback commands
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct DeviceDto {
    pub key: String,
    pub name: String,
}

#[tauri::command]
pub fn list_output_devices(state: State<AppState>) -> Result<Vec<DeviceDto>, CommandError> {
    let engine = state.engine.lock().map_err(|_| CommandError::Poisoned)?;
    let devices = engine.devices()?;
    Ok(devices
        .into_iter()
        .map(|d| DeviceDto {
            key: d.key().to_string(),
            name: d.name().to_string(),
        })
        .collect())
}

#[derive(Debug, Serialize)]
pub struct PlaybackStatus {
    pub state: EngineState,
    /// Negotiated output sample rate, when playing.
    pub sample_rate: Option<u32>,
    pub channels: Option<u16>,
    pub bits_per_sample: Option<u16>,
    /// Active transformations, reported per ADR 0002.
    pub transformations: Vec<aurora_core::AudioTransformation>,
    /// ADR 0002 output path integrity classification.
    pub integrity: aurora_core::OutputIntegrity,
    pub frames_played: u64,
    pub underruns: u64,
}

fn status_of(engine: &PlaybackEngine) -> PlaybackStatus {
    let format = engine.output_format();
    PlaybackStatus {
        state: engine.state(),
        sample_rate: format.map(|f| f.sample_rate),
        channels: format.map(|f| f.channels),
        bits_per_sample: format.map(|f| f.bits_per_sample),
        transformations: engine.transformations().to_vec(),
        integrity: engine.integrity(),
        frames_played: engine.metrics().frames_played(),
        underruns: engine.metrics().underruns(),
    }
}

#[tauri::command]
pub fn playback_status(state: State<AppState>) -> Result<PlaybackStatus, CommandError> {
    let engine = state.engine.lock().map_err(|_| CommandError::Poisoned)?;
    Ok(status_of(&engine))
}

#[derive(Debug, Deserialize)]
pub struct PlayRequest {
    pub path: String,
    /// Device key from `list_output_devices`; empty = default device.
    pub device: Option<String>,
    /// DSP configuration for this stream; defaults to bypassed.
    #[serde(default)]
    pub dsp: aurora_dsp::DspConfig,
}

#[tauri::command]
pub fn play_track(
    request: PlayRequest,
    state: State<AppState>,
) -> Result<PlaybackStatus, CommandError> {
    let mut engine = state.engine.lock().map_err(|_| CommandError::Poisoned)?;
    let device = match request.device.as_deref() {
        None | Some("") => aurora_audio::default_output_device()?,
        Some(key) => aurora_audio::output_devices()?
            .into_iter()
            .find(|d| d.key() == key)
            .ok_or_else(|| CommandError::Device(request.device.clone().unwrap_or_default()))?,
    };
    // The request's DSP overrides the persisted config when present;
    // otherwise the UI's last saved settings apply.
    let dsp = if request.dsp == aurora_dsp::DspConfig::default() {
        state
            .dsp
            .lock()
            .map_err(|_| CommandError::Poisoned)?
            .clone()
    } else {
        request.dsp
    };
    engine.play(PathBuf::from(&request.path), &device, dsp)?;
    Ok(status_of(&engine))
}

#[tauri::command]
pub fn get_dsp(state: State<AppState>) -> Result<aurora_dsp::DspConfig, CommandError> {
    let dsp = state.dsp.lock().map_err(|_| CommandError::Poisoned)?;
    Ok(dsp.clone())
}

// ---------------------------------------------------------------------------
// Queue commands
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct QueueDto {
    tracks: Vec<String>,
    current: Option<usize>,
}

#[tauri::command]
pub fn set_queue(tracks: Vec<String>, state: State<AppState>) -> Result<QueueDto, CommandError> {
    let mut queue = state.queue.lock().map_err(|_| CommandError::Poisoned)?;
    queue.tracks = tracks;
    queue.current = None;
    Ok(QueueDto {
        tracks: queue.tracks.clone(),
        current: queue.current,
    })
}

#[tauri::command]
pub fn get_queue(state: State<AppState>) -> Result<QueueDto, CommandError> {
    let queue = state.queue.lock().map_err(|_| CommandError::Poisoned)?;
    Ok(QueueDto {
        tracks: queue.tracks.clone(),
        current: queue.current,
    })
}

fn start_track_at(path: &str, state: &State<AppState>) -> Result<PlaybackStatus, CommandError> {
    let device = aurora_audio::default_output_device()?;
    let dsp = state
        .dsp
        .lock()
        .map_err(|_| CommandError::Poisoned)?
        .clone();
    let mut engine = state.engine.lock().map_err(|_| CommandError::Poisoned)?;
    engine.stop();
    engine.play(std::path::PathBuf::from(path), &device, dsp)?;
    Ok(status_of(&engine))
}

#[tauri::command]
pub fn play_queue_index(
    index: usize,
    state: State<AppState>,
) -> Result<PlaybackStatus, CommandError> {
    let path = {
        let mut queue = state.queue.lock().map_err(|_| CommandError::Poisoned)?;
        let path = queue
            .tracks
            .get(index)
            .cloned()
            .ok_or_else(|| CommandError::Device(format!("queue index {index} out of range")))?;
        queue.current = Some(index);
        path
    };
    start_track_at(&path, &state)
}

#[tauri::command]
pub fn next_track(state: State<AppState>) -> Result<Option<QueueDto>, CommandError> {
    let path = {
        let mut queue = state.queue.lock().map_err(|_| CommandError::Poisoned)?;
        queue.next()
    };
    match path {
        Some(p) => {
            start_track_at(&p, &state)?;
            let queue = state.queue.lock().map_err(|_| CommandError::Poisoned)?;
            Ok(Some(QueueDto {
                tracks: queue.tracks.clone(),
                current: queue.current,
            }))
        }
        None => {
            let mut engine = state.engine.lock().map_err(|_| CommandError::Poisoned)?;
            engine.stop();
            Ok(None)
        }
    }
}

#[tauri::command]
pub fn prev_track(state: State<AppState>) -> Result<QueueDto, CommandError> {
    let path = {
        let mut queue = state.queue.lock().map_err(|_| CommandError::Poisoned)?;
        queue.prev()
    };
    let path = path.ok_or_else(|| CommandError::Device("empty queue".into()))?;
    start_track_at(&path, &state)?;
    let queue = state.queue.lock().map_err(|_| CommandError::Poisoned)?;
    Ok(QueueDto {
        tracks: queue.tracks.clone(),
        current: queue.current,
    })
}

// ---------------------------------------------------------------------------
// Seek + live DSP
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn seek(position_s: f64, state: State<AppState>) -> Result<PlaybackStatus, CommandError> {
    let engine = state.engine.lock().map_err(|_| CommandError::Poisoned)?;
    engine.seek(position_s);
    Ok(status_of(&engine))
}

#[tauri::command]
pub fn playback_position(state: State<AppState>) -> Result<f64, CommandError> {
    let engine = state.engine.lock().map_err(|_| CommandError::Poisoned)?;
    Ok(engine.position_s())
}

#[tauri::command]
pub fn set_dsp(
    config: aurora_dsp::DspConfig,
    state: State<AppState>,
) -> Result<aurora_dsp::DspConfig, CommandError> {
    // Persisted for future streams...
    {
        let mut dsp = state.dsp.lock().map_err(|_| CommandError::Poisoned)?;
        *dsp = config.clone();
    }
    // ...and applied live to the playing stream.
    let engine = state.engine.lock().map_err(|_| CommandError::Poisoned)?;
    engine.set_dsp_config(config.clone());
    Ok(config)
}

/// Phase 5: probe WASAPI exclusive-mode support for the playing format.
///
/// Returns the honest ADR 0002 classification: what the device's
/// exclusive path can do for this exact stream.
#[tauri::command]
pub fn probe_exclusive(state: State<AppState>) -> Result<ExclusiveProbe, CommandError> {
    let engine = state.engine.lock().map_err(|_| CommandError::Poisoned)?;
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

#[derive(Debug, Serialize)]
pub struct ExclusiveProbe {
    pub playing: bool,
    /// Some(true): the device accepts the format in exclusive mode.
    pub exclusive_capable: Option<bool>,
    pub integrity: String,
    pub note: String,
}

#[tauri::command]
pub fn pause_playback(state: State<AppState>) -> Result<PlaybackStatus, CommandError> {
    let mut engine = state.engine.lock().map_err(|_| CommandError::Poisoned)?;
    engine.pause()?;
    Ok(status_of(&engine))
}

#[tauri::command]
pub fn resume_playback(state: State<AppState>) -> Result<PlaybackStatus, CommandError> {
    let mut engine = state.engine.lock().map_err(|_| CommandError::Poisoned)?;
    engine.resume()?;
    Ok(status_of(&engine))
}

#[tauri::command]
pub fn stop_playback(state: State<AppState>) -> Result<PlaybackStatus, CommandError> {
    let mut engine = state.engine.lock().map_err(|_| CommandError::Poisoned)?;
    engine.stop();
    Ok(status_of(&engine))
}

pub fn builder() -> tauri::Builder<tauri::Wry> {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            app_version,
            app_name,
            playback_modes,
            list_output_devices,
            playback_status,
            playback_position,
            play_track,
            get_dsp,
            set_dsp,
            seek,
            probe_exclusive,
            set_queue,
            get_queue,
            play_queue_index,
            next_track,
            prev_track,
            pause_playback,
            resume_playback,
            stop_playback
        ])
        .setup(|app| {
            tracing::info!(
                name = %app.package_info().name,
                version = %app.package_info().version,
                "Aurora desktop initialized"
            );
            Ok(())
        })
}
