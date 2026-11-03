//! Kernel error taxonomy.
//!
//! Mirrors the adapter's `CommandError` shape but lives in the kernel:
//! shells map it to their own transport errors (Tauri command error,
//! HTTP status, wasm exception) without knowing the internals.

#[derive(Debug, thiserror::Error)]
pub enum KernelError {
    #[error("a lock is poisoned")]
    Poisoned,
    #[error("engine error: {message}")]
    Engine { message: String },
    #[error("device error: {0}")]
    Device(String),
    #[error("library error: {message}")]
    Library { message: String },
    #[error("the library is not open")]
    LibraryNotOpen,
    #[error("metadata error: {message}")]
    Metadata { message: String },
}
impl From<aurora_audio::EngineError> for KernelError {
    fn from(e: aurora_audio::EngineError) -> Self {
        KernelError::Engine {
            message: e.to_string(),
        }
    }
}

impl From<aurora_metadata::MetadataError> for KernelError {
    fn from(e: aurora_metadata::MetadataError) -> Self {
        KernelError::Metadata {
            message: e.to_string(),
        }
    }
}

impl From<aurora_storage::StorageError> for KernelError {
    fn from(e: aurora_storage::StorageError) -> Self {
        KernelError::Library {
            message: e.to_string(),
        }
    }
}
