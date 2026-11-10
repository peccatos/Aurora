//! R128 loudness measurement - MOVED to the `aurora-loudness` crate.
//!
//! The scanner (K-weighting + BS.1770-4 gating) now lives in
//! `aurora_loudness::scanner` with corrected reference behavior and a
//! broader test suite. This shim re-exports it so existing call sites
//! (`aurora_web::analyze_lufs`) keep compiling; new code should depend
//! on `aurora-loudness` directly.
//!
//! NOTE: the previous implementation had a K-weighting defect (the
//! 48 kHz table path added ~+6 dB); the moved scanner fixes it. Values
//! from the old code and the new code differ by that amount - the new
//! numbers match the R128 reference (a -23 dBFS stereo 1 kHz sine
//! measures -23 LUFS).
pub use aurora_loudness::scanner::integrated_lufs;
