//! Decoder error taxonomy.
//!
//! Errors are split by recoverability, mirroring the symphonia decode-loop
//! contract: per-packet failures are recoverable (skip), source-level
//! failures end the stream cleanly, and only genuine bugs escalate.

use thiserror::Error;

/// Failures that occur while probing or setting up a source.
#[derive(Debug, Error)]
pub enum SourceError {
    #[error("the media source could not be read: {0}")]
    Io(#[from] std::io::Error),

    #[error("no supported format detected")]
    UnsupportedFormat,

    #[error("no decodable audio track found")]
    NoAudioTrack,

    #[error("the audio codec is not supported: {0}")]
    UnsupportedCodec(String),

    #[error("the track metadata is incomplete: {0}")]
    IncompleteTrack(String),

    #[error("the stream must be re-probed (chained stream)")]
    ResetRequired,
}

/// Outcome of a decode step.
#[derive(Debug, Error)]
pub enum DecodeError {
    #[error("the source is fully decoded")]
    EndOfStream,

    /// The packet was invalid or produced an IO error; decoding continues
    /// with the next packet. The caller decides whether to surface it.
    #[error("packet skipped after recoverable failure: {0}")]
    Recoverable(String),

    #[error("unrecoverable decode failure: {0}")]
    Unrecoverable(String),

    #[error("the decoder was used after a fatal error")]
    Poisoned,
}

impl From<symphonia::core::errors::Error> for DecodeError {
    fn from(err: symphonia::core::errors::Error) -> Self {
        use symphonia::core::errors::Error as SymphoniaError;
        use symphonia::core::errors::SeekErrorKind;
        match err {
            // End-of-stream is a clean stop, not a failure.
            SymphoniaError::IoError(ref io) if io.kind() == std::io::ErrorKind::UnexpectedEof => {
                DecodeError::EndOfStream
            }
            SymphoniaError::IoError(io) => DecodeError::Recoverable(format!("i/o: {io}")),
            SymphoniaError::DecodeError(msg) => DecodeError::Recoverable(msg.to_string()),
            SymphoniaError::SeekError(SeekErrorKind::OutOfRange) => DecodeError::EndOfStream,
            SymphoniaError::SeekError(kind) => DecodeError::Recoverable(format!("seek: {kind:?}")),
            other => DecodeError::Unrecoverable(other.to_string()),
        }
    }
}
