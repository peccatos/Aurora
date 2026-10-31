//! Typed PCM frame representation.
//!
//! The decoder emits [`PcmChunk`]s that preserve the source sample
//! representation (ADR 0002: no invisible int->float conversion) while
//! offering an explicit, visible conversion for the DSP path.

use aurora_core::{ChannelLayout, PcmFormat, SampleFormat};

/// Raw interleaved PCM data with its format.
///
/// Invariants (enforced by constructors):
/// - `data.len()` is a multiple of the frame size;
/// - the format is plausible (`PcmFormat::is_plausible`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PcmChunk {
    format: PcmFormat,
    data: Vec<u8>,
}

impl PcmChunk {
    /// Construct a chunk from raw interleaved bytes.
    ///
    /// Returns `None` if the data length is not a whole number of frames.
    /// `bits_per_sample` below 8 (no real-world format) is rounded up to
    /// byte granularity on the wire: callers pass byte-aligned buffers.
    pub fn from_bytes(format: PcmFormat, data: Vec<u8>) -> Option<Self> {
        let frame_bytes = frame_size_bytes(&format)?;
        if !data.len().is_multiple_of(frame_bytes) {
            return None;
        }
        if !format.is_plausible() {
            return None;
        }
        Some(Self { format, data })
    }

    pub fn format(&self) -> PcmFormat {
        self.format
    }

    /// Number of complete frames (one frame = one sample per channel).
    pub fn frames(&self) -> usize {
        frame_size_bytes(&self.format)
            .map(|b| self.data.len() / b)
            .unwrap_or(0)
    }

    /// Total number of individual samples across all channels.
    pub fn samples(&self) -> usize {
        self.frames() * usize::from(self.format.channels)
    }

    /// Raw interleaved bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    /// Convert to interleaved `f32` samples for the DSP path.
    ///
    /// This is the *explicit* format conversion: the caller takes
    /// responsibility for the transformation it applies (report it via
    /// `AudioTransformation::SampleFormatConversion`).
    pub fn to_f32_interleaved(&self) -> Vec<f32> {
        let channels = self.format.channels.max(1) as usize;
        let frames = self.frames();
        let mut out = Vec::with_capacity(frames * channels);
        let sample_bytes = (self.format.bits_per_sample as usize).div_ceil(8);
        let frame_bytes = sample_bytes * channels;

        for frame in 0..frames {
            let base = frame * frame_bytes;
            for ch in 0..channels {
                let off = base + ch * sample_bytes;
                out.push(read_sample_f32(
                    &self.data[off..off + sample_bytes],
                    self.format.sample_format,
                ));
            }
        }
        out
    }

    /// Silence of `frames` frames in the chunk's own format.
    pub fn silence(format: PcmFormat, frames: usize) -> Option<Self> {
        let frame_bytes = frame_size_bytes(&format)?;
        let mut data = vec![0u8; frames * frame_bytes];
        // Unsigned silence is the midpoint, not zero.
        if format.sample_format == SampleFormat::UnsignedInt {
            let sample_bytes = (format.bits_per_sample as usize).div_ceil(8);
            let mid = 0x80u8;
            for frame in 0..frames {
                let base = frame * frame_bytes;
                for ch in 0..format.channels as usize {
                    let off = base + ch * sample_bytes;
                    // 8-bit unsigned: silence = 128.
                    data[off] = mid;
                }
            }
        }
        Some(Self { format, data })
    }
}

/// Bytes occupied by one frame (all channels) for a format.
pub fn frame_size_bytes(format: &PcmFormat) -> Option<usize> {
    if format.channel_layout != ChannelLayout::Interleaved {
        // Interleaved only: the decoder normalizes planar input before
        // constructing chunks.
        return None;
    }
    if format.bits_per_sample == 0 || format.channels == 0 {
        return None;
    }
    let sample_bytes = usize::from(format.bits_per_sample).div_ceil(8);
    Some(sample_bytes * usize::from(format.channels))
}

/// Read a single little-endian sample as normalized f32.
fn read_sample_f32(bytes: &[u8], format: SampleFormat) -> f32 {
    let n = bytes.len();
    match format {
        SampleFormat::SignedInt => {
            let raw = i64::from_le_bytes(pad_le_signed(bytes)) as f64;
            // Scale by the actual bit width, not the byte width: 24-bit in
            // 3 bytes must map to [-1, 1) exactly as a native i24 would.
            let full = (1u64 << (n * 8 - 1)) as f64;
            (raw / full) as f32
        }
        SampleFormat::UnsignedInt => {
            let raw = u64::from_le_bytes(pad_le_unsigned(bytes)) as f64;
            let full = (1u64 << (n * 8)) as f64;
            ((raw / full) * 2.0 - 1.0) as f32
        }
        SampleFormat::Float => {
            if n >= 4 {
                f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
            } else if n == 2 {
                // Half-precision: approximate conversion.
                half_to_f32(u16::from_le_bytes([bytes[0], bytes[1]]))
            } else {
                0.0
            }
        }
    }
}

fn pad_le_signed(bytes: &[u8]) -> [u8; 8] {
    let mut out = [0u8; 8];
    out[..bytes.len()].copy_from_slice(bytes);
    if bytes.last().is_some_and(|b| b & 0x80 != 0) {
        // Sign-extend.
        for b in &mut out[bytes.len()..] {
            *b = 0xff;
        }
    }
    out
}

fn pad_le_unsigned(bytes: &[u8]) -> [u8; 8] {
    let mut out = [0u8; 8];
    out[..bytes.len()].copy_from_slice(bytes);
    out
}

fn half_to_f32(bits: u16) -> f32 {
    // Standard IEEE 754 half -> float.
    let sign = if bits & 0x8000 != 0 { -1.0f32 } else { 1.0 };
    let exp = ((bits >> 10) & 0x1f) as i32;
    let mant = (bits & 0x3ff) as f32;
    match exp {
        0 => sign * mant * 2f32.powi(-24),
        31 => {
            if mant == 0.0 {
                sign * f32::INFINITY
            } else {
                f32::NAN
            }
        }
        e => sign * (1.0 + mant / 1024.0) * 2f32.powi(e - 15),
    }
}
