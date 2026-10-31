//! Aurora audio engine: device output and playback orchestration.
//!
//! Composes the decoder (worker thread) and CPAL output (real-time
//! callback) around a SPSC ring buffer. Knows nothing about storage,
//! library logic, or the UI (see ARCHITECTURE.md).

pub mod device;
pub mod engine;
pub mod error;
pub mod metrics;

pub use device::{OutputDevice, default_output_device, output_devices};
pub use engine::{EngineState, PlaybackEngine};
pub use error::EngineError;
pub use metrics::PlaybackMetrics;
