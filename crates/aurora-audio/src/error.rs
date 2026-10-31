//! Audio engine error taxonomy.

use thiserror::Error;

/// Device enumeration and stream setup failures.
#[derive(Debug, Error)]
pub enum EngineError {
    #[error("no output devices available")]
    NoOutputDevices,

    #[error("device not found: {0}")]
    DeviceNotFound(String),

    #[error("the requested device cannot play this format: {0}")]
    FormatUnsupported(String),

    #[error("failed to build output stream: {0}")]
    StreamBuild(String),

    #[error("the engine is busy with another source")]
    AlreadyPlaying,

    #[error("no source is loaded")]
    NoSourceLoaded,

    #[error("decoder failure: {0}")]
    Decoder(#[from] aurora_decoder::SourceError),

    #[error("internal engine failure: {0}")]
    Internal(String),
}
