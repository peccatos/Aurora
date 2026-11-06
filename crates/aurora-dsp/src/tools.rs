//! Portable playback tools adapted from Orchard Studio's effect-tool concepts.
//! The processor is intentionally small and playback-oriented: no DAW routing,
//! sequencer, sample assets, or host/plugin runtime is required.

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ToolsParams {
    /// Stereo balance: -1.0 is left, 0.0 is centered, 1.0 is right.
    pub balance: f32,
    /// Invert the left channel polarity.
    pub phase_invert_left: bool,
    /// Mid/side width multiplier. 1.0 is unchanged.
    pub stereo_width: f32,
    /// Soft-clip amount. 0.0 is bypassed.
    pub soft_clip: f32,
}

impl Default for ToolsParams {
    fn default() -> Self {
        Self {
            balance: 0.0,
            phase_invert_left: false,
            stereo_width: 1.0,
            soft_clip: 0.0,
        }
    }
}

impl ToolsParams {
    pub fn is_bypassed(&self) -> bool {
        self.balance.abs() < 1e-6
            && !self.phase_invert_left
            && (self.stereo_width - 1.0).abs() < 1e-6
            && self.soft_clip.abs() < 1e-6
    }

    pub fn normalized(self) -> Self {
        Self {
            balance: finite_or(self.balance, 0.0).clamp(-1.0, 1.0),
            phase_invert_left: self.phase_invert_left,
            stereo_width: finite_or(self.stereo_width, 1.0).clamp(0.0, 2.0),
            soft_clip: finite_or(self.soft_clip, 0.0).clamp(0.0, 1.0),
        }
    }
}

pub struct Tools {
    params: ToolsParams,
}

impl Tools {
    pub fn new(params: ToolsParams) -> Self {
        Self {
            params: params.normalized(),
        }
    }

    pub fn process_interleaved(&mut self, samples: &mut [f32], channels: u16) {
        if self.params.is_bypassed() {
            return;
        }
        let p = self.params;
        let ch = usize::from(channels);
        if ch == 0 {
            return;
        }
        for frame in samples.chunks_exact_mut(ch) {
            if ch >= 2 {
                let mut left = frame[0];
                let mut right = frame[1];
                let balance = p.balance;
                let left_gain = if balance > 0.0 { 1.0 - balance } else { 1.0 };
                let right_gain = if balance < 0.0 { 1.0 + balance } else { 1.0 };
                left *= left_gain;
                right *= right_gain;
                let mid = (left + right) * 0.5;
                let side = (left - right) * 0.5 * p.stereo_width;
                left = mid + side;
                right = mid - side;
                if p.phase_invert_left {
                    left = -left;
                }
                frame[0] = soft_clip(left, p.soft_clip);
                frame[1] = soft_clip(right, p.soft_clip);
                for sample in &mut frame[2..] {
                    *sample = soft_clip(*sample, p.soft_clip);
                }
            } else {
                if p.phase_invert_left {
                    frame[0] = -frame[0];
                }
                frame[0] = soft_clip(frame[0], p.soft_clip);
            }
        }
    }
}

fn finite_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() { value } else { fallback }
}

fn soft_clip(sample: f32, amount: f32) -> f32 {
    if amount <= 1e-6 {
        return sample;
    }
    if !sample.is_finite() {
        return sample.signum();
    }
    let drive = 1.0 + amount * 9.0;
    let driven = sample * drive;
    if !driven.is_finite() {
        return sample.signum();
    }
    driven / (1.0 + driven.abs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bypass_is_bit_identity() {
        let mut tools = Tools::new(ToolsParams::default());
        let mut samples = vec![0.2, -0.4, 0.6, -0.8];
        let before = samples.clone();
        tools.process_interleaved(&mut samples, 2);
        assert_eq!(samples, before);
    }

    #[test]
    fn phase_and_width_are_applied() {
        let mut tools = Tools::new(ToolsParams {
            phase_invert_left: true,
            stereo_width: 2.0,
            ..ToolsParams::default()
        });
        let mut samples = vec![1.0, 0.25];
        tools.process_interleaved(&mut samples, 2);
        assert!(samples[0] < 0.0);
        assert!(
            samples[1] < 0.0,
            "width turns the unequal pair's side negative"
        );
    }

    #[test]
    fn serde_defaults_missing_fields() {
        let params: ToolsParams = serde_json::from_str("{}").expect("defaults");
        assert_eq!(params, ToolsParams::default());
    }

    #[test]
    fn values_are_bounded_and_finite_at_extremes() {
        let mut tools = Tools::new(ToolsParams {
            balance: f32::INFINITY,
            stereo_width: f32::NEG_INFINITY,
            soft_clip: f32::INFINITY,
            phase_invert_left: false,
        });
        let mut samples = vec![f32::MAX, f32::MIN, 0.0, 1.0];
        tools.process_interleaved(&mut samples, 2);
        assert!(samples.iter().all(|value| value.is_finite()));
    }

    #[test]
    fn soft_clip_is_finite_and_bounded() {
        let mut tools = Tools::new(ToolsParams {
            soft_clip: 1.0,
            ..ToolsParams::default()
        });
        let mut samples = vec![100.0, -100.0];
        tools.process_interleaved(&mut samples, 2);
        assert!(samples.iter().all(|v| v.is_finite() && v.abs() < 1.0));
    }
}
