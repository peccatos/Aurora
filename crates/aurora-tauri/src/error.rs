//! Error surface for Tauri commands: serializable, never panicking.

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
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommandError::Poisoned => write!(f, "internal state is poisoned"),
            CommandError::Engine { message } => write!(f, "engine error: {message}"),
            CommandError::Device(key) => write!(f, "device not found: {key}"),
        }
    }
}

impl std::error::Error for CommandError {}

impl From<aurora_audio::EngineError> for CommandError {
    fn from(err: aurora_audio::EngineError) -> Self {
        CommandError::Engine {
            message: err.to_string(),
        }
    }
}

// Tauri requires errors to implement Serialize (already derived) — no
// From<std::io::Error> needed since commands do not do IO directly.
