//! Tauri command surface: thin wrappers over the kernel (ADR 0005).
//!
//! One command = one kernel call + DTO translation. No business logic
//! lives here: the music player IS `aurora-kernel`.

use std::sync::Arc;

use aurora_kernel::{
    BitPerfectVerdict, DeckDto, ExclusivePlayResult, ExclusiveProbe, ImportReport, KernelError,
    LibraryStatus, PlayRequest, PlaybackStatus, PlayerKernel, QueueDto, QueueTrack, TrackDto,
    VstParamDto, VstPluginDto, VstPresetDto, VstSlotDto,
};
use serde::Serialize;
use tauri::{Emitter, Manager, State};
use tauri_plugin_global_shortcut::ShortcutState;

use crate::error::CommandError;

/// Managed application state: the kernel + the shell-side pieces that
/// are Tauri-specific (nothing but the GUI session, which is native).
pub struct AppState {
    pub kernel: Arc<PlayerKernel>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            kernel: Arc::new(PlayerKernel::new()),
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

fn map_err(e: KernelError) -> CommandError {
    match e {
        KernelError::Poisoned => CommandError::Poisoned,
        KernelError::Engine { message } => CommandError::Engine { message },
        KernelError::Device(key) => CommandError::Device(key),
        KernelError::Library { message } => CommandError::Library { message },
        KernelError::LibraryNotOpen => CommandError::LibraryNotOpen,
        KernelError::Metadata { message } => CommandError::Metadata { message },
    }
}

type K<'a> = State<'a, AppState>;

// ---------------------------------------------------------------------------
// Version / playback basics
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn app_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[tauri::command]
pub fn app_name() -> &'static str {
    "Aurora"
}

#[tauri::command]
pub fn playback_modes() -> Vec<aurora_core::PlaybackMode> {
    vec![
        aurora_core::PlaybackMode::BitPerfect,
        aurora_core::PlaybackMode::Dsp,
        aurora_core::PlaybackMode::Auto,
    ]
}

#[tauri::command]
pub fn playback_status(state: K<'_>) -> Result<PlaybackStatus, CommandError> {
    state.kernel.playback_status().map_err(map_err)
}

#[tauri::command]
pub fn playback_position(state: K<'_>) -> Result<f64, CommandError> {
    state.kernel.playback_position().map_err(map_err)
}

#[tauri::command]
pub fn pause_playback(state: K<'_>) -> Result<PlaybackStatus, CommandError> {
    state.kernel.pause_playback().map_err(map_err)
}

#[tauri::command]
pub fn resume_playback(state: K<'_>) -> Result<PlaybackStatus, CommandError> {
    state.kernel.resume_playback().map_err(map_err)
}

#[tauri::command]
pub fn stop_playback(state: K<'_>) -> Result<PlaybackStatus, CommandError> {
    state.kernel.stop_playback().map_err(map_err)
}

#[tauri::command]
pub fn seek(position_s: f64, state: K<'_>) -> Result<PlaybackStatus, CommandError> {
    state.kernel.seek(position_s).map_err(map_err)
}

#[tauri::command]
pub fn play_track(request: PlayRequest, state: K<'_>) -> Result<PlaybackStatus, CommandError> {
    state.kernel.play_track(request).map_err(map_err)
}

// ---------------------------------------------------------------------------
// Output devices
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct DeviceDto {
    pub key: String,
    pub name: String,
}

#[tauri::command]
pub fn list_output_devices(state: K<'_>) -> Result<Vec<DeviceDto>, CommandError> {
    let engine = state
        .kernel
        .engine
        .lock()
        .map_err(|_| CommandError::Poisoned)?;
    let devices = engine.devices().map_err(|e| CommandError::Engine {
        message: e.to_string(),
    })?;
    Ok(devices
        .into_iter()
        .map(|d| DeviceDto {
            key: d.key().to_string(),
            name: d.name().to_string(),
        })
        .collect())
}

#[tauri::command]
pub async fn select_output_device(
    device_key: Option<String>,
    state: K<'_>,
) -> Result<(), CommandError> {
    {
        let mut slot = state
            .kernel
            .device_key
            .lock()
            .map_err(|_| CommandError::Poisoned)?;
        *slot = device_key.clone().filter(|k| !k.is_empty());
    }
    // Persist into settings when the library is open (best effort).
    let library = state.kernel.library.lock().await;
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

#[tauri::command]
pub async fn get_output_device_setting(state: K<'_>) -> Result<Option<String>, CommandError> {
    let value = state
        .kernel
        .get_setting("output_device")
        .await
        .map_err(map_err)?;
    Ok(value.filter(|k| !k.is_empty()))
}

// ---------------------------------------------------------------------------
// Library
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn library_status(state: K<'_>) -> Result<LibraryStatus, CommandError> {
    state.kernel.library_status().await.map_err(map_err)
}

#[tauri::command]
pub async fn open_library(path: String, state: K<'_>) -> Result<LibraryStatus, CommandError> {
    state.kernel.open_library(&path).await.map_err(map_err)
}

#[tauri::command]
pub async fn scan_folder(
    root: String,
    state: K<'_>,
) -> Result<aurora_library::ScanSummary, CommandError> {
    state.kernel.scan_folder(&root).await.map_err(map_err)
}

#[tauri::command]
pub async fn rescan_all(state: K<'_>) -> Result<aurora_library::ScanSummary, CommandError> {
    state.kernel.rescan_all().await.map_err(map_err)
}

#[tauri::command]
pub async fn get_setting_cmd(key: String, state: K<'_>) -> Result<Option<String>, CommandError> {
    state.kernel.get_setting(&key).await.map_err(map_err)
}

#[tauri::command]
pub async fn set_setting_cmd(
    key: String,
    value: Option<String>,
    state: K<'_>,
) -> Result<(), CommandError> {
    state
        .kernel
        .set_setting(&key, value.as_deref())
        .await
        .map_err(map_err)
}

#[tauri::command]
pub async fn list_tracks(state: K<'_>) -> Result<Vec<TrackDto>, CommandError> {
    state.kernel.list_tracks().await.map_err(map_err)
}

#[tauri::command]
pub async fn search_library(query: String, state: K<'_>) -> Result<Vec<TrackDto>, CommandError> {
    state.kernel.search_library(&query).await.map_err(map_err)
}

// ---------------------------------------------------------------------------
// Playlists
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct PlaylistDto {
    pub id: String,
    pub name: String,
    pub track_count: u64,
}

#[tauri::command]
pub async fn list_playlists(state: K<'_>) -> Result<Vec<PlaylistDto>, CommandError> {
    let playlists = state.kernel.list_playlists().await.map_err(map_err)?;
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
pub async fn create_playlist(name: String, state: K<'_>) -> Result<PlaylistDto, CommandError> {
    let id = state.kernel.create_playlist(&name).await.map_err(map_err)?;
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
    state: K<'_>,
) -> Result<(), CommandError> {
    state
        .kernel
        .rename_playlist(&playlist_id, &new_name)
        .await
        .map_err(map_err)
}

#[tauri::command]
pub async fn delete_playlist(playlist_id: String, state: K<'_>) -> Result<(), CommandError> {
    state
        .kernel
        .delete_playlist(&playlist_id)
        .await
        .map_err(map_err)
}

#[tauri::command]
pub async fn set_playlist_tracks(
    playlist_id: String,
    track_ids: Vec<String>,
    state: K<'_>,
) -> Result<(), CommandError> {
    state
        .kernel
        .set_playlist_tracks(&playlist_id, &track_ids)
        .await
        .map_err(map_err)
}

#[tauri::command]
pub async fn playlist_tracks(
    playlist_id: String,
    state: K<'_>,
) -> Result<Vec<TrackDto>, CommandError> {
    state
        .kernel
        .playlist_tracks(&playlist_id)
        .await
        .map_err(map_err)
}

#[tauri::command]
pub async fn import_playlist(path: String, state: K<'_>) -> Result<ImportReport, CommandError> {
    state.kernel.import_playlist(&path).await.map_err(map_err)
}

#[tauri::command]
pub async fn export_playlist(
    playlist_id: String,
    dest: String,
    state: K<'_>,
) -> Result<(), CommandError> {
    state
        .kernel
        .export_playlist(&playlist_id, &dest)
        .await
        .map_err(map_err)
}

// ---------------------------------------------------------------------------
// Queue / DSP / exclusive / recovery
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn set_queue(tracks: Vec<QueueTrack>, state: K<'_>) -> Result<QueueDto, CommandError> {
    state.kernel.set_queue(tracks).map_err(map_err)
}

/// Append tracks to the queue (keeps the current index; a fresh queue
/// starts at 0).
#[tauri::command]
pub fn queue_add_tracks(tracks: Vec<QueueTrack>, state: K<'_>) -> Result<QueueDto, CommandError> {
    let kernel = &state.kernel;
    let mut queue = kernel.queue.lock().map_err(|_| CommandError::Poisoned)?;
    queue.tracks.extend(tracks);
    if queue.current.is_none() && !queue.tracks.is_empty() {
        queue.current = Some(0);
    }
    let dto = kernel.queue_dto(&queue);
    Ok(dto)
}

#[tauri::command]
pub fn get_queue(state: K<'_>) -> Result<QueueDto, CommandError> {
    state.kernel.get_queue().map_err(map_err)
}

#[tauri::command]
pub fn play_queue_index(index: usize, state: K<'_>) -> Result<PlaybackStatus, CommandError> {
    state.kernel.play_queue_index(index).map_err(map_err)
}

#[tauri::command]
pub fn next_track(state: K<'_>) -> Result<Option<QueueDto>, CommandError> {
    state.kernel.next_track().map_err(map_err)
}

#[tauri::command]
pub fn prev_track(state: K<'_>) -> Result<QueueDto, CommandError> {
    state.kernel.prev_track().map_err(map_err)
}

#[tauri::command]
pub fn get_dsp(state: K<'_>) -> Result<aurora_dsp::DspConfig, CommandError> {
    state.kernel.get_dsp().map_err(map_err)
}

#[tauri::command]
pub fn set_dsp(
    config: aurora_dsp::DspConfig,
    state: K<'_>,
) -> Result<aurora_dsp::DspConfig, CommandError> {
    state.kernel.set_dsp(config).map_err(map_err)
}

/// Phase 5: probe WASAPI exclusive-mode support for the playing format.
#[tauri::command]
pub fn probe_exclusive(state: K<'_>) -> Result<ExclusiveProbe, CommandError> {
    state.kernel.probe_exclusive().map_err(map_err)
}

#[tauri::command]
pub fn recover_playback(replay: bool, state: K<'_>) -> Result<PlaybackStatus, CommandError> {
    state.kernel.recover_playback(replay).map_err(map_err)
}

#[tauri::command]
pub fn play_exclusive_track(
    request: PlayRequest,
    state: K<'_>,
) -> Result<ExclusivePlayResult, CommandError> {
    state.kernel.play_exclusive_track(request).map_err(map_err)
}

#[tauri::command]
pub fn verify_bit_perfect(
    seconds: Option<f64>,
    state: K<'_>,
) -> Result<BitPerfectVerdict, CommandError> {
    state.kernel.verify_bit_perfect(seconds).map_err(map_err)
}

// ---------------------------------------------------------------------------
// VST3 plugin hosting (ADR 0004, Phase 9)
// ---------------------------------------------------------------------------

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

#[tauri::command]
pub fn set_vst_chain(slots: Vec<VstSlotDto>, state: K<'_>) -> Result<PlaybackStatus, CommandError> {
    state.kernel.set_vst_chain(slots).map_err(map_err)
}

#[tauri::command]
pub fn get_vst_chain(state: K<'_>) -> Result<Vec<VstSlotDto>, CommandError> {
    state.kernel.get_vst_chain().map_err(map_err)
}

#[tauri::command]
pub fn test_vst_plugin(path: String, class_id: String) -> Result<String, CommandError> {
    aurora_vst::test_plugin_load(&path, &class_id)
        .map_err(|reason| CommandError::Engine { message: reason })
}

#[tauri::command]
pub fn open_vst_editor(path: String, class_id: String, state: K<'_>) -> Result<(), CommandError> {
    state
        .kernel
        .open_vst_editor(&path, &class_id)
        .map_err(map_err)
}

#[tauri::command]
pub fn close_vst_editor(state: K<'_>) -> Result<(), CommandError> {
    state.kernel.close_vst_editor().map_err(map_err)
}

#[tauri::command]
pub fn list_vst_params(path: String, class_id: String) -> Result<Vec<VstParamDto>, CommandError> {
    let params = aurora_vst::probe_params(&path, &class_id)
        .map_err(|reason| CommandError::Engine { message: reason })?;
    Ok(params
        .into_iter()
        .map(|p| VstParamDto {
            id: p.id,
            title: p.title,
            value: p.value,
        })
        .collect())
}

#[tauri::command]
pub fn set_vst_param(
    path: String,
    class_id: String,
    id: u32,
    value: f64,
    state: K<'_>,
) -> Result<(), CommandError> {
    state
        .kernel
        .set_vst_param(&path, &class_id, id, value)
        .map_err(map_err)
}

#[tauri::command]
pub fn save_vst_preset(
    path: String,
    class_id: String,
    name: String,
) -> Result<String, CommandError> {
    let file = aurora_vst::save_preset_file(&path, &class_id, &name)
        .map_err(|reason| CommandError::Engine { message: reason })?;
    Ok(file.display().to_string())
}

#[tauri::command]
pub fn load_vst_preset(
    path: String,
    class_id: String,
    preset_path: String,
    state: K<'_>,
) -> Result<(), CommandError> {
    state
        .kernel
        .load_vst_preset(&path, &class_id, &preset_path)
        .map_err(map_err)
}

#[tauri::command]
pub fn list_vst_presets(path: String) -> Result<Vec<VstPresetDto>, CommandError> {
    let presets = aurora_vst::list_preset_files(&path)
        .map_err(|reason| CommandError::Engine { message: reason })?;
    Ok(presets
        .into_iter()
        .map(|(name, file)| VstPresetDto {
            name,
            path: file.display().to_string(),
        })
        .collect())
}

// ---------------------------------------------------------------------------
// Spectrum / waveform / loop / bypass / chain persistence
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn playback_spectrum(state: K<'_>) -> Result<Vec<f32>, CommandError> {
    state.kernel.playback_spectrum().map_err(map_err)
}

/// Waveform peaks for the seek bar: `bins` absolute values (0..1) across
/// the whole file. Full decode, ~1s per 10-min FLAC — cached here so the
/// UI asks once per track.
#[tauri::command]
pub fn waveform(path: String, bins: u32) -> Result<Vec<f32>, CommandError> {
    let bins = bins.clamp(16, 2048) as usize;
    fn cache() -> &'static std::sync::Mutex<std::collections::HashMap<String, Vec<f32>>> {
        static CACHE: std::sync::OnceLock<
            std::sync::Mutex<std::collections::HashMap<String, Vec<f32>>>,
        > = std::sync::OnceLock::new();
        CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
    }
    let cache = cache();
    if let Some(hit) = cache.lock().ok().and_then(|m| m.get(&path).cloned()) {
        return Ok(hit);
    }
    let peaks = aurora_decoder::waveform_peaks(std::path::Path::new(&path), bins).map_err(|e| {
        CommandError::Engine {
            message: e.to_string(),
        }
    })?;
    if let Ok(mut map) = cache.lock() {
        // Bound: 32 tracks.
        if map.len() >= 32 {
            map.clear();
        }
        map.insert(path, peaks.clone());
    }
    Ok(peaks)
}

#[tauri::command]
pub fn set_loop(
    loop_start: Option<f64>,
    loop_end: Option<f64>,
    state: K<'_>,
) -> Result<Option<(f64, f64)>, CommandError> {
    state.kernel.set_loop(loop_start, loop_end).map_err(map_err)
}

#[tauri::command]
pub fn set_vst_slot_bypass(slot: usize, bypassed: bool, state: K<'_>) -> Result<(), CommandError> {
    state
        .kernel
        .set_vst_slot_bypass(slot, bypassed)
        .map_err(map_err)
}

#[tauri::command]
pub fn vst_bypassed_slots(state: K<'_>) -> Result<Vec<usize>, CommandError> {
    state.kernel.vst_bypassed_slots().map_err(map_err)
}

#[tauri::command]
pub async fn save_vst_chain_setting(
    slots: Option<Vec<VstSlotDto>>,
    state: K<'_>,
) -> Result<(), CommandError> {
    state
        .kernel
        .save_vst_chain_setting(slots)
        .await
        .map_err(map_err)
}

#[tauri::command]
pub async fn load_vst_chain_setting(state: K<'_>) -> Result<Option<Vec<VstSlotDto>>, CommandError> {
    state.kernel.load_vst_chain_setting().await.map_err(map_err)
}

// ---------------------------------------------------------------------------
// Artwork
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn get_track_artwork(path: String) -> Result<Option<String>, CommandError> {
    aurora_kernel::artwork::track_artwork(&path).map_err(map_err)
}

// ---------------------------------------------------------------------------
// DJ decks (Pioneer DDJ-1000-style, Phase 10)
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn deck_load(
    deck: usize,
    path: String,
    title: Option<String>,
    artist: Option<String>,
    state: K<'_>,
) -> Result<DeckDto, CommandError> {
    state
        .kernel
        .deck_load(deck, path, title, artist)
        .await
        .map_err(map_err)
}

#[tauri::command]
pub fn deck_play(deck: usize, state: K<'_>) -> Result<DeckDto, CommandError> {
    state.kernel.deck_play(deck).map_err(map_err)
}

#[tauri::command]
pub fn deck_stop(deck: usize, state: K<'_>) -> Result<DeckDto, CommandError> {
    state.kernel.deck_stop(deck).map_err(map_err)
}

#[tauri::command]
pub fn deck_seek(deck: usize, position_s: f64, state: K<'_>) -> Result<(), CommandError> {
    state.kernel.deck_seek(deck, position_s).map_err(map_err)
}

#[tauri::command]
pub fn deck_fader(deck: usize, value: f64, state: K<'_>) -> Result<DeckDto, CommandError> {
    state.kernel.deck_fader(deck, value).map_err(map_err)
}

#[tauri::command]
pub fn set_crossfader(value: f64, state: K<'_>) -> Result<[DeckDto; 2], CommandError> {
    state.kernel.set_crossfader(value).map_err(map_err)
}

#[tauri::command]
pub fn deck_set_cue(
    deck: usize,
    slot: usize,
    position: Option<f64>,
    state: K<'_>,
) -> Result<Vec<Option<f64>>, CommandError> {
    state
        .kernel
        .deck_set_cue(deck, slot, position)
        .map_err(map_err)
}

#[tauri::command]
pub fn deck_jump_cue(deck: usize, slot: usize, state: K<'_>) -> Result<(), CommandError> {
    state.kernel.deck_jump_cue(deck, slot).map_err(map_err)
}

#[tauri::command]
pub fn deck_tempo(deck: usize, speed: f64, state: K<'_>) -> Result<DeckDto, CommandError> {
    state.kernel.deck_tempo(deck, speed).map_err(map_err)
}

#[tauri::command]
pub fn deck_status(state: K<'_>) -> Result<[DeckDto; 2], CommandError> {
    state.kernel.deck_status().map_err(map_err)
}

#[tauri::command]
pub async fn track_bpm(path: String, state: K<'_>) -> Result<Option<f64>, CommandError> {
    state.kernel.track_bpm(&path).await.map_err(map_err)
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

pub fn builder() -> tauri::Builder<tauri::Wry> {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |app, shortcut, event| {
                    if event.state != ShortcutState::Pressed {
                        return;
                    }
                    // Media keys: transport control from anywhere (keyboard,
                    // headphones, remote). Engine commands are shared with
                    // the UI buttons; queue order lives in the frontend.
                    let app_state = app.clone();
                    let key = shortcut.to_string();
                    let task = if key.contains("MediaPlayPause") {
                        "playpause"
                    } else if key.contains("MediaStop") {
                        "stop"
                    } else if key.contains("MediaTrackNext") {
                        "next"
                    } else if key.contains("MediaTrackPrevious") {
                        "prev"
                    } else {
                        return;
                    };
                    tauri::async_runtime::spawn(async move {
                        let state = app_state.state::<AppState>();
                        let kernel = state.kernel.clone();
                        match task {
                            "stop" => {
                                let _ = kernel.stop_playback();
                            }
                            "playpause" => {
                                let playing = kernel.engine.lock().map(|e| e.state()).ok()
                                    == Some(aurora_audio::EngineState::Playing);
                                if playing {
                                    let _ = kernel.pause_playback();
                                } else {
                                    let _ = kernel.resume_playback();
                                }
                            }
                            "next" => {
                                // The queue order is frontend state; ask it
                                // to advance (the frontend calls next_track).
                                let _ = app_state.emit("media-next", ());
                            }
                            "prev" => {
                                let _ = app_state.emit("media-prev", ());
                            }
                            _ => {}
                        }
                    });
                })
                .with_shortcut("MediaPlayPause")
                .expect("MediaPlayPause")
                .with_shortcut("MediaStop")
                .expect("MediaStop")
                .with_shortcut("MediaTrackNext")
                .expect("MediaTrackNext")
                .with_shortcut("MediaTrackPrevious")
                .expect("MediaTrackPrevious")
                .build(),
        )
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
            import_playlist,
            export_playlist,
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
            list_vst_params,
            set_vst_param,
            save_vst_preset,
            load_vst_preset,
            list_vst_presets,
            playback_spectrum,
            waveform,
            set_loop,
            set_vst_slot_bypass,
            vst_bypassed_slots,
            save_vst_chain_setting,
            load_vst_chain_setting,
            deck_load,
            deck_play,
            deck_stop,
            deck_seek,
            deck_fader,
            set_crossfader,
            deck_set_cue,
            deck_jump_cue,
            deck_tempo,
            deck_status,
            track_bpm,
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
                        let kernel = state.kernel.clone();
                        match aurora_library::LibraryService::open(&db_path).await {
                            Ok(service) => {
                                let count = aurora_storage::track_count(service.pool())
                                    .await
                                    .unwrap_or(0);
                                // Start the folder watcher BEFORE the pool
                                // moves into the kernel: it clones the pool
                                // and never touches the kernel's library slot.
                                let roots = aurora_storage::library_roots(service.pool())
                                    .await
                                    .unwrap_or_default()
                                    .into_iter()
                                    .map(std::path::PathBuf::from)
                                    .collect::<Vec<_>>();
                                aurora_kernel::watch::spawn_library_watcher(
                                    service.pool().clone(),
                                    roots,
                                    kernel.events.clone(),
                                );
                                *kernel.library.lock().await = Some(service);
                                *kernel.library_path_slot.lock().expect("path slot") =
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
                        // Restore the persisted VST chain + output device.
                        if let Ok(Some(slots)) = kernel.load_vst_chain_setting().await {
                            let empty = slots.is_empty();
                            if !empty {
                                let _ = kernel.set_vst_chain(slots);
                                tracing::info!("vst chain restored");
                            }
                        }
                        if let Ok(Some(key)) = kernel.get_setting("output_device").await {
                            *kernel.device_key.lock().expect("device key") = Some(key);
                        }
                    });
                }
            }

            // Forward kernel events to the frontend (Tauri events).
            let handle = app.handle().clone();
            let state = app.state::<AppState>();
            let kernel = state.kernel.clone();
            let _ = kernel.events.subscribe(move |event| match event {
                aurora_kernel::KernelEvent::LibraryChanged {
                    added,
                    pruned,
                    updated,
                } => {
                    let _ = handle.emit(
                        "library-changed",
                        serde_json::json!({
                            "added": added,
                            "pruned": pruned,
                            "updated": updated,
                        }),
                    );
                }
            });
            Ok(())
        })
}
