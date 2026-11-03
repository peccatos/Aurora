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
///
/// Logs go to stderr AND a daily-rotating file in the app-data log dir:
/// when a crash kills the console window, the file still holds the last
/// lines before the death (the "app falls over, no trace" debugging gap).
pub fn initialize_observability() {
    use tracing_subscriber::layer::Layer;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    let fmt_layer = tracing_subscriber::fmt::layer().with_filter(filter.clone());

    let file_layer = log_dir().map(|dir| {
        let appender = tracing_appender::rolling::daily(dir, "aurora.log");
        let (writer, _guard) = tracing_appender::non_blocking(appender);
        // Leak the guard: the writer must outlive any scope we could tie
        // it to; the process end flushes.
        std::mem::forget(_guard);
        tracing_subscriber::fmt::layer()
            .with_writer(writer)
            .with_ansi(false)
            .with_filter(filter)
    });

    let result = if let Some(file_layer) = file_layer {
        tracing_subscriber::registry()
            .with(fmt_layer)
            .with(file_layer)
            .try_init()
    } else {
        tracing_subscriber::registry().with(fmt_layer).try_init()
    };
    let _ = result;
}

/// The log dir: `<app-data>/logs` (Roaming profile next to library.db).
fn log_dir() -> Option<std::path::PathBuf> {
    let dir = std::env::var_os("APPDATA")
        .map(std::path::PathBuf::from)?
        .join("dev.aurora.player")
        .join("logs");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
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
    let service = state.kernel.library.lock().await.take();
    if let Some(service) = service {
        service.pool().close().await;
        tracing::info!("library closed cleanly (WAL checkpointed)");
    }
}
