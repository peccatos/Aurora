//! DSP error taxonomy.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum DspError {
    #[error("sample rate must be positive, got {0}")]
    InvalidSampleRate(u32),

    #[error("channel count must be positive, got {0}")]
    InvalidChannels(u16),

    #[error("band frequency {0} Hz is outside the Nyquist range for {1} Hz")]
    FrequencyAboveNyquist(f64, u32),

    #[error("resample ratio {0} is not supported ({1})")]
    UnsupportedRatio(f64, &'static str),

    #[error("processor graph contains a cycle")]
    GraphCycle,

    #[error("internal DSP failure: {0}")]
    Internal(String),
}
