//! Lock-free playback metrics shared between the callback and the app.
//!
//! The callback updates counters with `Relaxed` atomics only; readers
//! (commands, UI events) accept approximate values.

use std::sync::atomic::{AtomicU64, Ordering};

/// Counters the audio callback maintains.
pub struct PlaybackMetrics {
    frames_played: AtomicU64,
    underruns: AtomicU64,
    /// Real-time spectrum: 16 log-spaced band energies (0..1), written by
    /// the DECODE WORKER (non-RT; coarse atomic pairs per band, readers
    /// accept torn values within a band — display-only). Updated per
    /// chunk; the UI polls `spectrum()`.
    spectrum: [AtomicU64; 16],
}

impl PlaybackMetrics {
    pub fn new() -> Self {
        Self {
            frames_played: AtomicU64::new(0),
            underruns: AtomicU64::new(0),
            spectrum: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }

    pub(crate) fn add_frames(&self, frames: u64) {
        self.frames_played.fetch_add(frames, Ordering::Relaxed);
    }

    pub(crate) fn add_underrun(&self, frames: u64) {
        self.underruns.fetch_add(frames, Ordering::Relaxed);
    }

    /// Frames output since the last reset.
    pub fn frames_played(&self) -> u64 {
        self.frames_played.load(Ordering::Relaxed)
    }

    /// Frames of silence emitted due to buffer starvation.
    pub fn underruns(&self) -> u64 {
        self.underruns.load(Ordering::Relaxed)
    }

    /// Publish a new spectrum snapshot (worker side, non-RT). Values are
    /// f32 energies 0..1, quantized to u64 fixed-point (1e6 scale).
    pub(crate) fn publish_spectrum(&self, bands: &[f32]) {
        for (i, slot) in self.spectrum.iter().enumerate() {
            let v = bands.get(i).copied().unwrap_or(0.0);
            let q = (v.clamp(0.0, 1.0) * 1e6) as u64;
            slot.store(q, Ordering::Relaxed);
        }
    }

    /// The current spectrum snapshot (16 bands, 0..1).
    pub fn spectrum(&self) -> Vec<f32> {
        self.spectrum
            .iter()
            .map(|s| s.load(Ordering::Relaxed) as f32 / 1e6)
            .collect()
    }

    pub(crate) fn reset(&self) {
        self.frames_played.store(0, Ordering::Relaxed);
        self.underruns.store(0, Ordering::Relaxed);
        for slot in &self.spectrum {
            slot.store(0, Ordering::Relaxed);
        }
    }
}

impl Default for PlaybackMetrics {
    fn default() -> Self {
        Self::new()
    }
}
