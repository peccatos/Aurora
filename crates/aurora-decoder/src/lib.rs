//! Aurora decoder: source decoding into typed PCM.
//!
//! Depends only on `aurora-core` and symphonia. Knows nothing about output
//! devices, playback control, storage or the UI (see ARCHITECTURE.md).
//!
//! wasm builds additionally have an in-memory file registry: paths of
//! the form "web://<id>" resolve to bytes registered by the JS shell
//! (the browser has no filesystem). Native builds never see such paths.

pub mod error;
pub mod pcm;
pub mod source;
pub mod tempo;

#[cfg(target_family = "wasm")]
pub mod web_files;

pub use error::{DecodeError, SourceError};
pub use pcm::{PcmChunk, frame_size_bytes};
pub use source::{AudioSource, SourceInfo, waveform_peaks};
pub use tempo::detect_bpm;
