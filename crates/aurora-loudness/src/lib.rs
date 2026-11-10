//! Aurora loudness: EBU R128 / ITU-R BS.1770-4 measurement and
//! ReplayGain 2.0 gain derivation (ADR 0002 honest measurement).
//!
//! Split out of `aurora-dsp` (where a partial `lufs` module lived) so the
//! scanner is a pure, dependency-light measurement core:
//! K-weighting (spec biquads at 48 kHz, exactly-computed coefficients at
//! other rates), 400 ms gating blocks at 100 ms hops, -70 LUFS absolute
//! gate and -10 LU relative gate - the BS.1770-4 integrated loudness.
//!
//! ReplayGain 2.0 (rg2 spec) expresses normalization as a plain dB gain
//! against a target loudness: `gain_db = target_lufs - integrated_lufs`
//! (rg2 target -18; the JS shell historically used the -14 streaming
//! convention - both are parameterized here).
//!
//! Honesty: the number is a MEASUREMENT of decoded PCM, computed
//! identically on every platform. `None` means "not measurable" (too
//! short, silence) - never a fabricated value.

pub mod scanner;

pub use scanner::{LoudnessScan, analyze_source, integrated_lufs};

/// ReplayGain 2.0 target: -18 LUFS (the rg2 reference listening level).
pub const RG2_TARGET_LUFS: f64 = -18.0;

/// Streaming-platform normalization convention (-14 LUFS), kept for
/// callers that prefer it (the web shell historical target).
pub const STREAMING_TARGET_LUFS: f64 = -14.0;

/// ReplayGain 2.0 gain for a measured loudness: how many dB to apply so
/// the track plays at `target` LUFS. Positive for quiet tracks, negative
/// for loud ones. `None` in -> `None` out (no measurement, no gain).
pub fn replaygain2_db(integrated_lufs: Option<f64>, target_lufs: f64) -> Option<f64> {
    integrated_lufs.map(|lufs| target_lufs - lufs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rg2_gain_math() {
        assert!((replaygain2_db(Some(-12.0), RG2_TARGET_LUFS).unwrap() + 6.0).abs() < 1e-9);
        assert!((replaygain2_db(Some(-23.0), RG2_TARGET_LUFS).unwrap() - 5.0).abs() < 1e-9);
        assert!(replaygain2_db(None, RG2_TARGET_LUFS).is_none());
        assert!((replaygain2_db(Some(-10.0), STREAMING_TARGET_LUFS).unwrap() + 4.0).abs() < 1e-9);
    }
}
