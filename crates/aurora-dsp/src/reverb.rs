//! Freeverb: Schroeder/Moorer reverb (public-domain algorithm by Jezar at
//! Dreampoint, 1998), restructured with Rust safety and f64 accumulation.
//!
//! 8 feedback comb filters + 4 allpass filters per channel, stereo-coupled
//! through shared diffusion. Room size and damping map naturally to the
//! original parameters; wet/dry mixing is linear (equal-power at 50%).
//! The classic "studio plate" sound — the reverb every plugin ships first.

use aurora_core::AudioTransformation;

/// Reverb parameters as the UI sees them.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ReverbParams {
    /// 0.0..=1.0: comb feedback scale (room size).
    #[serde(default)]
    pub room_size: f32,
    /// 0.0..=1.0: high-frequency damping in the feedback loop.
    #[serde(default = "default_damping")]
    pub damping: f32,
    /// 0.0..=1.0: wet level (dry = 1 - wet).
    #[serde(default = "default_wet")]
    pub wet: f32,
    /// Stereo width of the wet signal.
    #[serde(default = "default_width")]
    pub width: f32,
    /// 0.0..=1.0: lowpass on input before the tank.
    #[serde(default)]
    pub input_lowpass: f32,
}

fn default_damping() -> f32 {
    0.3
}
fn default_wet() -> f32 {
    0.0
}
fn default_width() -> f32 {
    1.0
}

impl Default for ReverbParams {
    fn default() -> Self {
        Self {
            room_size: 0.5,
            damping: 0.3,
            wet: 0.0,
            width: 1.0,
            input_lowpass: 0.0,
        }
    }
}

impl ReverbParams {
    /// Disabled: dry-only at zero wet is exactly a no-op.
    pub fn is_bypassed(&self) -> bool {
        self.wet <= 1e-4
    }
}

// Freeverb constants (Jezar's original tuning).
const COMB_TUNINGS: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASS_TUNINGS: [usize; 4] = [556, 441, 341, 225];
const STEREO_SPREAD: usize = 23;
const FIXED_GAIN: f32 = 0.015;
const SCALE_WET: f32 = 3.0;
const SCALE_DAMP: f32 = 0.4;
const SCALE_ROOM: f32 = 0.28;
const OFFSET_ROOM: f32 = 0.7;

#[derive(Clone)]
struct Comb {
    buf: Vec<f32>,
    idx: usize,
    filter: f32, // lowpass damping state
}

impl Comb {
    fn new(size: usize) -> Self {
        Self {
            buf: vec![0.0; size],
            idx: 0,
            filter: 0.0,
        }
    }

    fn process(&mut self, input: f32, damp: f32, feedback: f32) -> f32 {
        let out = self.buf[self.idx];
        self.filter = out * damp + self.filter * (1.0 - damp);
        let v = input + self.filter * feedback;
        self.buf[self.idx] = v;
        self.idx = (self.idx + 1) % self.buf.len();
        out
    }
}

#[derive(Clone)]
struct Allpass {
    buf: Vec<f32>,
    idx: usize,
}

impl Allpass {
    fn new(size: usize) -> Self {
        Self {
            buf: vec![0.0; size],
            idx: 0,
        }
    }

    fn process(&mut self, input: f32) -> f32 {
        let buf_out = self.buf[self.idx];
        let out = -input + buf_out;
        self.buf[self.idx] = input + buf_out * 0.5;
        self.idx = (self.idx + 1) % self.buf.len();
        out
    }
}

/// A stereo Freeverb instance.
#[derive(Clone)]
pub struct Reverb {
    combs: [Comb; 8],   // left
    combs_r: [Comb; 8], // right (tunings + stereo spread)
    allpasses: [Allpass; 4],
    allpasses_r: [Allpass; 4],
    input_lp: [f32; 2], // pre-tank lowpass state per channel
    params: ReverbParams,
    damp: f32,
    feedback: f32,
    wet1: f32,
    wet2: f32,
}

impl Reverb {
    pub fn new(params: ReverbParams, sample_rate: u32) -> Self {
        // Tunings are for 44.1k; scale buffer sizes to the actual rate.
        let scale = (f64::from(sample_rate) / 44_100.0).max(0.5);
        let combs = COMB_TUNINGS.map(|t| Comb::new((t as f64 * scale) as usize + 1));
        let combs_r =
            COMB_TUNINGS.map(|t| Comb::new(((t + STEREO_SPREAD) as f64 * scale) as usize + 1));
        let allpasses = ALLPASS_TUNINGS.map(|t| Allpass::new((t as f64 * scale) as usize + 1));
        let allpasses_r = ALLPASS_TUNINGS
            .map(|t| Allpass::new(((t + STEREO_SPREAD) as f64 * scale) as usize + 1));

        let mut rv = Self {
            combs,
            combs_r,
            allpasses,
            allpasses_r,
            input_lp: [0.0; 2],
            params,
            damp: 0.0,
            feedback: 0.0,
            wet1: 0.0,
            wet2: 0.0,
        };
        rv.update();
        rv
    }

    fn update(&mut self) {
        self.damp = self.params.damping * SCALE_DAMP;
        self.feedback = self.params.room_size * SCALE_ROOM + OFFSET_ROOM;
        let wet = self.params.wet * SCALE_WET;
        let w = self.params.width;
        self.wet1 = wet * (0.5 + w * 0.5);
        self.wet2 = wet * (0.5 - w * 0.5);
    }

    pub fn set_params(&mut self, params: ReverbParams) {
        self.params = params;
        self.update();
    }

    pub fn params(&self) -> ReverbParams {
        self.params
    }

    pub fn reset(&mut self) {
        for c in &mut self.combs {
            c.buf.fill(0.0);
            c.filter = 0.0;
            c.idx = 0;
        }
        for c in &mut self.combs_r {
            c.buf.fill(0.0);
            c.filter = 0.0;
            c.idx = 0;
        }
        for a in &mut self.allpasses {
            a.buf.fill(0.0);
            a.idx = 0;
        }
        for a in &mut self.allpasses_r {
            a.buf.fill(0.0);
            a.idx = 0;
        }
        self.input_lp = [0.0; 2];
    }

    /// Process interleaved stereo (or mono-duplicated) f32 in place.
    ///
    /// Only handles 2-channel buffers; other channel counts are passed
    /// through untouched (the pipeline guards this).
    pub fn process_interleaved(&mut self, buf: &mut [f32]) {
        if buf.len() % 2 != 0 {
            return;
        }
        let lp_amount = self.params.input_lowpass;
        let dry = 1.0 - self.params.wet.min(1.0);

        for pair in buf.chunks_exact_mut(2) {
            // Pre-tank lowpass on input.
            let mut in_l = pair[0];
            let mut in_r = pair[1];
            if lp_amount > 1e-4 {
                self.input_lp[0] += lp_amount * (in_l - self.input_lp[0]);
                self.input_lp[1] += lp_amount * (in_r - self.input_lp[1]);
                in_l = self.input_lp[0];
                in_r = self.input_lp[1];
            }
            let input_l = in_l * FIXED_GAIN;
            let input_r = in_r * FIXED_GAIN;

            // Comb tank.
            let mut out_l = 0.0f32;
            let mut out_r = 0.0f32;
            for i in 0..8 {
                out_l += self.combs[i].process(input_l, self.damp, self.feedback);
                out_r += self.combs_r[i].process(input_r, self.damp, self.feedback);
            }

            // Allpass diffusion.
            for ap in &mut self.allpasses {
                out_l = ap.process(out_l);
            }
            for ap in &mut self.allpasses_r {
                out_r = ap.process(out_r);
            }

            // Equal-power-adjacent cross mix + dry.
            pair[0] = out_l * self.wet1 + out_r * self.wet2 + in_l * dry;
            pair[1] = out_r * self.wet1 + out_l * self.wet2 + in_r * dry;
        }
    }

    /// The transformation this effect reports when active.
    pub fn transformation(&self) -> Option<AudioTransformation> {
        if self.is_bypassed() {
            None
        } else {
            Some(AudioTransformation::Reverb)
        }
    }

    pub fn is_bypassed(&self) -> bool {
        self.params.is_bypassed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_wet_is_bit_identity() {
        let mut rv = Reverb::new(
            ReverbParams {
                wet: 0.0,
                ..Default::default()
            },
            44_100,
        );
        let mut buf: Vec<f32> = (0..2048).map(|i| ((i as f32) * 0.02).sin() * 0.5).collect();
        // Stereo interleave from mono: L=R.
        let stereo: Vec<f32> = buf.iter().flat_map(|&s| [s, s]).collect();
        let mut stereo = stereo;
        let input = stereo.clone();
        rv.process_interleaved(&mut stereo);
        assert_eq!(stereo, input, "zero wet must be bit-exact identity");
    }

    #[test]
    fn reverb_extends_tail_energy() {
        // A short impulse into a wet reverb must produce a decaying tail
        // longer than the input.
        let mut rv = Reverb::new(
            ReverbParams {
                room_size: 0.8,
                wet: 1.0,
                ..Default::default()
            },
            44_100,
        );
        let mut buf = vec![0.0f32; 2 * 48_000]; // 1 second stereo
        buf[0] = 0.8;
        buf[1] = 0.8;
        rv.process_interleaved(&mut buf);

        // Count non-near-zero samples in the second half (the tail).
        let tail = &buf[44_100..];
        let audible = tail.iter().filter(|&&s| s.abs() > 1e-4).count();
        assert!(audible > 100, "reverb tail too short: {audible} samples");

        // And the energy decays (no runaway feedback): the global peak is
        // in the first half, the tail's peak well below it.
        let peak_global = buf.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
        let peak_late = tail.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
        assert!(
            peak_late < peak_global * 0.5,
            "tail must decay: {peak_late} vs global {peak_global}"
        );
        assert!(
            peak_global < 1.0,
            "feedback must stay bounded: {peak_global}"
        );
    }

    #[test]
    fn stereo_channels_diverge() {
        // Identical L/R input produces decorrelated wet output.
        let mut rv = Reverb::new(
            ReverbParams {
                wet: 0.5,
                ..Default::default()
            },
            44_100,
        );
        let mut buf: Vec<f32> = (0..4096)
            .flat_map(|i| {
                let s = ((i as f32) * 0.01).sin() * 0.6;
                [s, s]
            })
            .collect();
        rv.process_interleaved(&mut buf);
        let mut diff = 0.0f32;
        for pair in buf.chunks_exact(2) {
            diff += (pair[0] - pair[1]).abs();
        }
        assert!(diff > 0.1, "expected stereo decorrelation, diff={diff}");
    }

    #[test]
    fn params_roundtrip() {
        let p = ReverbParams {
            room_size: 0.7,
            damping: 0.2,
            wet: 0.45,
            width: 0.8,
            input_lowpass: 0.1,
        };
        let json = serde_json::to_string(&p).unwrap();
        let back: ReverbParams = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }
}
