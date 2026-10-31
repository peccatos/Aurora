//! Aurora decoder: source decoding into typed PCM.
//!
//! Depends only on `aurora-core` and symphonia. Knows nothing about output
//! devices, playback control, storage or the UI (see ARCHITECTURE.md).

pub mod error;
pub mod pcm;
pub mod source;

pub use error::{DecodeError, SourceError};
pub use pcm::{PcmChunk, frame_size_bytes};
pub use source::{AudioSource, SourceInfo};
