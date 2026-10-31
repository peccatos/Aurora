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
}

impl AppState {
    pub fn new() -> Self {
        Self {
            engine: Mutex::new(PlaybackEngine::new()),
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
    engine.play(PathBuf::from(&request.path), &device)?;
    Ok(status_of(&engine))
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
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            app_version,
            app_name,
            playback_modes,
            list_output_devices,
            playback_status,
            play_track,
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
