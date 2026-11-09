//! Room correction: a user-loadable parametric correction curve, imported
//! from REW (Room EQ Wizard) "Export filter as text" files.
//!
//! Scope (v1, honest): Aurora does NOT measure the room. The user measures
//! elsewhere and loads the exported correction filters; Aurora applies them
//! as a cascade of biquads (same coefficient math as the parametric EQ,
//! Audio EQ Cookbook) and reports the transformation (ADR 0002).
//!
//! Validation is the contract: files violating the caps are rejected, not
//! clamped — a silently clamped correction is a lie about the room.

use aurora_core::AudioTransformation;
use serde::{Deserialize, Serialize};

use crate::biquad::{Biquad, BiquadKind, BiquadParams};

/// Hard caps on an accepted correction file.
pub const MAX_FILTERS: usize = 32;
pub const FC_MIN_HZ: f64 = 10.0;
pub const FC_MAX_HZ: f64 = 20_000.0;
pub const GAIN_MIN_DB: f64 = -20.0;
pub const GAIN_MAX_DB: f64 = 20.0;
pub const Q_MIN: f64 = 0.1;
pub const Q_MAX: f64 = 12.0;

/// Filter shape (REW export vocabulary).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CorrectionFilterKind {
    /// REW `PK`: peaking.
    Pk,
    /// REW `LS`: low shelf.
    Ls,
    /// REW `HS`: high shelf.
    Hs,
}

/// One correction filter exactly as REW exported it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CorrectionFilter {
    pub kind: CorrectionFilterKind,
    pub fc_hz: f64,
    pub gain_db: f64,
    pub q: f64,
}

/// User-facing room correction configuration (serializable, UI-driven).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RoomCorrectionParams {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub filters: Vec<CorrectionFilter>,
}

impl RoomCorrectionParams {
    /// True when this configuration provably passes audio untouched.
    pub fn is_bypassed(&self) -> bool {
        !self.enabled || self.filters.is_empty()
    }

    /// The transformation this stage reports when active.
    pub fn transformation(&self) -> Option<AudioTransformation> {
        if self.is_bypassed() {
            None
        } else {
            Some(AudioTransformation::RoomCorrection)
        }
    }
}

/// Parse error with a user-facing message (validation is the contract:
/// violations reject the file, never clamp).
#[derive(Debug, Clone, PartialEq)]
pub struct ParseError(pub String);

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ParseError {}

/// Parse REW "Export filter as text" content into correction filters.
///
/// Accepted line shape (whitespace-tolerant, mirroring REW's output):
/// `Filter 1: ON PK Fc 100 Hz Gain -3.5 dB Q 2.00`
/// `OFF` filters are skipped (REW's own convention: disabled bands, not
/// validated). `Preamp: ...` lines are skipped (REW's headroom trim, not a
/// room correction). Lines that do not start with `Filter` are ignored
/// (REW header text); a line that starts with `Filter` but fails to
/// tokenize rejects the whole file.
pub fn parse_rew_filters(text: &str) -> Result<Vec<CorrectionFilter>, ParseError> {
    let mut filters = Vec::new();
    for (line_no, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with("Preamp:") {
            continue;
        }
        let Some(rest) = line.strip_prefix("Filter") else {
            // Header/preamble text: not a filter line, ignore.
            continue;
        };
        let tokens: Vec<&str> = rest.split_whitespace().collect();
        // tokens: [<n>:, ON|OFF, PK|LS|HS, Fc, <num>, Hz, Gain, <num>, dB, (Q, <num>)]
        if tokens.len() < 9 {
            return Err(ParseError(format!(
                "line {}: not a REW filter export line",
                line_no + 1
            )));
        }
        let state = tokens[1];
        let kind = match tokens[2] {
            "PK" => CorrectionFilterKind::Pk,
            "LS" => CorrectionFilterKind::Ls,
            "HS" => CorrectionFilterKind::Hs,
            other => {
                return Err(ParseError(format!(
                    "line {}: unsupported filter type '{other}' (only PK/LS/HS)",
                    line_no + 1
                )));
            }
        };
        let mut fc = None;
        let mut gain = None;
        let mut q = None;
        let mut i = 3;
        while i + 1 < tokens.len() {
            match tokens[i] {
                "Fc" => {
                    fc = parse_number(tokens[i + 1]);
                    i += 2;
                }
                "Gain" => {
                    gain = parse_number(tokens[i + 1]);
                    i += 2;
                }
                "Q" => {
                    q = parse_number(tokens[i + 1]);
                    i += 2;
                }
                _ => i += 1,
            }
        }
        // OFF filters are skipped entirely (not validated — REW itself
        // may keep extreme disabled experiments in the file).
        if state != "ON" {
            continue;
        }
        let Some(fc) = fc else {
            return Err(ParseError(format!("line {}: missing Fc", line_no + 1)));
        };
        let Some(gain) = gain else {
            return Err(ParseError(format!("line {}: missing Gain", line_no + 1)));
        };
        let Some(q) = q else {
            return Err(ParseError(format!("line {}: missing Q", line_no + 1)));
        };
        if !(FC_MIN_HZ..=FC_MAX_HZ).contains(&fc) {
            return Err(ParseError(format!(
                "line {}: Fc {fc} Hz outside 10..20000 Hz",
                line_no + 1
            )));
        }
        if !(GAIN_MIN_DB..=GAIN_MAX_DB).contains(&gain) {
            return Err(ParseError(format!(
                "line {}: Gain {gain} dB outside -20..+20 dB",
                line_no + 1
            )));
        }
        if !(Q_MIN..=Q_MAX).contains(&q) {
            return Err(ParseError(format!(
                "line {}: Q {q} outside 0.1..12",
                line_no + 1
            )));
        }
        if filters.len() >= MAX_FILTERS {
            return Err(ParseError(format!(
                "more than {MAX_FILTERS} enabled filters"
            )));
        }
        filters.push(CorrectionFilter {
            kind,
            fc_hz: fc,
            gain_db: gain,
            q,
        });
    }
    if filters.is_empty() {
        return Err(ParseError(
            "no enabled filters found (all OFF or an empty file)".to_string(),
        ));
    }
    Ok(filters)
}

fn parse_number(token: &str) -> Option<f64> {
    token.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// The compiled, stateful correction chain (worker side, not serializable).
pub struct RoomCorrection {
    sections: Vec<Biquad>,
}

impl RoomCorrection {
    /// Compile for `sample_rate`/`channels`, reusing the EQ's biquad
    /// coefficient math. 0 dB filters are neutral and skipped; a filter
    /// above Nyquist for this rate is an honest miss (warned, skipped —
    /// same policy as an unknown convolution IR id), never a fake curve.
    pub fn compile(params: &RoomCorrectionParams, sample_rate: u32, channels: u16) -> Self {
        let nyquist = f64::from(sample_rate) / 2.0;
        let mut sections = Vec::new();
        for f in &params.filters {
            if f.gain_db.abs() < 1e-9 {
                continue;
            }
            if f.fc_hz >= nyquist {
                tracing::warn!(
                    fc_hz = f.fc_hz,
                    nyquist,
                    "room correction filter above Nyquist; skipped"
                );
                continue;
            }
            let bq = Biquad::design(
                BiquadParams {
                    kind: match f.kind {
                        CorrectionFilterKind::Pk => BiquadKind::PeakingEq,
                        CorrectionFilterKind::Ls => BiquadKind::LowShelf,
                        CorrectionFilterKind::Hs => BiquadKind::HighShelf,
                    },
                    freq_hz: f.fc_hz,
                    q: f.q,
                    gain_db: f.gain_db,
                },
                sample_rate,
                channels,
            );
            match bq {
                Ok(bq) => sections.push(bq),
                Err(e) => tracing::warn!(%e, "room correction filter rejected at compile; skipped"),
            }
        }
        Self { sections }
    }

    /// Process interleaved f32 samples in place; true if clipping
    /// protection engaged.
    pub fn process_interleaved(&mut self, buf: &mut [f32]) -> bool {
        let mut clipped = false;
        for bq in &mut self.sections {
            clipped |= bq.process_interleaved(buf);
        }
        clipped
    }

    pub fn is_empty(&self) -> bool {
        self.sections.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REW_FILE: &str = "\
Room EQ V5.20  Room 1 Correction.txt
Filter  1: ON  PK  Fc  25.2 Hz  Gain  0.5 dB  Q  2.00
Filter  2: ON  LS  Fc  100 Hz  Gain -3.5 dB  Q  0.70
Filter  3: OFF PK  Fc  3000 Hz  Gain  8.0 dB  Q  4.00
Filter  4: ON  HS  Fc  8000 Hz  Gain -2.0 dB  Q  0.50
Preamp: -3.0 dB
";

    #[test]
    fn parses_full_rew_file() {
        let filters = parse_rew_filters(REW_FILE).unwrap();
        assert_eq!(filters.len(), 3, "OFF filter must be skipped");
        assert_eq!(
            filters[0],
            CorrectionFilter {
                kind: CorrectionFilterKind::Pk,
                fc_hz: 25.2,
                gain_db: 0.5,
                q: 2.0
            }
        );
        assert_eq!(filters[1].kind, CorrectionFilterKind::Ls);
        assert_eq!(filters[1].gain_db, -3.5);
        assert_eq!(filters[2].kind, CorrectionFilterKind::Hs);
        assert_eq!(filters[2].fc_hz, 8000.0);
    }

    #[test]
    fn all_off_file_is_rejected() {
        let err = parse_rew_filters("Filter 1: OFF PK Fc 100 Hz Gain 1.0 dB Q 2.0\n").unwrap_err();
        assert!(err.0.contains("no enabled filters"));
    }

    #[test]
    fn malformed_line_is_rejected() {
        let err = parse_rew_filters("Filter 1: ON PK Fc Hz Gain 1.0 dB Q 2.0\n").unwrap_err();
        assert!(err.0.contains("missing Fc"), "{err}");
        let err = parse_rew_filters("Filter 1: ON BPZ Fc 100 Hz Gain 1.0 dB Q 2.0\n").unwrap_err();
        assert!(err.0.contains("unsupported filter type"));
    }

    #[test]
    fn bounds_violations_reject_not_clamp() {
        let cases = [
            (
                "Filter 1: ON PK Fc 5 Hz Gain 1.0 dB Q 2.0",
                "outside 10..20000",
            ),
            (
                "Filter 1: ON PK Fc 25000 Hz Gain 1.0 dB Q 2.0",
                "outside 10..20000",
            ),
            (
                "Filter 1: ON PK Fc 100 Hz Gain 25.0 dB Q 2.0",
                "outside -20..+20",
            ),
            (
                "Filter 1: ON PK Fc 100 Hz Gain -25.0 dB Q 2.0",
                "outside -20..+20",
            ),
            (
                "Filter 1: ON PK Fc 100 Hz Gain 1.0 dB Q 0.05",
                "outside 0.1..12",
            ),
            (
                "Filter 1: ON PK Fc 100 Hz Gain 1.0 dB Q 14.0",
                "outside 0.1..12",
            ),
        ];
        for (line, needle) in cases {
            let err = parse_rew_filters(line).unwrap_err();
            assert!(err.0.contains(needle), "line {line}: {err}");
        }
    }

    #[test]
    fn thirty_two_filter_cap_rejects_the_33rd() {
        let mut text = String::new();
        for i in 0..33 {
            text.push_str(&format!(
                "Filter {i}: ON PK Fc {fc} Hz Gain 1.0 dB Q 2.0\n",
                fc = 100 + i
            ));
        }
        let err = parse_rew_filters(&text).unwrap_err();
        assert!(err.0.contains("32"), "{err}");
        // Exactly 32 is fine.
        let ok = parse_rew_filters(&text.lines().take(32).collect::<Vec<_>>().join("\n"));
        assert_eq!(ok.unwrap().len(), 32);
    }

    #[test]
    fn disabled_params_are_bypassed() {
        let mut p = RoomCorrectionParams::default();
        assert!(p.is_bypassed());
        assert_eq!(p.transformation(), None);
        p.enabled = true;
        assert!(p.is_bypassed(), "enabled with no filters is still a bypass");
        assert_eq!(p.transformation(), None);
        p.filters = parse_rew_filters(REW_FILE).unwrap();
        assert!(!p.is_bypassed());
        assert_eq!(
            p.transformation(),
            Some(AudioTransformation::RoomCorrection)
        );
    }

    #[test]
    fn peaking_boost_at_center_beats_far_frequency() {
        // A +3 dB PK at 1 kHz must boost 1 kHz more than 5 kHz.
        let params = RoomCorrectionParams {
            enabled: true,
            filters: vec![CorrectionFilter {
                kind: CorrectionFilterKind::Pk,
                fc_hz: 1000.0,
                gain_db: 3.0,
                q: 2.0,
            }],
        };
        let mut rc = RoomCorrection::compile(&params, 44_100, 1);
        // Steady-state sine amplitude (enough cycles for the biquad to
        // settle; direct form 1 converges within a few cycles).
        fn amplitude(rc: &mut RoomCorrection, freq: f32) -> f32 {
            let n = 8192;
            let mut buf: Vec<f32> = (0..n)
                .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / 44_100.0).sin() * 0.1)
                .collect();
            rc.process_interleaved(&mut buf);
            let tail = &buf[n / 2..];
            tail.iter().fold(0.0f32, |m, s| m.max(s.abs()))
        }
        let at_1k = amplitude(&mut rc, 1000.0);
        let at_5k = amplitude(&mut rc, 5000.0);
        assert!(
            at_1k > at_5k * 1.3,
            "1 kHz ({at_1k}) must beat 5 kHz ({at_5k}) by more than 1.3x"
        );
        // ~+3 dB at center: 10^(3/20) * 0.1 = 0.141.
        assert!(
            (at_1k - 0.141).abs() < 0.02,
            "expected ~0.141 at 1 kHz, got {at_1k}"
        );
    }

    #[test]
    fn neutral_filters_compile_to_empty_chain() {
        let params = RoomCorrectionParams {
            enabled: true,
            filters: vec![CorrectionFilter {
                kind: CorrectionFilterKind::Pk,
                fc_hz: 1000.0,
                gain_db: 0.0,
                q: 1.0,
            }],
        };
        let rc = RoomCorrection::compile(&params, 44_100, 2);
        assert!(rc.is_empty());
    }
}
