//! Error surface for Tauri commands: serializable, never panicking.

use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CommandError {
    Poisoned,
    Engine { message: String },
    Device(String),
    Library { message: String },
    LibraryNotOpen,
    Metadata { message: String },
    Credential { message: String },
    Provider { message: String },
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Poisoned => write!(f, "internal state is poisoned"),
            Self::Engine { message } => write!(f, "engine error: {message}"),
            Self::Device(key) => write!(f, "device not found: {key}"),
            Self::Library { message } => write!(f, "library error: {message}"),
            Self::LibraryNotOpen => write!(f, "library is not open"),
            Self::Metadata { message } => write!(f, "metadata error: {message}"),
            Self::Credential { message } => write!(f, "credential error: {message}"),
            Self::Provider { message } => write!(f, "provider error: {message}"),
        }
    }
}
impl std::error::Error for CommandError {}
impl From<aurora_kernel::KernelError> for CommandError {
    fn from(err: aurora_kernel::KernelError) -> Self {
        match err {
            aurora_kernel::KernelError::Poisoned => Self::Poisoned,
            aurora_kernel::KernelError::Engine { message } => Self::Engine { message },
            aurora_kernel::KernelError::Device(key) => Self::Device(key),
            aurora_kernel::KernelError::Library { message } => Self::Library { message },
            aurora_kernel::KernelError::LibraryNotOpen => Self::LibraryNotOpen,
            aurora_kernel::KernelError::Metadata { message } => Self::Metadata { message },
        }
    }
}
// Provider errors are intentionally mapped to a safe message; secrets never
// form part of ProviderError.
