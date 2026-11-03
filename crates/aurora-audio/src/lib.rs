//! Aurora audio engine: device output and playback orchestration.
//!
//! Composes the decoder (worker thread) and CPAL output (real-time
//! callback) around a SPSC ring buffer. Knows nothing about storage,
//! library logic, or the UI (see ARCHITECTURE.md).
//!
//! The `win-wasapi` feature (default on Windows builds) adds the WASAPI
//! exclusive path + loopback verification. Without it the shared CPAL
//! path works everywhere, and exclusive/verification APIs honestly
//! report "unsupported on this build" — never a fake verdict (ADR 0002).

pub mod device;
pub mod engine;
pub mod error;
pub mod metrics;
pub mod spectrum;

#[cfg(feature = "win-wasapi")]
pub mod exclusive;
#[cfg(feature = "win-wasapi")]
pub mod exclusive_stream;
#[cfg(feature = "win-wasapi")]
pub mod loopback;

pub use device::{OutputDevice, default_output_device, output_devices};
pub use engine::{EngineState, PlaybackEngine};
pub use error::EngineError;
pub use metrics::PlaybackMetrics;

#[cfg(feature = "win-wasapi")]
pub use exclusive::{exclusive_integrity, probe_exclusive_support};
