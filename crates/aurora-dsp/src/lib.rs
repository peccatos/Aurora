//! Aurora DSP: optional signal processing and resampling.
//!
//! Depends only on `aurora-core` (domain types) and rubato (resampling).
//! Knows nothing about devices, storage, or the UI. The pipeline runs on
//! the decode worker thread — never in the real-time audio callback.
//!
//! Honest-processing contract (ADR 0002): every processor reports its
//! `AudioTransformation`; a bypassed pipeline is bit-exact identity,
//! verified by tests.

pub mod biquad;
pub mod eq;
pub mod error;
pub mod gain;
pub mod pipeline;
pub mod resample;

pub use biquad::{Biquad, BiquadKind, BiquadParams};
pub use eq::{EqBand, EqBandKind, EqChain, Equalizer};
pub use error::DspError;
pub use gain::{Gain, ReplayGainMode, TpdfDither};
pub use pipeline::{DspConfig, DspPipeline, ReplayGainModeDef};
pub use resample::Resampler;
