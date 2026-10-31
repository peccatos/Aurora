//! Tauri adapter: exposes application use cases to the frontend as typed
//! commands and events.
//!
//! This crate is a boundary, not a composition root: it wires the invoke
//! handler and publishes builder hooks, while `aurora-app` decides what
//! services exist.

pub mod commands;

pub use commands::builder;
