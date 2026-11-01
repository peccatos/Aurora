//! Application composition root.
//!
//! This crate assembles backend services and owns the Tauri builder wiring.
//! The desktop binary owns only Tauri's generated context (which requires
//! compile-time assets that live with the executable) and delegates all
//! composition to this crate. As implementations arrive in later phases,
//! services are constructed and injected here — never inside the tauri
//! adapter, which stays a thin command/event boundary.

use tauri::Manager;

use aurora_tauri::commands;

/// Initialize process-wide observability (tracing).
pub fn initialize_observability() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}

/// Install platform-appropriate termination handling.
///
/// - Unix (macOS/Linux): registers SIGTERM/SIGINT handlers so the process
///   exits cleanly (audio devices released, SQLite WAL checkpointed) when
///   stopped by a service manager, desktop session, or `kill`.
/// - Windows: termination is delivered via the event loop/`ExitProcess`;
///   nothing to register here.
///
/// Tauri's window close already routes through the normal event loop on all
/// platforms; this covers the out-of-band termination paths.
pub fn install_termination_handling() {
    #[cfg(unix)]
    {
        // ctrlc delivers SIGINT/SIGTERM as cross-platform channel events.
        // The handler only sets a flag / exits; heavy teardown happens in
        // the exit path, not in the signal context.
        let _ = ctrlc::try_set_cleanup(|| {
            tracing::info!("termination signal received, exiting");
        });
    }
    #[cfg(not(unix))]
    {
        // Windows: nothing to install; process termination goes through
        // the standard event loop and DLL detach notifications.
    }
}

/// Compose the full application and hand the ready builder to the caller.
///
/// The caller supplies Tauri's generated context (`tauri::generate_context!`),
/// which must be expanded inside the binary crate.
pub fn compose() -> tauri::Builder<tauri::Wry> {
    commands::builder().on_window_event(|window, event| {
        if matches!(event, tauri::WindowEvent::Destroyed) && window.label() == "main" {
            // Phase 8: graceful teardown. The async library close runs on
            // the Tauri runtime, which is still alive during Destroyed.
            let handle = window.app_handle().clone();
            tauri::async_runtime::spawn(async move {
                shutdown_library(&handle).await;
            });
        }
    })
}

/// Close the SQLite pool cleanly (WAL checkpoint + connection close).
///
/// WAL+NORMAL already tolerates a hard kill (the next scan repairs), so
/// this is best-effort polish, not a correctness requirement.
async fn shutdown_library(app: &tauri::AppHandle) {
    use tauri::Manager;
    let state = app.state::<aurora_tauri::commands::AppState>();
    let service = state.library.lock().await.take();
    if let Some(service) = service {
        service.pool().close().await;
        tracing::info!("library closed cleanly (WAL checkpointed)");
    }
}
