//! Error surface for Tauri commands: serializable, never panicking.
//!
//! With the kernel split (ADR 0005) the adapter's errors are translations
//! of `KernelError` — this enum is the wire shape, the kernel owns the
//! taxonomy.

use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CommandError {
    Poisoned,
    #[serde(rename = "engine")]
    Engine {
        message: String,
    },
    #[serde(rename = "device")]
    Device(String),
    Library {
        message: String,
    },
    #[serde(rename = "library_not_open")]
    LibraryNotOpen,
    Metadata {
        message: String,
    },
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommandError::Poisoned => write!(f, "internal state is poisoned"),
            CommandError::Engine { message } => write!(f, "engine error: {message}"),
            CommandError::Device(key) => write!(f, "device not found: {key}"),
            CommandError::Library { message } => write!(f, "library error: {message}"),
            CommandError::LibraryNotOpen => write!(f, "library is not open"),
            CommandError::Metadata { message } => write!(f, "metadata error: {message}"),
        }
    }
}

impl std::error::Error for CommandError {}

impl From<aurora_kernel::KernelError> for CommandError {
    fn from(err: aurora_kernel::KernelError) -> Self {
        match err {
            aurora_kernel::KernelError::Poisoned => CommandError::Poisoned,
            aurora_kernel::KernelError::Engine { message } => CommandError::Engine { message },
            aurora_kernel::KernelError::Device(key) => CommandError::Device(key),
            aurora_kernel::KernelError::Library { message } => CommandError::Library { message },
            aurora_kernel::KernelError::LibraryNotOpen => CommandError::LibraryNotOpen,
            aurora_kernel::KernelError::Metadata { message } => CommandError::Metadata { message },
        }
    }
}
