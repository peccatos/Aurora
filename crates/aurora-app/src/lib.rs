//! Application composition root.
//!
//! This crate assembles backend services and owns the Tauri builder wiring.
//! The desktop binary owns only Tauri's generated context (which requires
//! compile-time assets that live with the executable) and delegates all
//! composition to this crate. As implementations arrive in later phases,
//! services are constructed and injected here — never inside the tauri
//! adapter, which stays a thin command/event boundary.

use aurora_tauri::commands;

/// Initialize process-wide observability (tracing).
pub fn initialize_observability() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}

/// Compose the full application and hand the ready builder to the caller.
///
/// The caller supplies Tauri's generated context (`tauri::generate_context!`),
/// which must be expanded inside the binary crate.
pub fn compose() -> tauri::Builder<tauri::Wry> {
    commands::builder()
}
