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
#[cfg(target_family = "wasm")]
pub(crate) mod web_worker;

#[cfg(feature = "win-wasapi")]
pub mod exclusive;
#[cfg(feature = "win-wasapi")]
pub mod exclusive_stream;
#[cfg(feature = "win-wasapi")]
pub mod loopback;

/// No-op VST3 stand-in for the wasm build (Aurora Web): native plugins
/// cannot load in a browser. Identity processing, honest empty chains.
#[cfg(target_family = "wasm")]
pub(crate) mod vst_stub;

/// The VST chain types the engine integrates with: the real host
/// natively, the no-op stub on wasm. Same wire shape on both.
#[cfg(not(target_family = "wasm"))]
pub use aurora_vst::VstPluginList;
#[cfg(target_family = "wasm")]
pub use vst_stub::VstPluginList;

pub use device::{OutputDevice, default_output_device, output_devices};
pub use engine::{EngineState, PlaybackEngine};
pub use error::EngineError;
pub use metrics::PlaybackMetrics;

#[cfg(feature = "win-wasapi")]
pub use exclusive::{exclusive_integrity, probe_exclusive_support};
