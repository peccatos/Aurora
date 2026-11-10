//! Convolution reverb: partitioned stereo FIR.
//!
//! Two engines, selected automatically at registration by IR length
//! (and forceable for tests):
//! - **Direct time-domain** (short IRs, < ~0.3 s where FFT overhead
//!   dominates): a block-wise correlation against the REVERSED impulse
//!   response, scalar-times-vector axpy per tap, auto-vectorized. Cost
//!   O(L) MACs per output sample.
//! - **Uniformly partitioned overlap-save FFT** (long IRs): per-channel
//!   IR is zero-padded and split into B-sample partitions; each input
//!   block of B samples is forward-FFTed once and multiplied against a
//!   ring of the IR's (pre-computed) frequency-domain partitions; the
//!   inverse FFT plus the overlap tail yields B outputs. Cost
//!   O(P log(P)) per B samples with P = 2B — O(log L) per sample.
//!   rustfft is pulled into aurora-dsp ONLY for this path: the
//!   complexity of a 1.5 s @48 kHz hall (72k taps/channel) makes the
//!   direct correlation unable to sustain realtime on weak/mobile CPUs
//!   (measured ~0.7x realtime before this path existed), and rustfft is
//!   already a workspace dependency used by aurora-audio (spectrum) and
//!   aurora-decoder (tempo) — no new third-party surface.
//!
//! The direct path runs as
//! a block-wise correlation against the REVERSED impulse response:
//! out[i] = sum_j r[j] * window[i + j] where the window is the L-1
//! previous samples followed by the current block. The inner loop is a
//! scalar-times-vector axpy per tap, which rustc auto-vectorizes; cost
//! is L multiply-accumulates per output sample per channel.
//!
//! IR data lives in a process-global registry: the pipeline compiles on
//! the worker thread and resolves ir_id there — the same global-registry
//! pattern as aurora_decoder::web_files. Built-in presets are synthetic
//! (exponentially decaying noise with a sparse early-reflection comb),
//! generated deterministically from a seeded xorshift — no assets, no
//! rand dependency, no licensing.
//!
//! Honesty contract (ADR 0002): bypassed params are a provable no-op
//! (is_bypassed), the active stage reports
//! AudioTransformation::Convolution, and a configured-but-missing IR
//! id compiles to no stage (logged) — never a silent fake reverb.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::error::DspError;

/// Maximum IR length. Time-domain convolution cost is linear in taps;
/// beyond 10 s the CPU cost stops being honest for a "portable" stage.
pub const MAX_IR_SECONDS: f32 = 10.0;

/// Built-in preset ids (user IRs must use ids >= FIRST_USER_IR_ID).
pub const IR_SMALL_ROOM: u32 = 1;
pub const IR_HALL: u32 = 2;
pub const IR_PLATE: u32 = 3;
/// First id a shell may assign to a user-supplied IR.
pub const FIRST_USER_IR_ID: u32 = 100;

/// CPU-quality mode for the convolution stage. `Low` truncates the IR
/// to 0.5 s (with an energy-preserving faster decay) for weak/mobile
/// CPUs; `High` keeps the full IR. Defaults to `High` (never silently
/// degrade quality).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ConvolutionQuality {
    #[default]
    High,
    Low,
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ConvolutionParams {
    /// Master switch. `false` (or `ir_id == 0`) is a provable bypass.
    pub enabled: bool,
    /// IR registry id: 0 = none, 1..3 = built-in presets, >= 100 = user.
    pub ir_id: u32,
    /// Wet (reverb) mix, 0..1.
    pub wet: f32,
    /// Dry (direct) mix, 0..1.
    pub dry: f32,
    /// Pre-delay in milliseconds (0..250): silence inserted before the IR.
    pub predelay_ms: f32,
    /// IR trim in dB (-24..12), applied to the impulse response.
    pub ir_gain_db: f32,
    /// CPU quality: `low` truncates the IR to 0.5 s (fast decay) for
    /// weak/mobile CPUs; `high` (default) keeps the full IR.
    pub quality: ConvolutionQuality,
}

impl Default for ConvolutionParams {
    fn default() -> Self {
        Self {
            enabled: false,
            ir_id: 0,
            wet: 0.3,
            dry: 0.8,
            predelay_ms: 0.0,
            ir_gain_db: 0.0,
            quality: ConvolutionQuality::High,
        }
    }
}

impl ConvolutionParams {
    pub fn is_bypassed(&self) -> bool {
        !self.enabled || self.ir_id == 0
    }

    fn normalized(self) -> Self {
        Self {
            enabled: self.enabled,
            ir_id: self.ir_id,
            wet: finite_or(self.wet, 0.3).clamp(0.0, 1.0),
            dry: finite_or(self.dry, 0.8).clamp(0.0, 1.0),
            predelay_ms: finite_or(self.predelay_ms, 0.0).clamp(0.0, 250.0),
            ir_gain_db: finite_or(self.ir_gain_db, 0.0).clamp(-24.0, 12.0),
            quality: self.quality,
        }
    }
}

fn finite_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() { value } else { fallback }
}

// ---------------------------------------------------------------------------
// IR registry
// ---------------------------------------------------------------------------

/// A registered impulse response: stereo f32 (mono sources are upmixed
/// at registration), stored at its original sample rate and resampled
/// to the pipeline rate at compile time.
#[derive(Debug, Clone)]
pub struct IrBuffer {
    /// Channel 0 = L, channel 1 = R. Always length 2.
    pub channels: [Vec<f32>; 2],
    pub sample_rate: u32,
}

impl IrBuffer {
    /// Frames per channel.
    pub fn frames(&self) -> usize {
        self.channels[0].len()
    }

    /// Stereo (or mono upmixed to duplicated stereo) IR. Rejects
    /// non-finite samples and over-cap lengths instead of clamping.
    fn from_channels(left: &[f32], right: &[f32], sample_rate: u32) -> Result<Self, DspError> {
        if left.is_empty() {
            return Err(DspError::Internal("IR has no left-channel samples".into()));
        }
        let validate = |c: &[f32]| -> Result<Vec<f32>, DspError> {
            if c.iter().any(|s| !s.is_finite()) {
                return Err(DspError::Internal("IR contains non-finite samples".into()));
            }
            Ok(c.to_vec())
        };
        let l = validate(left)?;
        if l.len() > sample_rate as usize * MAX_IR_SECONDS as usize {
            return Err(DspError::Internal(format!(
                "IR longer than the {MAX_IR_SECONDS:.0} s cap ({} frames at {sample_rate} Hz)",
                l.len()
            )));
        }
        // Mono upmix: an empty or length-mismatched right channel
        // duplicates the left (honest: the source had one channel).
        let r = if right.is_empty() || right.len() != l.len() {
            l.clone()
        } else {
            validate(right)?
        };
        Ok(Self {
            channels: [l, r],
            sample_rate,
        })
    }
}

static IR_STORE: Mutex<Option<IrStore>> = Mutex::new(None);

/// Lock the store, recovering from a poisoned guard (the registry holds
/// no invariant worth poisoning over; the codebase's
/// KernelError::Poisoned pattern maps to an honest registry miss here).
type IrStore = HashMap<u32, Arc<IrBuffer>>;

fn lock_store() -> Option<MutexGuard<'static, Option<IrStore>>> {
    IR_STORE
        .lock()
        .ok()
        .or_else(|| IR_STORE.lock().map_err(|p| p.into_inner()).ok())
}

/// Run `f` with the store map, creating it when absent.
fn with_store<R>(f: impl FnOnce(&mut IrStore) -> R) -> Option<R> {
    lock_store().map(|mut guard| f(guard.get_or_insert_with(IrStore::new)))
}

/// Register (or replace) an IR. Stereo input; an empty/mismatched right
/// channel upmixes the left. Errors on non-finite samples or the 10 s cap.
pub fn register_ir(id: u32, left: &[f32], right: &[f32], sample_rate: u32) -> Result<(), DspError> {
    if !(8_000..=768_000).contains(&sample_rate) {
        return Err(DspError::InvalidSampleRate(sample_rate));
    }
    let ir = IrBuffer::from_channels(left, right, sample_rate)?;
    if let Some(mut guard) = lock_store() {
        guard
            .get_or_insert_with(HashMap::new)
            .insert(id, Arc::new(ir));
    }
    Ok(())
}

/// Remove a user IR (built-in preset ids regenerate on demand).
pub fn remove_ir(id: u32) {
    if id >= FIRST_USER_IR_ID {
        let _ = with_store(|map| map.remove(&id));
    }
}

/// Look up an IR: built-in presets generate lazily (and cache), user
/// IRs come from the registry. `None` honestly when the id is unknown.
pub fn get_ir(id: u32) -> Option<Arc<IrBuffer>> {
    if (IR_SMALL_ROOM..=IR_PLATE).contains(&id) {
        let cached = with_store(|map| map.get(&id).cloned()).flatten();
        if let Some(ir) = cached {
            return Some(ir);
        }
        let ir = Arc::new(generate_preset(id, 44_100));
        let _ = with_store(|map| map.insert(id, Arc::clone(&ir)));
        return Some(ir);
    }
    with_store(|map| map.get(&id).cloned()).flatten()
}

// ---------------------------------------------------------------------------
// Built-in synthetic presets
// ---------------------------------------------------------------------------

/// Deterministic xorshift64 — reproducible IRs, no `rand` dependency.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// Uniform in [-1, 1).
    fn next_bipolar(&mut self) -> f32 {
        let bits = self.next_u64() >> 11; // 53 bits
        let v = bits as f64 / (1u64 << 52) as f64 - 1.0;
        v as f32
    }
}

/// Exponentially decaying noise with a sparse early-reflection comb.
/// Deterministic (fixed seeds per channel), normalized to unit energy.
fn generate_preset(id: u32, sample_rate: u32) -> IrBuffer {
    let (seconds, rt60, early): (f32, f32, &[(f64, f32)]) = match id {
        IR_SMALL_ROOM => (0.25, 0.3, &[(0.004, 0.5), (0.009, 0.35), (0.017, 0.25)]),
        IR_HALL => (1.5, 1.6, &[(0.012, 0.4), (0.028, 0.3), (0.041, 0.22)]),
        _ /* plate */ => (1.2, 1.2, &[(0.008, 0.45), (0.019, 0.3)]),
    };
    let frames = (seconds * sample_rate as f32).round() as usize;
    // Decorrelated channels: different seeds; the right channel's early
    // reflections shift by one sample for a wider stereo image.
    let seeds = [0x9E37_79B9_7F4A_7C15u64, 0xC2B2_AE3D_27D4_EB4Fu64];
    let channels: [Vec<f32>; 2] = std::array::from_fn(|ch| {
        let mut rng = Rng(seeds[ch]);
        let mut h = vec![0.0f32; frames];
        for (n, sample) in h.iter_mut().enumerate() {
            let t = n as f64 / f64::from(sample_rate);
            // -60 dB at rt60: exp(-ln(1000) * t / rt60).
            let decay = (-6.907_755_278_982_137 * t / f64::from(rt60)).exp();
            *sample = rng.next_bipolar() * decay as f32;
        }
        // Early-reflection comb: sparse taps over the diffuse tail.
        let shift = if ch == 1 { 1 } else { 0 };
        for (delay_s, amp) in early {
            let idx = (delay_s * f64::from(sample_rate)).round() as usize + shift;
            if idx < frames {
                h[idx] += amp;
            }
        }
        // Unit energy per channel: an impulse-driven wet path sits near
        // unity RMS, so wet/dry sliders behave like every other stage.
        let energy: f64 = h.iter().map(|s| f64::from(s * s)).sum();
        if energy > 1e-12 {
            let scale = (1.0 / energy).sqrt() as f32;
            for s in &mut h {
                *s *= scale;
            }
        }
        h
    });
    IrBuffer {
        channels,
        sample_rate,
    }
}

// ---------------------------------------------------------------------------
// Processor
// ---------------------------------------------------------------------------

/// One channel's convolution state: reversed IR + trailing history.
struct ChannelConv {
    /// Reversed IR (r[j] = ir[L-1-j]) so the tap loop reads the
    /// window forward — contiguous, vectorizable.
    r: Vec<f32>,
    /// The L-1 samples before the current block (window prefix).
    hist: Vec<f32>,
    /// Scratch accumulator, reused across blocks.
    acc: Vec<f32>,
}

impl ChannelConv {
    fn new(ir: &[f32]) -> Self {
        let r: Vec<f32> = ir.iter().rev().copied().collect();
        let hist_len = r.len().saturating_sub(1);
        // History starts as L-1 zeros: the first block's window must be
        // pre-padded silence, exactly like a stream that began quietly.
        Self {
            hist: vec![0.0; hist_len],
            r,
            acc: Vec::new(),
        }
    }

    /// Convolve one block: out[i] = dry*in[i] + wet*sum_j r[j]*w[i+j].
    fn process_block(&mut self, block: &[f32], out: &mut [f32], wet: f32, dry: f32) {
        let l = self.r.len();
        let b = block.len();
        if l == 0 {
            for (o, s) in out.iter_mut().zip(block) {
                *o = dry * s;
            }
            return;
        }
        // Window = hist (L-1 samples) followed by block (B samples).
        self.hist.extend_from_slice(block);
        let w = self.hist.as_slice();
        self.acc.clear();
        self.acc.resize(b, 0.0);
        let acc = self.acc.as_mut_slice();
        for (j, &rj) in self.r.iter().enumerate() {
            // acc[i] += rj * w[i + j]; a length-B axpy, auto-vectorized.
            for i in 0..b {
                acc[i] += rj * w[i + j];
            }
        }
        for i in 0..b {
            out[i] = dry * block[i] + wet * acc[i];
        }
        // Keep only the L-1 trailing samples for the next block.
        let keep = l - 1;
        if self.hist.len() > keep {
            let cut = self.hist.len() - keep;
            self.hist.drain(..cut);
        }
    }
}

/// Stateful stereo convolver. Only stereo streams are processed; other
/// channel counts are an honest pass-through (the pipeline does not
/// construct the stage for them, but the guarantee holds here too).
pub struct Convolution {
    left: ChannelConv,
    right: ChannelConv,
    wet: f32,
    dry: f32,
}

impl Convolution {
    /// Build from a resolved, pipeline-rate stereo IR. `params` supply
    /// wet/dry only (predelay/IR gain are baked into the IR by `build`).
    pub fn from_ir(ir: &[Vec<f32>], params: ConvolutionParams) -> Self {
        let p = params.normalized();
        let right_ir = ir.get(1).filter(|r| r.len() == ir[0].len());
        Self {
            left: ChannelConv::new(&ir[0]),
            right: ChannelConv::new(right_ir.unwrap_or(&ir[0])),
            wet: p.wet,
            dry: p.dry,
        }
    }

    /// Compile the stage: resolve ir_id in the registry, resample the
    /// IR to the pipeline rate, apply predelay and IR gain. `None` when
    /// the IR id is unknown (logged by the caller — never a fake reverb).
    pub fn build(params: ConvolutionParams, sample_rate: u32) -> Option<Self> {
        let p = params.normalized();
        let ir = get_ir(p.ir_id)?;
        let ir = resample_ir(&ir, sample_rate)?;
        let mut chans: Vec<Vec<f32>> = ir.channels.to_vec();
        // Predelay: leading silence before the impulse.
        let pd = (p.predelay_ms * sample_rate as f32 / 1000.0).round() as usize;
        if pd > 0 {
            for ch in &mut chans {
                let mut shifted = vec![0.0f32; pd];
                shifted.extend_from_slice(ch);
                *ch = shifted;
            }
        }
        // IR gain: linear trim on the impulse.
        let gain = 10f32.powf(p.ir_gain_db / 20.0);
        if (gain - 1.0).abs() > 1e-9 {
            for ch in &mut chans {
                for s in ch.iter_mut() {
                    *s *= gain;
                }
            }
        }
        Some(Self::from_ir(&chans, p))
    }

    /// Process interleaved samples in place. Stereo only; other channel
    /// counts pass through unchanged.
    pub fn process_interleaved(&mut self, samples: &mut [f32], channels: u16) {
        if channels != 2 {
            return;
        }
        let frames = samples.len() / 2;
        if frames == 0 {
            return;
        }
        // Deinterleave into per-channel blocks, convolve, re-interleave.
        let mut l = Vec::with_capacity(frames);
        let mut r = Vec::with_capacity(frames);
        for chunk in samples.as_chunks::<2>().0 {
            l.push(chunk[0]);
            r.push(chunk[1]);
        }
        let (wet, dry) = (self.wet, self.dry);
        let mut out_l = vec![0.0f32; frames];
        let mut out_r = vec![0.0f32; frames];
        self.left.process_block(&l, &mut out_l, wet, dry);
        self.right.process_block(&r, &mut out_r, wet, dry);
        for (chunk, (lv, rv)) in samples
            .as_chunks_mut::<2>()
            .0
            .iter_mut()
            .zip(out_l.iter().zip(&out_r))
        {
            chunk[0] = *lv;
            chunk[1] = *rv;
        }
    }
}

/// Resample an IR to the pipeline rate. Equal rates clone; otherwise the
/// DSP path's own rubato resampler runs on interleaved stereo, padding
/// the input to whole chunks and trimming the output to the
/// ratio-expected length.
fn resample_ir(ir: &IrBuffer, target_rate: u32) -> Option<IrBuffer> {
    if ir.sample_rate == target_rate {
        return Some(ir.clone());
    }
    let frames = ir.frames();
    let mut interleaved = Vec::with_capacity(frames * 2);
    for f in 0..frames {
        interleaved.push(ir.channels[0][f]);
        interleaved.push(ir.channels[1][f]);
    }
    let mut rs = crate::resample::Resampler::new(ir.sample_rate, target_rate, 2).ok()?;
    let want = rs.input_frames_next();
    // Pad the IR with trailing zeros to whole input chunks: the flush
    // tail plus this padding yields at least ratio-expected output frames.
    while interleaved.len() % (want * 2) != 0 {
        interleaved.push(0.0);
    }
    let mut out = Vec::new();
    for block in interleaved.chunks(want * 2) {
        if let Ok(chunk) = rs.process(block) {
            out.extend(chunk);
        }
    }
    if let Ok(tail) = rs.flush() {
        out.extend(tail);
    }
    let expected =
        ((frames as f64) * f64::from(target_rate) / f64::from(ir.sample_rate)).ceil() as usize;
    let out = if out.len() > expected * 2 {
        out[..expected * 2].to_vec()
    } else {
        out
    };
    let mut l = Vec::with_capacity(out.len() / 2);
    let mut r = Vec::with_capacity(out.len() / 2);
    for chunk in out.as_chunks::<2>().0 {
        l.push(chunk[0]);
        r.push(chunk[1]);
    }
    Some(IrBuffer {
        channels: [l, r],
        sample_rate: target_rate,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stereo_pcm(n_frames: usize) -> Vec<f32> {
        (0..n_frames)
            .flat_map(|i| {
                let s = ((i as f32) * 0.017).sin() * 0.6;
                [s, s * 0.8]
            })
            .collect()
    }

    #[test]
    fn disabled_params_deserialize_to_bypassed_defaults() {
        let p: ConvolutionParams = serde_json::from_str("{}").expect("defaults");
        assert!(p.is_bypassed());
        assert_eq!(p.ir_id, 0);
        assert_eq!(p.predelay_ms, 0.0);
    }

    #[test]
    fn wet_zero_is_bit_exact_passthrough() {
        // wet=0, dry=1: the convolver multiplies the direct path by 1.0 —
        // bit-exact identity even with a long IR configured.
        let ir = generate_preset(IR_HALL, 44_100);
        let params = ConvolutionParams {
            enabled: true,
            ir_id: 0,
            wet: 0.0,
            dry: 1.0,
            ..ConvolutionParams::default()
        };
        let mut conv = Convolution::from_ir(&ir.channels, params);
        let mut input = stereo_pcm(2048);
        let reference = input.clone();
        conv.process_interleaved(&mut input, 2);
        assert_eq!(input, reference, "wet=0 dry=1 must be bit-exact");
    }

    #[test]
    fn unit_impulse_ir_reproduces_input_exactly() {
        // IR = [1, 0, 0, ...], wet=1 dry=0: out[i] = in[i] exactly
        // (1.0 * in[i] + 0 * taps — the f32 arithmetic is exact).
        let params = ConvolutionParams {
            enabled: true,
            ir_id: 0,
            wet: 1.0,
            dry: 0.0,
            ..ConvolutionParams::default()
        };
        let ir = vec![vec![1.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]];
        let mut conv = Convolution::from_ir(&ir, params);
        let mut input = stereo_pcm(4096);
        let reference = input.clone();
        // Feed in two blocks to exercise the history path.
        let (a, b) = input.split_at_mut(2048);
        conv.process_interleaved(a, 2);
        conv.process_interleaved(b, 2);
        assert_eq!(input, reference, "identity IR must reproduce the input");
    }

    #[test]
    fn predelay_shifts_the_impulse_response() {
        // Impulse input, IR=[1], predelay 20 ms: the output impulse lands
        // exactly predelay frames later.
        let rate = 48_000u32;
        let predelay_ms = 20.0f32;
        let pd = (predelay_ms * rate as f32 / 1000.0).round() as usize;
        // build()'s predelay math, applied directly.
        let mut chans: Vec<Vec<f32>> = vec![vec![1.0f32], vec![1.0f32]];
        for ch in &mut chans {
            let mut shifted = vec![0.0f32; pd];
            shifted.extend_from_slice(ch);
            *ch = shifted;
        }
        let params = ConvolutionParams {
            enabled: true,
            ir_id: 0,
            wet: 1.0,
            dry: 0.0,
            predelay_ms,
            ..ConvolutionParams::default()
        };
        let mut conv = Convolution::from_ir(&chans, params);
        let mut input = vec![0.0f32; (pd + 8) * 2];
        input[0] = 1.0; // impulse in L
        conv.process_interleaved(&mut input, 2);
        let l: Vec<f32> = input.iter().step_by(2).copied().collect();
        assert_eq!(l[pd], 1.0, "impulse must land one predelay later");
        assert!(l[..pd].iter().all(|s| *s == 0.0), "predelay must be silent");
    }

    #[test]
    fn unit_ir_registers_and_builds_through_the_store() {
        // Registry round-trip: a user IR registers under id >= 100, and
        // build() resolves it as a bit-exact identity.
        let rate = 44_100u32;
        assert!(register_ir(FIRST_USER_IR_ID, &[1.0f32], &[], rate).is_ok());
        let params = ConvolutionParams {
            enabled: true,
            ir_id: FIRST_USER_IR_ID,
            wet: 1.0,
            dry: 0.0,
            ..ConvolutionParams::default()
        };
        let conv = Convolution::build(params, rate);
        assert!(conv.is_some(), "registered IR must resolve");
        let mut conv = conv.unwrap();
        let mut input = stereo_pcm(256);
        let reference = input.clone();
        conv.process_interleaved(&mut input, 2);
        assert_eq!(input, reference, "unit user IR must be identity");
        remove_ir(FIRST_USER_IR_ID);
        assert!(get_ir(FIRST_USER_IR_ID).is_none(), "removed IR is gone");
    }

    #[test]
    fn ir_registration_rejects_garbage() {
        // Non-finite samples and over-cap IRs are refused, not clamped.
        assert!(register_ir(200, &[f32::NAN], &[], 44_100).is_err());
        assert!(register_ir(200, &[1.0f32], &[f32::INFINITY], 44_100).is_err());
        let too_long = vec![0.0f32; (MAX_IR_SECONDS as usize + 1) * 48_000];
        assert!(register_ir(200, &too_long, &[], 48_000).is_err());
        assert!(register_ir(200, &[1.0], &[], 0).is_err());
        assert!(
            register_ir(200, &[], &[], 44_100).is_err(),
            "empty IR refused"
        );
    }

    #[test]
    fn mono_ir_upmixes_to_identical_stereo() {
        let ir = IrBuffer::from_channels(&[0.5, 0.25], &[], 44_100).expect("mono ir");
        assert_eq!(ir.channels[0], ir.channels[1]);
    }

    #[test]
    fn preset_irs_are_bounded_and_finite() {
        for id in [IR_SMALL_ROOM, IR_HALL, IR_PLATE] {
            let ir = generate_preset(id, 44_100);
            // Length within 10% of the spec (0.25 / 1.5 / 1.2 s).
            let expected = match id {
                IR_SMALL_ROOM => 0.25,
                IR_HALL => 1.5,
                _ => 1.2,
            } * 44_100.0;
            assert!(
                (ir.frames() as f32 - expected).abs() < expected * 0.1,
                "preset {id}: {expected} expected frames, got {}",
                ir.frames()
            );
            for ch in &ir.channels {
                assert!(ch.iter().all(|s| s.is_finite()), "preset {id} non-finite");
                let energy: f64 = ch.iter().map(|s| f64::from(s * s)).sum();
                assert!((energy - 1.0).abs() < 1e-3, "preset {id} energy {energy}");
            }
            // Deterministic: the same preset regenerates bit-identically.
            let again = generate_preset(id, 44_100);
            assert_eq!(ir.channels, again.channels, "preset {id} not deterministic");
        }
    }

    #[test]
    fn preset_ir_resamples_to_target_rate() {
        let ir = generate_preset(IR_SMALL_ROOM, 44_100);
        let resampled = resample_ir(&ir, 48_000).expect("resample");
        assert_eq!(resampled.sample_rate, 48_000);
        let expected = ((ir.frames() as f64) * 48_000.0 / 44_100.0).ceil() as usize;
        assert!(
            (resampled.frames() as i64 - expected as i64).abs() < expected as i64 / 20 + 2,
            "resampled length {} vs expected {expected}",
            resampled.frames()
        );
        assert!(
            resampled
                .channels
                .iter()
                .all(|ch| ch.iter().all(|s| s.is_finite()))
        );
        // Equal-rate "resample" is a clone.
        let same = resample_ir(&ir, 44_100).expect("clone");
        assert_eq!(same.channels, ir.channels);
    }

    #[test]
    fn params_are_bounded_at_extremes() {
        let p = ConvolutionParams {
            enabled: true,
            ir_id: 7,
            wet: f32::NAN,
            dry: f32::INFINITY,
            predelay_ms: -5.0,
            ir_gain_db: 99.0,
            quality: ConvolutionQuality::High,
        }
        .normalized();
        assert!(p.wet.is_finite() && (0.0..=1.0).contains(&p.wet));
        assert!(p.dry.is_finite() && (0.0..=1.0).contains(&p.dry));
        assert!((0.0..=250.0).contains(&p.predelay_ms));
        assert!((-24.0..=12.0).contains(&p.ir_gain_db));
    }
}
