//! Lock-free playback metrics shared between the callback and the app.
//!
//! The callback updates counters with `Relaxed` atomics only; readers
//! (commands, UI events) accept approximate values.

use std::sync::atomic::{AtomicU64, Ordering};

/// Counters the audio callback maintains.
pub struct PlaybackMetrics {
    frames_played: AtomicU64,
    underruns: AtomicU64,
}

impl PlaybackMetrics {
    pub fn new() -> Self {
        Self {
            frames_played: AtomicU64::new(0),
            underruns: AtomicU64::new(0),
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

    pub(crate) fn reset(&self) {
        self.frames_played.store(0, Ordering::Relaxed);
        self.underruns.store(0, Ordering::Relaxed);
    }
}

impl Default for PlaybackMetrics {
    fn default() -> Self {
        Self::new()
    }
}
