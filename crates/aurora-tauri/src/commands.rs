//! Tauri command surface: thin wrappers over the application services.
//!
//! The engine lives in managed state; commands only translate between the
//! frontend's JSON payloads and backend calls. No business logic here.

use std::path::PathBuf;
use std::sync::Mutex;

use aurora_audio::{EngineState, PlaybackEngine};
use aurora_core::PlaybackMode;
use serde::{Deserialize, Serialize};
use tauri::{Manager, State};

use crate::error::CommandError;

/// Managed application state.
pub struct AppState {
    pub engine: Mutex<PlaybackEngine>,
    /// Persisted DSP configuration: the UI edits this and every new
    /// playback picks it up.
    pub dsp: std::sync::Mutex<aurora_dsp::DspConfig>,
    /// Playback queue with the current index.
    pub queue: std::sync::Mutex<TrackQueue>,
    /// Track library (SQLite + scanner). Async by nature: scanning and
    /// queries run on the tokio runtime, never blocking the UI thread.
    /// None until `open_library` runs.
    pub library: tokio::sync::Mutex<Option<aurora_library::LibraryService>>,
    /// Library database path (None = library not opened yet).
    pub library_path_slot: std::sync::Mutex<Option<PathBuf>>,
    /// Selected output device key ("" = default). Persisted to settings.
    pub device_key: std::sync::Mutex<Option<String>>,
    /// Open plugin editor GUI session (one at a time; the window owns
    /// its own GUI thread with a Win32 message pump).
    pub vst_gui: std::sync::Mutex<Option<aurora_vst::PluginGui>>,
}

/// One queue entry: the path plus what the engine needs from the library
/// (ReplayGain tags) and what the UI shows (display name).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QueueTrack {
    pub path: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub artist: Option<String>,
    #[serde(default)]
    pub replaygain_track_db: Option<f64>,
    #[serde(default)]
    pub replaygain_album_db: Option<f64>,
}

impl QueueTrack {
    /// Display label: "Artist — Title" with file-stem fallback.
    pub fn display_line(&self) -> String {
        let title = self
            .title
            .as_deref()
            .unwrap_or_else(|| file_stem(&self.path));
        match &self.artist {
            Some(a) if !a.is_empty() => format!("{a} — {title}"),
            _ => title.to_string(),
        }
    }
}

/// File stem of a path (both separators, no extension).
fn file_stem(path: &str) -> &str {
    let start = path.rfind(['/', '\\']).map_or(0, |i| i + 1);
    let end = path.rfind('.').filter(|&i| i > start).unwrap_or(path.len());
    &path[start..end]
}

/// The user's playback queue.
#[derive(Default)]
pub struct TrackQueue {
    pub tracks: Vec<QueueTrack>,
    pub current: Option<usize>,
}

impl TrackQueue {
    fn next(&mut self) -> Option<QueueTrack> {
        let idx = self.current? + 1;
        if idx >= self.tracks.len() {
            return None;
        }
        self.current = Some(idx);
        self.tracks.get(idx).cloned()
    }

    fn prev(&mut self) -> Option<QueueTrack> {
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
            library: tokio::sync::Mutex::new(None),
            library_path_slot: std::sync::Mutex::new(None),
            device_key: std::sync::Mutex::new(None),
            vst_gui: std::sync::Mutex::new(None),
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Library commands (Phase 6)
// ---------------------------------------------------------------------------

/// Track as the frontend sees it: display fields + what playback needs.
#[derive(Debug, Clone, Serialize)]
pub struct TrackDto {
    pub id: String,
    pub path: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub genre: Option<String>,
    pub year: Option<i32>,
    pub track_number: Option<u32>,
    pub format: String,
    pub duration_ms: Option<u64>,
    pub replaygain_track_db: Option<f64>,
    pub replaygain_album_db: Option<f64>,
    /// "Artist - Title" (file stem when untagged): the correct name.
    pub display: String,
    /// Duration formatted "m:ss" for list rows.
    pub duration_display: String,
}

impl TrackDto {
    fn from_track(t: &aurora_core::Track) -> Self {
        let duration_display = t
            .duration_ms
            .map(|ms| {
                let sec = ms / 1000;
                format!("{}:{:02}", sec / 60, sec % 60)
            })
            .unwrap_or_default();
        Self {
            id: t.id.0.to_string(),
            path: t.path.clone(),
            title: t.title.clone(),
            artist: t.artist.as_ref().map(|a| a.name.clone()),
            album: t.album.as_ref().map(|a| a.title.clone()),
            genre: t.genre.clone(),
            year: t.year,
            track_number: t.track_number,
            format: format!("{:?}", t.format).to_lowercase(),
            duration_ms: t.duration_ms,
            replaygain_track_db: t.replaygain_track_db,
            replaygain_album_db: t.replaygain_album_db,
            display: t.display_line(),
            duration_display,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct LibraryStatus {
    pub open: bool,
    pub path: Option<String>,
    pub track_count: u64,
}

#[tauri::command]
pub async fn library_status(state: State<'_, AppState>) -> Result<LibraryStatus, CommandError> {
    let count = {
        let slot = state.library.lock().await;
        match slot.as_ref() {
            Some(lib) => aurora_storage::track_count(lib.pool())
                .await
                .map_err(CommandError::from)?,
            None => 0,
        }
    };
    let path = state
        .library_path_slot
        .lock()
        .map_err(|_| CommandError::Poisoned)?
        .as_ref()
        .map(|p| p.display().to_string());
    Ok(LibraryStatus {
        open: path.is_some(),
        path,
        track_count: count,
    })
}

/// Open (or create) the library database at `path` (a .db file).
#[tauri::command]
pub async fn open_library(
    path: String,
    state: State<'_, AppState>,
) -> Result<LibraryStatus, CommandError> {
    let service = aurora_library::LibraryService::open(std::path::Path::new(&path))
        .await
        .map_err(|e| CommandError::Library {
            message: e.to_string(),
        })?;
    let count = aurora_storage::track_count(service.pool())
        .await
        .map_err(CommandError::from)?;
    let mut slot = state.library.lock().await;
    *slot = Some(service);
    *state
        .library_path_slot
        .lock()
        .map_err(|_| CommandError::Poisoned)? = Some(PathBuf::from(&path));
    Ok(LibraryStatus {
        open: true,
        path: Some(path),
        track_count: count,
    })
}

/// Scan a folder into the library. Returns the honest scan summary.
#[tauri::command]
pub async fn scan_folder(
    root: String,
    state: State<'_, AppState>,
) -> Result<aurora_library::ScanSummary, CommandError> {
    let library = state.library.lock().await;
    let service = library.as_ref().ok_or(CommandError::LibraryNotOpen)?;
    let summary = service
        .scan_folder(std::path::Path::new(&root))
        .await
        .map_err(|e| CommandError::Library {
            message: e.to_string(),
        })?;
    // Remember the root so future "refresh" rescans can replay it
    // (Phase 7: device-independent library roots).
    aurora_storage::add_library_root(service.pool(), &root)
        .await
        .map_err(CommandError::from)?;
    drop(library);
    Ok(summary)
}

/// Rescan every known library root (the "refresh library" button).
#[tauri::command]
pub async fn rescan_all(
    state: State<'_, AppState>,
) -> Result<aurora_library::ScanSummary, CommandError> {
    let library = state.library.lock().await;
    let service = library.as_ref().ok_or(CommandError::LibraryNotOpen)?;
    let roots = aurora_storage::library_roots(service.pool())
        .await
        .map_err(CommandError::from)?;
    let mut total = aurora_library::ScanSummary::default();
    for root in roots {
        let summary = service
            .scan_folder(std::path::Path::new(&root))
            .await
            .map_err(|e| CommandError::Library {
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

/// Get a persisted setting (e.g. "output_device").
#[tauri::command]
pub async fn get_setting_cmd(
    key: String,
    state: State<'_, AppState>,
) -> Result<Option<String>, CommandError> {
    let library = state.library.lock().await;
    let service = library.as_ref().ok_or(CommandError::LibraryNotOpen)?;
    let value = aurora_storage::get_setting(service.pool(), &key)
        .await
        .map_err(CommandError::from)?;
    drop(library);
    Ok(value)
}

/// Persist a setting (device selection, UI prefs).
#[tauri::command]
pub async fn set_setting_cmd(
    key: String,
    value: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), CommandError> {
    let library = state.library.lock().await;
    let service = library.as_ref().ok_or(CommandError::LibraryNotOpen)?;
    match value {
        Some(v) => {
            aurora_storage::set_setting(service.pool(), &key, &v)
                .await
                .map_err(CommandError::from)?;
        }
        None => {
            // Erase by writing an empty sentinel-free delete.
            aurora_storage::delete_setting(service.pool(), &key)
                .await
                .map_err(CommandError::from)?;
        }
    }
    drop(library);
    Ok(())
}

/// All library tracks (ordered artist/album/track).
#[tauri::command]
pub async fn list_tracks(state: State<'_, AppState>) -> Result<Vec<TrackDto>, CommandError> {
    let library = state.library.lock().await;
    let tracks = library
        .as_ref()
        .ok_or(CommandError::LibraryNotOpen)?
        .tracks()
        .await
        .map_err(|e| CommandError::Library {
            message: e.to_string(),
        })?;
    drop(library);
    Ok(tracks.iter().map(TrackDto::from_track).collect())
}

/// Case-insensitive search over title/artist/album/file name.
#[tauri::command]
pub async fn search_library(
    query: String,
    state: State<'_, AppState>,
) -> Result<Vec<TrackDto>, CommandError> {
    let library = state.library.lock().await;
    let tracks = library
        .as_ref()
        .ok_or(CommandError::LibraryNotOpen)?
        .search(&query)
        .await
        .map_err(|e| CommandError::Library {
            message: e.to_string(),
        })?;
    drop(library);
    Ok(tracks.iter().map(TrackDto::from_track).collect())
}

#[derive(Debug, Serialize)]
pub struct PlaylistDto {
    pub id: String,
    pub name: String,
    pub track_count: u64,
}

#[tauri::command]
pub async fn list_playlists(state: State<'_, AppState>) -> Result<Vec<PlaylistDto>, CommandError> {
    let library = state.library.lock().await;
    let playlists = {
        let lib = library.as_ref().ok_or(CommandError::LibraryNotOpen)?;
        aurora_storage::all_playlists(lib.pool())
            .await
            .map_err(|e| CommandError::Library {
                message: e.to_string(),
            })?
    };
    Ok(playlists
        .into_iter()
        .map(|p| PlaylistDto {
            id: p.id,
            name: p.name,
            track_count: p.track_count,
        })
        .collect())
}

#[tauri::command]
pub async fn create_playlist(
    name: String,
    state: State<'_, AppState>,
) -> Result<PlaylistDto, CommandError> {
    let library = state.library.lock().await;
    let id = {
        let lib = library.as_ref().ok_or(CommandError::LibraryNotOpen)?;
        aurora_storage::create_playlist(lib.pool(), &name)
            .await
            .map_err(|e| CommandError::Library {
                message: e.to_string(),
            })?
    };
    Ok(PlaylistDto {
        id,
        name,
        track_count: 0,
    })
}

#[tauri::command]
pub async fn rename_playlist(
    playlist_id: String,
    new_name: String,
    state: State<'_, AppState>,
) -> Result<(), CommandError> {
    let library = state.library.lock().await;
    let lib = library.as_ref().ok_or(CommandError::LibraryNotOpen)?;
    aurora_storage::rename_playlist(lib.pool(), &playlist_id, &new_name)
        .await
        .map_err(|e| CommandError::Library {
            message: e.to_string(),
        })
}

#[tauri::command]
pub async fn delete_playlist(
    playlist_id: String,
    state: State<'_, AppState>,
) -> Result<(), CommandError> {
    let library = state.library.lock().await;
    let lib = library.as_ref().ok_or(CommandError::LibraryNotOpen)?;
    aurora_storage::delete_playlist(lib.pool(), &playlist_id)
        .await
        .map_err(|e| CommandError::Library {
            message: e.to_string(),
        })
}

/// Set playlist contents (track ids, order preserved).
#[tauri::command]
pub async fn set_playlist_tracks(
    playlist_id: String,
    track_ids: Vec<String>,
    state: State<'_, AppState>,
) -> Result<(), CommandError> {
    let library = state.library.lock().await;
    let ids: Vec<aurora_core::TrackId> = track_ids
        .iter()
        .filter_map(|s| uuid::Uuid::parse_str(s).ok())
        .map(aurora_core::TrackId)
        .collect();
    let lib = library.as_ref().ok_or(CommandError::LibraryNotOpen)?;
    aurora_storage::set_playlist_tracks(lib.pool(), &playlist_id, &ids)
        .await
        .map_err(|e| CommandError::Library {
            message: e.to_string(),
        })
}

/// Playlist tracks as full DTOs, in saved order.
#[tauri::command]
pub async fn playlist_tracks(
    playlist_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<TrackDto>, CommandError> {
    let library = state.library.lock().await;
    let tracks = {
        let lib = library.as_ref().ok_or(CommandError::LibraryNotOpen)?;
        aurora_storage::playlist_tracks(lib.pool(), &playlist_id)
            .await
            .map_err(|e| CommandError::Library {
                message: e.to_string(),
            })?
    };
    Ok(tracks.iter().map(TrackDto::from_track).collect())
}

/// Select the output device for all playback (persisted across runs).
///
/// Empty key = system default. Applied to the next track; the current
/// stream keeps its device until it ends (switching a live CPAL stream
/// mid-flight is a device-loss recovery path, not a hot-swap).
#[tauri::command]
pub async fn select_output_device(
    device_key: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), CommandError> {
    {
        let mut slot = state
            .device_key
            .lock()
            .map_err(|_| CommandError::Poisoned)?;
        *slot = device_key.clone().filter(|k| !k.is_empty());
    }
    // Persist into settings when the library is open (best effort).
    let library = state.library.lock().await;
    if let Some(service) = library.as_ref() {
        let value = device_key.clone().filter(|k| !k.is_empty());
        let result = match value {
            Some(v) => aurora_storage::set_setting(service.pool(), "output_device", &v).await,
            None => aurora_storage::delete_setting(service.pool(), "output_device").await,
        };
        if let Err(e) = result {
            tracing::warn!(%e, "could not persist output device setting");
        }
    }
    drop(library);
    Ok(())
}

/// The persisted output device key, for restoring the selector on start.
#[tauri::command]
pub async fn get_output_device_setting(
    state: State<'_, AppState>,
) -> Result<Option<String>, CommandError> {
    let library = state.library.lock().await;
    let service = library.as_ref().ok_or(CommandError::LibraryNotOpen)?;
    let value = aurora_storage::get_setting(service.pool(), "output_device")
        .await
        .map_err(CommandError::from)?;
    drop(library);
    Ok(value.filter(|k| !k.is_empty()))
}

/// Append tracks to the end of the queue (library "add to queue").
#[tauri::command]
pub fn queue_add_tracks(
    tracks: Vec<QueueTrack>,
    state: State<AppState>,
) -> Result<QueueDto, CommandError> {
    let mut queue = state.queue.lock().map_err(|_| CommandError::Poisoned)?;
    queue.tracks.extend(tracks);
    if queue.current.is_none() && !queue.tracks.is_empty() {
        queue.current = Some(0);
    }
    Ok(queue_dto(&queue))
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
    /// Phase 8: set when the stream errored (device loss) or the decode
    /// worker panicked since the last status poll. The UI shows a
    /// recovery note; faults are one-shot (cleared on read).
    pub fault: Option<String>,
    /// Negotiated output sample rate, when playing.
    pub sample_rate: Option<u32>,
    pub channels: Option<u16>,
    pub bits_per_sample: Option<u16>,
    /// Active transformations, reported per ADR 0002.
    pub transformations: Vec<aurora_core::AudioTransformation>,
    /// ADR 0002 output path integrity classification.
    pub integrity: aurora_core::OutputIntegrity,
    /// Total track duration in seconds, when the container declares it.
    pub duration_s: Option<f64>,
    pub frames_played: u64,
    pub underruns: u64,
}

fn status_of(engine: &PlaybackEngine) -> PlaybackStatus {
    let format = engine.output_format();
    // One-shot fault flags: reading consumes them, so each fault is
    // reported to the UI exactly once (not on every 500ms poll).
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
    /// ReplayGain tags for this track (from the library), when known.
    #[serde(default)]
    pub replaygain_track_db: Option<f64>,
    #[serde(default)]
    pub replaygain_album_db: Option<f64>,
}

#[tauri::command]
pub fn play_track(
    request: PlayRequest,
    state: State<AppState>,
) -> Result<PlaybackStatus, CommandError> {
    let mut engine = state.engine.lock().map_err(|_| CommandError::Poisoned)?;
    let selected = state
        .device_key
        .lock()
        .map_err(|_| CommandError::Poisoned)?
        .clone();
    let requested = request
        .device
        .clone()
        .filter(|d| !d.is_empty())
        .or(selected);
    let device = match requested.as_deref() {
        None => aurora_audio::default_output_device()?,
        Some(key) => aurora_audio::output_devices()?
            .into_iter()
            .find(|d| d.key() == key)
            .ok_or_else(|| {
                tracing::warn!(key, "requested device missing, using default");
                CommandError::Device(key.to_string())
            })
            .or_else(|e| aurora_audio::default_output_device().map_err(|_| e))?,
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
    let rg = request
        .replaygain_track_db
        .map(|t| (t, request.replaygain_album_db));
    engine.play_with_replaygain(PathBuf::from(&request.path), &device, dsp, rg)?;
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
    tracks: Vec<QueueTrack>,
    current: Option<usize>,
    /// "Artist — Title" per entry, in order (the UI's correct names).
    display: Vec<String>,
}

fn queue_dto(queue: &TrackQueue) -> QueueDto {
    QueueDto {
        display: queue.tracks.iter().map(|t| t.display_line()).collect(),
        tracks: queue.tracks.clone(),
        current: queue.current,
    }
}

#[tauri::command]
pub fn set_queue(
    tracks: Vec<QueueTrack>,
    state: State<AppState>,
) -> Result<QueueDto, CommandError> {
    let mut queue = state.queue.lock().map_err(|_| CommandError::Poisoned)?;
    queue.tracks = tracks;
    queue.current = None;
    Ok(queue_dto(&queue))
}

#[tauri::command]
pub fn get_queue(state: State<AppState>) -> Result<QueueDto, CommandError> {
    let queue = state.queue.lock().map_err(|_| CommandError::Poisoned)?;
    Ok(queue_dto(&queue))
}

fn start_track_at(
    track: &QueueTrack,
    state: &State<AppState>,
) -> Result<PlaybackStatus, CommandError> {
    let device = match state
        .device_key
        .lock()
        .map_err(|_| CommandError::Poisoned)?
        .as_deref()
    {
        None | Some("") => aurora_audio::default_output_device()?,
        Some(key) => aurora_audio::output_devices()?
            .into_iter()
            .find(|d| d.key() == key)
            .ok_or_else(|| {
                // Saved/selected device vanished (unplugged): fall back
                // to the default instead of failing playback (invariant 8).
                tracing::warn!(key, "selected output device missing, using default");
                CommandError::Device(key.to_string())
            })
            .or_else(|e| aurora_audio::default_output_device().map_err(|_| e))?,
    };
    let dsp = state
        .dsp
        .lock()
        .map_err(|_| CommandError::Poisoned)?
        .clone();
    let mut engine = state.engine.lock().map_err(|_| CommandError::Poisoned)?;
    engine.stop();
    let rg = track
        .replaygain_track_db
        .map(|t| (t, track.replaygain_album_db));
    engine.play_with_replaygain(std::path::PathBuf::from(&track.path), &device, dsp, rg)?;
    Ok(status_of(&engine))
}

#[tauri::command]
pub fn play_queue_index(
    index: usize,
    state: State<AppState>,
) -> Result<PlaybackStatus, CommandError> {
    let track = {
        let mut queue = state.queue.lock().map_err(|_| CommandError::Poisoned)?;
        let track = queue
            .tracks
            .get(index)
            .cloned()
            .ok_or_else(|| CommandError::Device(format!("queue index {index} out of range")))?;
        queue.current = Some(index);
        track
    };
    start_track_at(&track, &state)
}

#[tauri::command]
pub fn next_track(state: State<AppState>) -> Result<Option<QueueDto>, CommandError> {
    let track = {
        let mut queue = state.queue.lock().map_err(|_| CommandError::Poisoned)?;
        queue.next()
    };
    match track {
        Some(t) => {
            start_track_at(&t, &state)?;
            let queue = state.queue.lock().map_err(|_| CommandError::Poisoned)?;
            Ok(Some(queue_dto(&queue)))
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
    let track = {
        let mut queue = state.queue.lock().map_err(|_| CommandError::Poisoned)?;
        queue.prev()
    };
    let track = track.ok_or_else(|| CommandError::Device("empty queue".into()))?;
    start_track_at(&track, &state)?;
    let queue = state.queue.lock().map_err(|_| CommandError::Poisoned)?;
    Ok(queue_dto(&queue))
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
    // ...and applied live to the playing stream. When the stream runs on
    // the native (bit-exact) path and DSP just became active, the worker
    // flags a restart: the native path has no pipeline, the track re-plays
    // through the DSP path.
    let mut engine = state.engine.lock().map_err(|_| CommandError::Poisoned)?;
    engine.set_dsp_config(config.clone());
    if engine.take_dsp_transition() {
        tracing::info!("restarting stream through the DSP path");
        let _ = engine.restart_for_dsp();
    }
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

/// Phase 8: stop a faulted stream and optionally replay the current
/// queue track on the (new) default device — the recovery path for
/// device loss. The engine itself never restarts on its own: recovery is
/// an explicit, honest action.
#[tauri::command]
pub fn recover_playback(
    replay: bool,
    state: State<AppState>,
) -> Result<PlaybackStatus, CommandError> {
    {
        let mut engine = state.engine.lock().map_err(|_| CommandError::Poisoned)?;
        engine.stop();
    }
    if replay {
        let track = {
            let queue = state.queue.lock().map_err(|_| CommandError::Poisoned)?;
            queue.current.and_then(|i| queue.tracks.get(i).cloned())
        };
        if let Some(track) = track {
            return start_track_at(&track, &state);
        }
    }
    let engine = state.engine.lock().map_err(|_| CommandError::Poisoned)?;
    Ok(status_of(&engine))
}

// ---------------------------------------------------------------------------
// Exclusive bit-perfect path (Windows/WASAPI)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct ExclusivePlayResult {
    /// True when the exclusive path was taken; false = fell back to the
    /// shared path (device refused the format — reported honestly).
    pub exclusive: bool,
    /// Why the fallback happened, when it did.
    pub fallback_reason: Option<String>,
    pub status: PlaybackStatus,
}

/// Play a track through WASAPI exclusive mode: no mixer, no DSP, the
/// source's exact bytes to the endpoint.
///
/// If the device rejects the format, we fall back to the normal shared
/// path and say so — never silently claim exclusive.
#[tauri::command]
pub fn play_exclusive_track(
    request: PlayRequest,
    state: State<AppState>,
) -> Result<ExclusivePlayResult, CommandError> {
    let device = match request.device.as_deref() {
        None | Some("") => aurora_audio::default_output_device()?,
        Some(key) => aurora_audio::output_devices()?
            .into_iter()
            .find(|d| d.key() == key)
            .ok_or_else(|| CommandError::Device(key.to_string()))?,
    };

    let mut engine = state.engine.lock().map_err(|_| CommandError::Poisoned)?;
    let path = std::path::PathBuf::from(&request.path);
    match engine.play_exclusive(path, &device) {
        Ok(()) => Ok(ExclusivePlayResult {
            exclusive: true,
            fallback_reason: None,
            status: status_of(&engine),
        }),
        Err(aurora_audio::EngineError::FormatUnsupported(reason)) => {
            // Honest fallback: the device cannot take this format
            // exclusively; play the normal way and report it.
            tracing::warn!(%reason, "exclusive refused, falling back to shared");
            engine.play_with_replaygain(
                std::path::PathBuf::from(&request.path),
                &device,
                aurora_dsp::DspConfig::default(),
                request
                    .replaygain_track_db
                    .map(|t| (t, request.replaygain_album_db)),
            )?;
            Ok(ExclusivePlayResult {
                exclusive: false,
                fallback_reason: Some(reason),
                status: status_of(&engine),
            })
        }
        Err(e) => Err(e.into()),
    }
}

#[derive(Debug, Serialize)]
pub struct BitPerfectVerdict {
    /// True when an exclusive stream is playing.
    pub exclusive_active: bool,
    /// The physical measurement, when it ran.
    pub identity: Option<f64>,
    pub frames_compared: Option<u64>,
    /// Honest ADR 0002 verdict ("verified" / "transformed" /
    /// "unverified" — unverified = measurement failed, never assumed).
    pub integrity: String,
    pub note: String,
}

/// Run the physical loopback verification on the playing exclusive
/// stream: capture what the endpoint rendered, compare byte-for-byte
/// with the reference the engine pushed.
#[tauri::command]
pub fn verify_bit_perfect(
    seconds: Option<f64>,
    state: State<AppState>,
) -> Result<BitPerfectVerdict, CommandError> {
    let engine = state.engine.lock().map_err(|_| CommandError::Poisoned)?;
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

// ---------------------------------------------------------------------------
// VST3 plugin hosting (ADR 0004, Phase 9)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct VstPluginDto {
    pub path: String,
    pub name: String,
    pub vendor: String,
    pub version: String,
    pub category: String,
    pub class_id: String,
    pub input_channels: i32,
    pub output_channels: i32,
}

/// Scan standard VST3 directories (plus an optional extra folder) with the
/// crash-resistant out-of-process probe. Blocking — run from a thread.
#[tauri::command]
pub fn scan_vst_plugins(extra_folder: Option<String>) -> Result<Vec<VstPluginDto>, CommandError> {
    let extra = extra_folder
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(std::path::Path::new);
    let found = aurora_vst::scan_available_plugins(extra);
    Ok(found
        .into_iter()
        .map(|d| VstPluginDto {
            path: d.path,
            name: d.name,
            vendor: d.vendor,
            version: d.version,
            category: d.category,
            class_id: d.class_id,
            input_channels: d.input_channels,
            output_channels: d.output_channels,
        })
        .collect())
}

/// Set the active VST3 plugin chain (live: reloads without interrupting
/// playback). Empty slots = chain off. Plugins run out-of-process; the
/// transformation list reports them honestly (ADR 0002/0004).
#[tauri::command]
pub fn set_vst_chain(
    slots: Vec<VstSlotDto>,
    state: State<AppState>,
) -> Result<PlaybackStatus, CommandError> {
    let list = aurora_vst::VstPluginList {
        slots: slots.into_iter().map(|s| (s.path, s.class_id)).collect(),
    };
    let engine = state.engine.lock().map_err(|_| CommandError::Poisoned)?;
    engine.set_vst(list);
    Ok(status_of(&engine))
}

#[derive(Debug, Serialize, Deserialize)]
pub struct VstSlotDto {
    pub path: String,
    pub class_id: String,
}

/// The currently-configured VST3 chain (path + class id per slot).
#[tauri::command]
pub fn get_vst_chain(state: State<AppState>) -> Result<Vec<VstSlotDto>, CommandError> {
    let engine = state.engine.lock().map_err(|_| CommandError::Poisoned)?;
    let list = engine.vst_chain();
    Ok(list
        .slots
        .into_iter()
        .map(|(path, class_id)| VstSlotDto { path, class_id })
        .collect())
}

/// Load one plugin out-of-process and run a silent block through it:
/// the honest "does this plugin actually work here" verdict, surfaced to
/// the UI instead of a silent skip when playback starts.
#[tauri::command]
pub fn test_vst_plugin(path: String, class_id: String) -> Result<String, CommandError> {
    aurora_vst::test_plugin_load(&path, &class_id)
        .map_err(|reason| CommandError::Engine { message: reason })
}

/// Open the plugin's editor window (its own GUI thread + Win32 message
/// pump; the audio chain stays out-of-process). Parameter changes made
/// in the editor flow into the chain live. One editor at a time.
#[tauri::command]
pub fn open_vst_editor(
    path: String,
    class_id: String,
    state: State<AppState>,
) -> Result<(), CommandError> {
    let mut gui_slot = state.vst_gui.lock().map_err(|_| CommandError::Poisoned)?;
    // One editor at a time: close the previous session first.
    if let Some(mut old) = gui_slot.take() {
        old.close();
    }
    // The GUI thread forwards editor parameter snapshots into the
    // engine's live param slot; the worker applies them to the chain
    // between chunks.
    let engine = {
        let guard = state.engine.lock().map_err(|_| CommandError::Poisoned)?;
        let ptr: *const PlaybackEngine = &*guard;
        // SAFETY: the engine lives in AppState for the whole app
        // lifetime (managed state is never dropped before commands end);
        // the GUI thread only calls set_vst_params, which locks the
        // shared mutex, so the borrow is read-only and synchronized.
        unsafe { &*ptr }
    };
    let gui = aurora_vst::open_gui(
        &path,
        &class_id,
        Box::new(move |params| {
            engine.set_vst_params(params.into_iter().collect());
        }),
    )
    .map_err(|reason| CommandError::Engine { message: reason })?;
    *gui_slot = Some(gui);
    tracing::info!(%path, "vst editor window opened");
    Ok(())
}

/// Close the open plugin editor window, if any.
#[tauri::command]
pub fn close_vst_editor(state: State<AppState>) -> Result<(), CommandError> {
    let mut gui_slot = state.vst_gui.lock().map_err(|_| CommandError::Poisoned)?;
    if let Some(mut gui) = gui_slot.take() {
        gui.close();
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Artwork (embedded cover art from tags, cache in app temp)
// ---------------------------------------------------------------------------

/// In-memory artwork cache: track path -> data URL. Bounded by the
/// library size; one entry per track actually displayed.
fn artwork_cache() -> &'static std::sync::Mutex<std::collections::HashMap<String, String>> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, String>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Get the embedded cover art of a track as a data URL (for <img src>).
/// `Ok(None)` = no artwork in tags (the UI shows its placeholder).
#[tauri::command]
pub fn get_track_artwork(path: String) -> Result<Option<String>, CommandError> {
    // Cache hit (the library re-asks for the now-playing art on every
    // status poll's title update).
    if let Some(hit) = artwork_cache()
        .lock()
        .ok()
        .and_then(|c| c.get(&path).cloned())
    {
        return Ok(Some(hit));
    }
    let bytes = aurora_metadata::read_cover_art(std::path::Path::new(&path))?;
    let Some(bytes) = bytes else {
        return Ok(None);
    };
    let mime = sniff_image_mime(&bytes);
    let data = base64_encode(&bytes);
    let url = format!("data:{mime};base64,{data}");
    if let Ok(mut cache) = artwork_cache().lock() {
        // Bound the cache: drop the oldest entries past 64.
        if cache.len() >= 64 {
            cache.clear();
        }
        cache.insert(path, url.clone());
    }
    Ok(Some(url))
}

fn sniff_image_mime(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        "image/png"
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg"
    } else if bytes.starts_with(b"BM") {
        "image/bmp"
    } else if bytes.len() > 12 && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else {
        "application/octet-stream"
    }
}

/// Minimal base64 (std lib has none; keep it local and simple).
fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
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
            library_status,
            open_library,
            scan_folder,
            rescan_all,
            get_setting_cmd,
            set_setting_cmd,
            select_output_device,
            get_output_device_setting,
            queue_add_tracks,
            list_tracks,
            search_library,
            list_playlists,
            create_playlist,
            rename_playlist,
            delete_playlist,
            set_playlist_tracks,
            playlist_tracks,
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
            recover_playback,
            play_exclusive_track,
            verify_bit_perfect,
            scan_vst_plugins,
            set_vst_chain,
            get_vst_chain,
            test_vst_plugin,
            open_vst_editor,
            close_vst_editor,
            get_track_artwork,
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

            // Phase 7: auto-open the library at the app data dir. The
            // user never has to pick a .db by hand; `open_library`
            // remains available to point the app at a different one.
            if let Ok(data_dir) = app.path().app_data_dir() {
                if let Err(e) = std::fs::create_dir_all(&data_dir) {
                    tracing::warn!(%e, "cannot create app data dir");
                } else {
                    let db_path = data_dir.join("library.db");
                    let handle = app.handle().clone();
                    tauri::async_runtime::spawn(async move {
                        let state = handle.state::<AppState>();
                        match aurora_library::LibraryService::open(&db_path).await {
                            Ok(service) => {
                                let count = aurora_storage::track_count(service.pool())
                                    .await
                                    .unwrap_or(0);
                                *state.library.lock().await = Some(service);
                                *state.library_path_slot.lock().expect("path slot") =
                                    Some(db_path.clone());
                                tracing::info!(
                                    path = %db_path.display(),
                                    count,
                                    "library auto-opened"
                                );
                            }
                            Err(e) => {
                                tracing::warn!(
                                    path = %db_path.display(),
                                    %e,
                                    "auto-open library failed"
                                );
                            }
                        }
                    });
                }
            }
            Ok(())
        })
}
