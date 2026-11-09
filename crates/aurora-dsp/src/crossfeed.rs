//! bs2b-style headphone crossfeed (2x2 cross matrix).
//!
//! Headphone drivers are hard-panned: bass in the left channel stays
//! entirely in the left ear, unlike speakers in a room where low
//! frequencies diffract around the head. Crossfeed bleeds an attenuated,
//! lowpassed copy of each channel into the opposite ear, mimicking
//! speaker listening.
//!
//! Standard bs2b constants: 700 Hz cutoff, -6.5 dB feed level. Per
//! sample: `L' = L + g * lp(R)`, `R' = R + g * lp(L)` where `lp` is a
//! one-pole lowpass at the cutoff. Only stereo streams are processed;
//! other channel counts pass through unchanged.
//!
//! Honesty contract (ADR 0002): disabled params are a provable no-op
//! (`is_bypassed`), and the active processor is reported as
//! `AudioTransformation::Crossfeed`.

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct CrossfeedParams {
    /// Master switch. `false` is a provable bypass.
    pub enabled: bool,
    /// Crossfeed filter cutoff in Hz (bs2b default: 700).
    pub cutoff_hz: f32,
    /// Feed level in dB (bs2b default: -6.5).
    pub feed_db: f32,
}

impl Default for CrossfeedParams {
    fn default() -> Self {
        Self {
            enabled: false,
            cutoff_hz: 700.0,
            feed_db: -6.5,
        }
    }
}

impl CrossfeedParams {
    pub fn is_bypassed(&self) -> bool {
        !self.enabled
    }

    fn normalized(self) -> Self {
        Self {
            enabled: self.enabled,
            cutoff_hz: finite_or(self.cutoff_hz, 700.0).clamp(20.0, 5000.0),
            feed_db: finite_or(self.feed_db, -6.5).clamp(-24.0, 0.0),
        }
    }
}

/// Stateful crossfeed processor (stereo only).
pub struct Crossfeed {
    /// One-pole lowpass coefficient: `y += a * (x - y)`.
    a: f32,
    /// Feed gain (linear).
    g: f32,
    /// Lowpassed copy of R feeding L's output.
    lp_r: f32,
    /// Lowpassed copy of L feeding R's output.
    lp_l: f32,
}

impl Crossfeed {
    pub fn new(params: CrossfeedParams, sample_rate: u32) -> Self {
        let p = params.normalized();
        let fs = f64::from(sample_rate).max(1.0);
        // Standard one-pole coefficient for -3 dB at the cutoff.
        let a = 1.0 - (-2.0 * std::f64::consts::PI * f64::from(p.cutoff_hz) / fs).exp();
        let g = 10f64.powf(f64::from(p.feed_db) / 20.0);
        Self {
            a: a as f32,
            g: g as f32,
            lp_r: 0.0,
            lp_l: 0.0,
        }
    }

    /// Process interleaved samples. Stereo only: any other channel
    /// count is an honest pass-through.
    pub fn process_interleaved(&mut self, samples: &mut [f32], channels: u16) {
        if channels != 2 {
            return;
        }
        let (a, g) = (self.a, self.g);
        let (mut lp_l, mut lp_r) = (self.lp_l, self.lp_r);
        for frame in samples.chunks_exact_mut(2) {
            let left = frame[0];
            let right = frame[1];
            lp_l += a * (left - lp_l);
            lp_r += a * (right - lp_r);
            frame[0] = left + g * lp_r;
            frame[1] = right + g * lp_l;
        }
        self.lp_l = lp_l;
        self.lp_r = lp_r;
    }
}

fn finite_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() { value } else { fallback }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stereo_pcm(n_frames: usize, l: f32, r: f32) -> Vec<f32> {
        (0..n_frames).flat_map(|_| [l, r]).collect()
    }

    #[test]
    fn disabled_params_deserialize_to_bypassed_defaults() {
        // Backward compat: a stored config from before crossfeed
        // existed deserializes with disabled bs2b defaults.
        let p: CrossfeedParams = serde_json::from_str("{}").expect("defaults");
        assert!(p.is_bypassed());
        assert_eq!(p.cutoff_hz, 700.0);
        assert_eq!(p.feed_db, -6.5);
    }

    #[test]
    fn dc_bleeds_more_than_high_frequency() {
        // The crossfeed matrix must differ between DC and HF: bass
        // bleeds across the head, treble stays panned. Feed a constant
        // (DC) into L only, then an alternating (quarter-Nyquist)
        // signal, and compare how much lands in R.
        let rate = 44_100u32;
        let p = CrossfeedParams {
            enabled: true,
            ..CrossfeedParams::default()
        };

        // DC: steady state after ~100ms of frames.
        let mut dc = Crossfeed::new(p, rate);
        let mut samples = stereo_pcm(4096, 1.0, 0.0);
        dc.process_interleaved(&mut samples, 2);
        let dc_bleed = samples[samples.len() - 1];

        // HF: alternating +/-1 alternating per sample, far above the
        // 700 Hz one-pole, so the bleed stays a fraction of the DC case.
        let mut hf = Crossfeed::new(p, rate);
        let mut samples = (0..4096)
            .flat_map(|i| {
                let s = if i % 2 == 0 { 1.0f32 } else { -1.0f32 };
                [s, 0.0f32]
            })
            .collect::<Vec<f32>>();
        hf.process_interleaved(&mut samples, 2);
        let hf_bleed = samples[samples.len() - 1];

        // DC bleed approaches the feed gain (~0.47 for -6.5 dB);
        // HF bleed is a small fraction of the DC bleed.
        assert!(
            dc_bleed > 0.4,
            "DC crossfeed too weak: {dc_bleed} (expected near the -6.5 dB feed level)"
        );
        assert!(
            hf_bleed.abs() < dc_bleed * 0.2,
            "HF must bleed far less than DC: hf {hf_bleed} vs dc {dc_bleed}"
        );
    }

    #[test]
    fn mono_channel_count_passes_through() {
        let p = CrossfeedParams {
            enabled: true,
            ..CrossfeedParams::default()
        };
        let mut cf = Crossfeed::new(p, 44_100);
        let mut samples = vec![0.5f32; 64];
        let before = samples.clone();
        cf.process_interleaved(&mut samples, 1);
        assert_eq!(samples, before, "mono must not be crossfed");
    }

    #[test]
    fn params_are_bounded_at_extremes() {
        let p = CrossfeedParams {
            enabled: true,
            cutoff_hz: f32::INFINITY,
            feed_db: f32::NAN,
        }
        .normalized();
        assert!(p.cutoff_hz.is_finite());
        assert!(p.feed_db.is_finite());
        let mut cf = Crossfeed::new(p, 48_000);
        let mut samples = stereo_pcm(1024, f32::MAX, f32::MIN);
        cf.process_interleaved(&mut samples, 2);
        assert!(samples.iter().all(|v| v.is_finite()));
    }
}
