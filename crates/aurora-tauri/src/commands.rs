//! Tauri command surface.
//!
//! Commands are thin stateless wrappers over backend services; as services
//! arrive they will be injected via managed state, not constructed here.

use aurora_core::PlaybackMode;

#[tauri::command]
pub fn app_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[tauri::command]
pub fn app_name() -> &'static str {
    "Aurora"
}

/// Report the playback modes the backend understands. Frontend UI state is a
/// projection of backend state (ARCHITECTURE.md invariant 7): the source of
/// truth for the enum lives in `aurora-core`, surfaced here for the UI.
#[tauri::command]
pub fn playback_modes() -> Vec<PlaybackMode> {
    vec![
        PlaybackMode::BitPerfect,
        PlaybackMode::Dsp,
        PlaybackMode::Auto,
    ]
}

pub fn builder() -> tauri::Builder<tauri::Wry> {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            app_version,
            app_name,
            playback_modes
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
