//! WASAPI exclusive-mode output backend (ADR 0001: the Aurora-owned
//! `AudioOutputBackend` boundary, CPAL is one implementation — this is the
//! other one, Windows-specific).
//!
//! Exclusive mode bypasses the Windows audio engine (mixer, format
//! conversion, resampling) entirely: the stream goes to the hardware in
//! the source's exact format. This is the only Windows path where
//! bit-perfect is *provable* rather than assumed.
//!
//! Stream lifecycle: initialize COM per thread, activate the endpoint,
//! negotiate the exact format (IsFormatSupported with EXCLUSIVE share
//! mode), event-driven buffer refill on a dedicated thread.
//!
//! Phase 5 scope: this module provides exclusive-mode **capability
//! probing** (which formats the device accepts in exclusive mode) and the
//! integrity classification upgrade. Full playback-stream integration is
//! gated behind device support and lands with the soak tests.

use aurora_core::{PcmFormat, SampleFormat};

use crate::error::EngineError;

/// Whether the device at `device_key` accepts the exact source format in
/// WASAPI exclusive mode.
///
/// Exclusive mode requires: the *periodicity* matches the device's
/// supported range and the format is bit-exact — Windows does no
/// conversion for you. When this returns true, playing through the
/// exclusive path preserves every sample bit.
pub fn probe_exclusive_support(device_key: &str, source: PcmFormat) -> Result<bool, EngineError> {
    let device = crate::device::find_by_key(device_key)?;

    // COM apartment for this thread.
    com_init()?;
    let result = unsafe { probe_device(&device, source) };
    com_uninit();
    result
}

/// The highest achievable integrity for a format on a device.
///
/// ADR 0002 in one function: if the device takes the format natively in
/// exclusive mode, the full software chain can be bit-exact up to the
/// DAC; verification (loopback capture) is still a separate measurement.
pub fn exclusive_integrity(
    device_key: &str,
    source: PcmFormat,
    transformations: &[aurora_core::AudioTransformation],
) -> Result<aurora_core::OutputIntegrity, EngineError> {
    if !transformations.is_empty() {
        // Any active transformation makes the path provably non-identical.
        return Ok(aurora_core::OutputIntegrity::Transformed);
    }
    if probe_exclusive_support(device_key, source)? {
        // Exclusive-eligible: the software chain is bit-exact. Whether the
        // whole path is verified needs loopback measurement; until then
        // this is honest "eligible, software-identical".
        Ok(aurora_core::OutputIntegrity::Unverified)
    } else {
        Ok(aurora_core::OutputIntegrity::Transformed)
    }
}

// ---------------------------------------------------------------------------
// Windows plumbing
// ---------------------------------------------------------------------------

const RPC_E_CHANGED_MODE: windows::core::HRESULT = windows::core::HRESULT(0x8001_0106u32 as i32);

fn com_init() -> Result<(), EngineError> {
    unsafe {
        let hr = windows::Win32::System::Com::CoInitializeEx(
            None,
            windows::Win32::System::Com::COINIT_MULTITHREADED,
        );
        // S_FALSE (1, already MTA) is fine. RPC_E_CHANGED_MODE means this
        // thread already runs in an STA (someone below us initialized it):
        // the thread's apartment cannot change, but COM calls still work —
        // cross-apartment marshaling handles it. Proceed.
        if hr.is_err() && hr != windows::core::HRESULT(1) && hr != RPC_E_CHANGED_MODE {
            return Err(EngineError::Internal(format!("CoInitializeEx: {hr:?}")));
        }
    }
    Ok(())
}

fn com_uninit() {
    unsafe {
        windows::Win32::System::Com::CoUninitialize();
    }
}

unsafe fn probe_device(device: &cpal::Device, source: PcmFormat) -> Result<bool, EngineError> {
    use windows::Win32::Devices::Properties::DEVPKEY_Device_FriendlyName;
    use windows::Win32::Foundation::PROPERTYKEY;
    use windows::Win32::Media::Audio::{
        AUDCLNT_SHAREMODE_EXCLUSIVE, DEVICE_STATE_ACTIVE, IAudioClient, IMMDeviceEnumerator,
        MMDeviceEnumerator, eRender,
    };
    use windows::Win32::System::Com::StructuredStorage::PropVariantClear;
    use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, STGM_READ};

    // Resolve the WASAPI endpoint behind the cpal Device by friendly name.
    let target_name = device.to_string();

    let enumerator: IMMDeviceEnumerator = unsafe {
        CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
            .map_err(|e| EngineError::Internal(format!("CoCreateInstance: {e}")))?
    };

    let collection = unsafe {
        enumerator
            .EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)
            .map_err(|e| EngineError::Internal(format!("EnumAudioEndpoints: {e}")))?
    };

    let count = unsafe {
        collection
            .GetCount()
            .map_err(|e| EngineError::Internal(format!("GetCount: {e}")))?
    };

    for idx in 0..count {
        let endpoint = unsafe {
            collection
                .Item(idx)
                .map_err(|e| EngineError::Internal(format!("Item({idx}): {e}")))?
        };

        // Read the friendly name PROPVARIANT.
        let mut prop_value = {
            let store = unsafe {
                endpoint
                    .OpenPropertyStore(STGM_READ)
                    .map_err(|e| EngineError::Internal(format!("OpenPropertyStore: {e}")))?
            };
            unsafe {
                store
                    .GetValue(&DEVPKEY_Device_FriendlyName as *const _ as *const PROPERTYKEY)
                    .map_err(|e| EngineError::Internal(format!("GetValue: {e}")))?
            }
        };

        let name = prop_variant_string(&prop_value);
        let _ = unsafe { PropVariantClear(&mut prop_value) };
        if name != target_name {
            continue;
        }

        // Found the endpoint: probe the format in exclusive mode.
        let client: IAudioClient = unsafe {
            endpoint
                .Activate(windows::Win32::System::Com::CLSCTX_ALL, None)
                .map_err(|e| EngineError::Internal(format!("Activate: {e}")))?
        };

        let format = pcm_to_waveformatex(source);
        let hr = unsafe { client.IsFormatSupported(AUDCLNT_SHAREMODE_EXCLUSIVE, &format, None) };
        // S_OK: the device accepts the exact format with no conversion.
        // Any error (incl. AUDCLNT_E_UNSUPPORTED_FORMAT): not bit-exact.
        return Ok(hr.is_ok());
    }

    Err(EngineError::DeviceNotFound(target_name))
}

pub fn prop_variant_string(
    value: &windows::Win32::System::Com::StructuredStorage::PROPVARIANT,
) -> String {
    // The union's LPWSTR member, read as *const u16 (same pattern cpal uses:
    // PROPVARIANT unions have no safe accessor).
    use windows::Win32::System::Variant::VT_LPWSTR;
    unsafe {
        let prop_variant = &value.Anonymous.Anonymous;
        if prop_variant.vt != VT_LPWSTR {
            return String::new();
        }
        let ptr = *(&prop_variant.Anonymous as *const _ as *const *const u16);
        if ptr.is_null() {
            return String::new();
        }
        let mut len = 0usize;
        const MAX: usize = 32768;
        while len < MAX && *ptr.add(len) != 0 {
            len += 1;
        }
        let slice = std::slice::from_raw_parts(ptr, len);
        String::from_utf16_lossy(slice)
    }
}

/// Map a PCM format to the WAVEFORMATEX the device must accept verbatim.
///
/// Exclusive mode does no conversion: 24-bit PCM probes as the packed
/// 3-byte-per-sample representation, floats as IEEE float.
pub fn pcm_to_waveformatex(source: PcmFormat) -> windows::Win32::Media::Audio::WAVEFORMATEX {
    use windows::Win32::Media::Audio::WAVEFORMATEX;

    let bits = source.bits_per_sample;
    let container_bits = bits.div_ceil(8) * 8;
    let block_align = source.channels * (container_bits / 8);

    WAVEFORMATEX {
        wFormatTag: match source.sample_format {
            SampleFormat::Float => 0x0003, // WAVE_FORMAT_IEEE_FLOAT
            _ => 0x0001,                   // WAVE_FORMAT_PCM
        },
        nChannels: source.channels,
        nSamplesPerSec: source.sample_rate,
        nAvgBytesPerSec: source.sample_rate * u32::from(block_align),
        nBlockAlign: block_align,
        wBitsPerSample: container_bits,
        cbSize: 0,
    }
}

// ---------------------------------------------------------------------------
// Loopback verification (ADR 0002 exit criteria)
// ---------------------------------------------------------------------------

/// Verification plan for one playback: what to compare and how.
///
/// The loopback captures the *device's own render stream* (what actually
/// reached the endpoint) and compares it against the reference bytes the
/// engine pushed. Shared-mode loopback comes back through the Windows
/// mixer (it re-encodes to the mix format), so byte identity is only
/// expected when the source format equals the mix format and no other
/// audio is playing.
#[derive(Debug, Clone)]
pub struct LoopbackPlan {
    pub device_key: String,
    pub expected_format: PcmFormat,
    /// Reference bytes captured from the engine side during playback.
    pub reference: Vec<u8>,
}

/// Compare a captured loopback buffer with the reference.
///
/// Returns the fraction of frames that are byte-identical (0.0..=1.0).
/// Alignment is attempted over a small leading-offset window: loopback
/// capture typically starts a few frames after the render stream began,
/// and the engine's ring already contained pre-roll.
pub fn loopback_identity(plan: &LoopbackPlan, captured: &[u8]) -> f64 {
    let frame_bytes = usize::from(plan.expected_format.channels)
        * usize::from(plan.expected_format.bits_per_sample).div_ceil(8);
    if frame_bytes == 0 || plan.reference.is_empty() || captured.is_empty() {
        return 0.0;
    }

    let ref_frames = plan.reference.len() / frame_bytes;
    let cap_frames = captured.len() / frame_bytes;
    if ref_frames == 0 || cap_frames == 0 {
        return 0.0;
    }

    // Try offsets 0..=8 frames; keep the best match. The comparison window
    // for each offset is bounded by BOTH buffers.
    let mut best = 0usize;
    let mut best_window = 0usize;
    for off in 0..8.min(cap_frames) {
        let window = ref_frames.min(cap_frames - off);
        if window == 0 {
            continue;
        }
        let matches = (0..window)
            .take_while(|&i| {
                let r = i * frame_bytes;
                let c = (i + off) * frame_bytes;
                plan.reference[r..r + frame_bytes] == captured[c..c + frame_bytes]
            })
            .count();
        if matches > best {
            best = matches;
            best_window = window;
        }
    }
    // Identity fraction over the best overlapping region: a truncated
    // capture that matches perfectly on its overlap still counts as
    // verified for that region; the caller decides the threshold.
    if best_window == 0 {
        return 0.0;
    }
    f64::from(u32::try_from(best).unwrap_or(0)) / f64::from(u32::try_from(best_window).unwrap_or(1))
}

/// ADR 0002 verdict from an identity measurement.
///
/// 99.9% byte-identical frames or better: Verified (the complete chain
/// preserved the samples; this is a measurement, not an assumption).
/// Anything below means the path is genuinely lossy somewhere, and
/// honest reporting means Transformed.
pub fn verdict_from_identity(identity: f64) -> aurora_core::OutputIntegrity {
    if identity >= 0.999 {
        aurora_core::OutputIntegrity::Verified
    } else {
        aurora_core::OutputIntegrity::Transformed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aurora_core::ChannelLayout;

    fn s16_stereo() -> PcmFormat {
        PcmFormat {
            sample_rate: 44_100,
            channels: 2,
            bits_per_sample: 16,
            sample_format: SampleFormat::SignedInt,
            channel_layout: ChannelLayout::Interleaved,
        }
    }

    #[test]
    fn identical_buffers_score_one() {
        let fmt = s16_stereo();
        let reference: Vec<u8> = (0..4096u16).flat_map(|v| v.to_le_bytes()).collect();
        let plan = LoopbackPlan {
            device_key: "test".into(),
            expected_format: fmt,
            reference: reference.clone(),
        };
        assert!((loopback_identity(&plan, &reference) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn offset_capture_aligns() {
        // The capture starts 3 frames late; alignment must find them.
        let fmt = s16_stereo();
        let frame = [0x11u8, 0x22, 0x33, 0x44];
        let reference: Vec<u8> = frame.repeat(1000);
        let captured: Vec<u8> = frame
            .repeat(3)
            .iter()
            .chain(frame.repeat(1000).iter())
            .copied()
            .collect();
        let plan = LoopbackPlan {
            device_key: "test".into(),
            expected_format: fmt,
            reference,
        };
        assert!(loopback_identity(&plan, &captured) > 0.99);
    }

    #[test]
    fn garbage_scores_zero() {
        let fmt = s16_stereo();
        let reference: Vec<u8> = vec![0xAA; 8192];
        let captured: Vec<u8> = vec![0x55; 8192];
        let plan = LoopbackPlan {
            device_key: "test".into(),
            expected_format: fmt,
            reference,
        };
        assert!(loopback_identity(&plan, &captured) < 0.01);
    }

    #[test]
    fn verdict_thresholds() {
        assert_eq!(
            verdict_from_identity(1.0),
            aurora_core::OutputIntegrity::Verified
        );
        assert_eq!(
            verdict_from_identity(0.999),
            aurora_core::OutputIntegrity::Verified
        );
        assert_eq!(
            verdict_from_identity(0.99),
            aurora_core::OutputIntegrity::Transformed
        );
        assert_eq!(
            verdict_from_identity(0.5),
            aurora_core::OutputIntegrity::Transformed
        );
    }
}
