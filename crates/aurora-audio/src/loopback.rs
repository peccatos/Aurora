//! Physical loopback verification (ADR 0002 exit criterion).
//!
//! WASAPI loopback capture records what an endpoint *actually rendered*.
//! In exclusive mode the render stream IS the source stream (no mixer in
//! between), so a byte-identical capture proves the whole chain —
//! decoder -> ring -> exclusive render -> endpoint — preserved every
//! bit. That upgrades the integrity verdict from `Unverified` to
//! `Verified`, a measurement rather than a claim.
//!
//! Honest caveats (documented, not hidden):
//! - The capture starts mid-stream, so the first frames are partial;
//!   alignment windows handle it (see `exclusive::loopback_identity`).
//! - The endpoint must be silent (exclusive mode guarantees this: we own
//!   it) and the capture duration must stay inside the played stream.
//! - Shared-mode loopback passes through the Windows mixer, so identity
//!   is only meaningful when the source equals the mix format.

use std::time::Duration;

use aurora_core::PcmFormat;

use crate::error::EngineError;

/// The verification verdict for one loopback run.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct LoopbackVerdict {
    /// Fraction of byte-identical frames (0.0..=1.0) in the best-aligned
    /// window.
    pub identity: f64,
    /// Frames compared (the overlap window).
    pub frames_compared: u64,
    /// Final ADR 0002 classification: Verified only when >= 99.9% of
    /// frames are bit-identical.
    pub integrity: aurora_core::OutputIntegrity,
}

/// Capture the endpoint's render stream for `duration` and compare with
/// `reference` (the bytes the engine pushed).
///
/// Requires exclusive ownership of the endpoint (true while an
/// exclusive stream is playing); on any WASAPI failure the verdict is
/// honest `Unverified` (we report the failure, never invent success).
#[cfg(target_os = "windows")]
pub fn verify_playback(
    device_key: &str,
    format: PcmFormat,
    reference: &[u8],
    duration: Duration,
) -> Result<LoopbackVerdict, EngineError> {
    let captured = capture_loopback(device_key, format, duration)?;
    let plan = crate::exclusive::LoopbackPlan {
        device_key: device_key.to_string(),
        expected_format: format,
        reference: reference.to_vec(),
    };
    let identity = crate::exclusive::loopback_identity(&plan, &captured);
    let integrity = crate::exclusive::verdict_from_identity(identity);
    let frame_bytes =
        usize::from(format.channels) * usize::from(format.bits_per_sample).div_ceil(8);
    let frames_compared =
        (captured.len() / frame_bytes.max(1)).min(reference.len() / frame_bytes.max(1)) as u64;
    Ok(LoopbackVerdict {
        identity,
        frames_compared,
        integrity,
    })
}

/// Capture `duration` of the endpoint's render stream via WASAPI
/// loopback (shared-mode capture of the endpoint we render to).
#[cfg(target_os = "windows")]
fn capture_loopback(
    device_key: &str,
    format: PcmFormat,
    duration: Duration,
) -> Result<Vec<u8>, EngineError> {
    unsafe { crate::exclusive_stream::com_init_pub()? };
    let result = unsafe { capture_loopback_inner(device_key, format, duration) };
    unsafe { crate::exclusive_stream::com_uninit_pub() };
    result
}

#[cfg(target_os = "windows")]
unsafe fn capture_loopback_inner(
    device_key: &str,
    format: PcmFormat,
    duration: Duration,
) -> Result<Vec<u8>, EngineError> {
    use windows::Win32::Media::Audio::{
        AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, IAudioCaptureClient,
    };
    use windows::Win32::System::Threading::{CREATE_EVENT, CreateEventExW, WaitForSingleObject};
    use windows::core::PCWSTR;

    let (client, _endpoint) =
        unsafe { crate::exclusive_stream::activate_endpoint_pub(device_key)? };

    // Loopback capture always runs in shared mode with the *mix* format.
    // For an exclusively-driven endpoint the mix format equals the
    // exclusive format the endpoint was opened with (we own it), which
    // is exactly what makes byte comparison meaningful.
    let wave = crate::exclusive::pcm_to_waveformatex(format);
    unsafe {
        client
            .Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK,
                20_000_000, // 200ms buffer, REFERENCE_TIME (100ns units)
                0,
                &wave,
                None,
            )
            .map_err(|e| EngineError::StreamBuild(format!("loopback Initialize: {e:?}")))?;
    }

    let event = unsafe { CreateEventExW(None, PCWSTR::null(), CREATE_EVENT(0), 0) }
        .map_err(|e| EngineError::StreamBuild(format!("CreateEventExW: {e:?}")))?;
    unsafe {
        client
            .SetEventHandle(event)
            .map_err(|e| EngineError::StreamBuild(format!("SetEventHandle: {e:?}")))?;
    }

    let capture: IAudioCaptureClient = unsafe { client.GetService() }
        .map_err(|e| EngineError::StreamBuild(format!("GetService(capture): {e:?}")))?;

    unsafe {
        client
            .Start()
            .map_err(|e| EngineError::StreamBuild(format!("loopback Start: {e:?}")))?;
    }

    let frame_bytes =
        (usize::from(format.channels) * usize::from(format.bits_per_sample).div_ceil(8)).max(1);
    let target_bytes =
        (duration.as_millis() as usize) * (format.sample_rate as usize) * frame_bytes / 1000;

    let mut captured: Vec<u8> = Vec::with_capacity(target_bytes);
    let deadline = std::time::Instant::now() + duration + Duration::from_millis(500);

    loop {
        if captured.len() >= target_bytes || std::time::Instant::now() > deadline {
            break;
        }
        let wait = unsafe { WaitForSingleObject(event, 200) };
        if wait.0 != 0 {
            if std::time::Instant::now() > deadline {
                break;
            }
            continue;
        }

        loop {
            let packet = unsafe { capture.GetNextPacketSize() }
                .map_err(|e| EngineError::StreamBuild(format!("GetNextPacketSize: {e:?}")))?;
            if packet == 0 {
                break;
            }
            let mut data: *mut u8 = std::ptr::null_mut();
            let mut frames: u32 = 0;
            let mut flags: u32 = 0;
            unsafe {
                capture
                    .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
                    .map_err(|e| EngineError::StreamBuild(format!("GetBuffer: {e:?}")))?;
            }
            let bytes = frames as usize * frame_bytes;
            // AUDCLNT_BUFFERFLAGS_SILENT (2): the endpoint rendered
            // silence for this packet — record zeros so the timeline
            // stays aligned.
            if flags & 2 != 0 {
                captured.extend(std::iter::repeat_n(0u8, bytes));
            } else if !data.is_null() {
                let slice = unsafe { std::slice::from_raw_parts(data, bytes) };
                captured.extend_from_slice(slice);
            }
            unsafe {
                capture
                    .ReleaseBuffer(frames)
                    .map_err(|e| EngineError::StreamBuild(format!("ReleaseBuffer: {e:?}")))?;
            }
            if captured.len() >= target_bytes {
                break;
            }
        }
    }

    let _ = unsafe { client.Stop() };
    unsafe {
        let _ = windows::Win32::Foundation::CloseHandle(event);
    }
    captured.truncate(target_bytes);
    Ok(captured)
}

/// Non-Windows stub: verification unavailable, honest error.
#[cfg(not(target_os = "windows"))]
pub fn verify_playback(
    _device_key: &str,
    _format: PcmFormat,
    _reference: &[u8],
    _duration: Duration,
) -> Result<LoopbackVerdict, EngineError> {
    Err(EngineError::Internal(
        "loopback verification is only available on Windows (WASAPI)".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aurora_core::{ChannelLayout, SampleFormat};

    fn s16_stereo() -> PcmFormat {
        PcmFormat {
            sample_rate: 44_100,
            channels: 2,
            bits_per_sample: 16,
            sample_format: SampleFormat::SignedInt,
            channel_layout: ChannelLayout::Interleaved,
        }
    }

    /// The pure comparison math is fully tested in exclusive.rs; here we
    /// pin the verdict plumbing shape.
    #[test]
    fn verdict_shape_is_honest() {
        let v = crate::exclusive::verdict_from_identity(1.0);
        assert_eq!(v, aurora_core::OutputIntegrity::Verified);
        let v = crate::exclusive::verdict_from_identity(0.9);
        assert_eq!(v, aurora_core::OutputIntegrity::Transformed);
        let _ = s16_stereo();
    }
}
