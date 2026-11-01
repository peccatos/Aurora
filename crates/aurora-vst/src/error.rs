//! Errors for the VST3 hosting layer.

/// Errors from the VST3 plugin layer.
#[derive(Debug, thiserror::Error)]
pub enum VstError {
    /// The host library failed to construct or scan.
    #[error("vst3 host error: {0}")]
    Host(String),
    /// A plugin failed to load (missing, corrupted, refused in isolation).
    #[error("plugin load failed: {0}")]
    Load(String),
    /// A processing error from the plugin (isolated process reported it).
    #[error("plugin processing failed: {0}")]
    Processing(String),
    /// The plugin chain was asked to process with a bus/channel layout it
    /// cannot map (e.g. more channels than the chain was built for).
    #[error("channel layout mismatch: {0}")]
    Layout(String),
}

impl From<vst3_host::Error> for VstError {
    fn from(e: vst3_host::Error) -> Self {
        VstError::Host(e.to_string())
    }
}
