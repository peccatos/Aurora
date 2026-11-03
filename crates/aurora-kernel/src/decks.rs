//! DJ decks (Pioneer DDJ-1000-style): each deck is a fully independent
//! playback stream — own engine, own DSP, own cue points. The crossfader
//! applies an equal-power curve to the per-deck channel gain.

use aurora_audio::PlaybackEngine;

use crate::queue::QueueTrack;

/// One DJ deck: engine + channel fader + hot cues.
pub struct Deck {
    pub engine: PlaybackEngine,
    /// Channel fader 0..1 (the user's own volume, BEFORE crossfader).
    pub channel_fader: f64,
    /// The loaded track (path + display), for the deck's UI.
    pub loaded: Option<QueueTrack>,
    /// Hot cues: 8 slots, positions in seconds.
    pub cues: [Option<f64>; 8],
    /// The deck's own DSP config (EQ/tempo/fader gain).
    pub dsp: aurora_dsp::DspConfig,
}

impl Deck {
    pub fn new() -> Self {
        Self {
            engine: PlaybackEngine::new(),
            channel_fader: 1.0,
            loaded: None,
            cues: std::array::from_fn(|_| None),
            dsp: aurora_dsp::DspConfig::default(),
        }
    }

    /// Total gain in dB from fader + crossfader (equal-power curve):
    /// A gets cos(x·π/2), B gets sin(x·π/2) — at center (x=0.5) both are
    /// -3dB, at x=0 deck A is full and B silent, at x=1 vice versa.
    pub fn gain_db(channel_fader: f64, crossfader: f64, is_deck_b: bool) -> f64 {
        let fader = channel_fader.clamp(0.0, 1.0);
        // Fader: dB scale (0 -> -inf.. we clamp at -60dB silence).
        let fader_db = if fader <= 0.001 {
            -60.0
        } else {
            60.0 * fader.log10() // 1.0 -> 0dB, 0.5 -> -18dB, 0.1 -> -60dB
        };
        let x = crossfader.clamp(0.0, 1.0);
        let cf = if is_deck_b {
            (x * std::f64::consts::FRAC_PI_2).sin()
        } else {
            (x * std::f64::consts::FRAC_PI_2).cos()
        };
        let cf_db = if cf <= 0.001 {
            -60.0
        } else {
            20.0 * cf.log10()
        };
        fader_db + cf_db
    }

    /// Push the computed gain into the deck's live DSP volume (keeping
    /// the user's other DSP settings).
    pub fn apply_volume(&mut self, gain_db: f64) {
        self.dsp.volume_db = gain_db as f32;
        self.engine.set_dsp_config(self.dsp.clone());
        // Native-path streams have no pipeline: a DSP change requests a
        // restart through the DSP path (honest; the transition is
        // audible but correct).
        if self.engine.take_dsp_transition() {
            let _ = self.engine.restart_for_dsp();
        }
    }
}

impl Default for Deck {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_power_center_is_minus_3db_each_side() {
        let a = Deck::gain_db(1.0, 0.5, false);
        let b = Deck::gain_db(1.0, 0.5, true);
        assert!((a - -3.01).abs() < 0.1, "deck A at center: {a} dB");
        assert!((b - -3.01).abs() < 0.1, "deck B at center: {b} dB");
    }

    #[test]
    fn crossfader_ends_are_full_and_silent() {
        let a_full = Deck::gain_db(1.0, 0.0, false);
        let b_silent = Deck::gain_db(1.0, 0.0, true);
        assert!(a_full.abs() < 0.01, "A full: {a_full}");
        assert!(b_silent <= -59.0, "B silent: {b_silent}");
    }
}
