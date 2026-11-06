//! Tauri adapter: a thin shell over the kernel (ADR 0005).
//!
//! One command = one kernel call + DTO translation; kernel events are
//! forwarded to the frontend. No business logic lives here — the music
//! player is `aurora-kernel`.

pub mod commands;
pub mod credentials;
pub mod error;

pub use commands::builder;
