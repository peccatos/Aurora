import io

p = r'C:/Users/madei/OneDrive/Desktop/aurora/crates/aurora-tauri/src/commands.rs'
s = io.open(p, encoding='utf-8').read()

anchor = '''#[tauri::command]
pub fn pause_playback(state: State<AppState>) -> Result<PlaybackStatus, CommandError> {'''
addition = '''// ---------------------------------------------------------------------------
// Exclusive bit-perfect path (Windows/WASAPI)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct ExclusivePlayResult {
    /// True when the exclusive path was taken; false = fell back to the
    /// shared path (device refused the format — reported honestly).
    pub exclusive: bool,
    /// Why the fallback happened, when it did.
    pub fallback_reason: Option<String>,
    pub status: PlaybackStatus,
}

/// Play a track through WASAPI exclusive mode: no mixer, no DSP, the
/// source's exact bytes to the endpoint.
///
/// If the device rejects the format, we fall back to the normal shared
/// path and say so — never silently claim exclusive.
#[tauri::command]
pub fn play_exclusive_track(
    request: PlayRequest,
    state: State<AppState>,
) -> Result<ExclusivePlayResult, CommandError> {
    let device = match request.device.as_deref() {
        None | Some("") => aurora_audio::default_output_device()?,
        Some(key) => aurora_audio::output_devices()?
            .into_iter()
            .find(|d| d.key() == key)
            .ok_or_else(|| CommandError::Device(key.to_string()))?,
    };

    let mut engine = state.engine.lock().map_err(|_| CommandError::Poisoned)?;
    let path = std::path::PathBuf::from(&request.path);
    match engine.play_exclusive(path, &device) {
        Ok(()) => Ok(ExclusivePlayResult {
            exclusive: true,
            fallback_reason: None,
            status: status_of(&engine),
        }),
        Err(aurora_audio::EngineError::FormatUnsupported(reason)) => {
            // Honest fallback: the device cannot take this format
            // exclusively; play the normal way and report it.
            tracing::warn!(%reason, "exclusive refused, falling back to shared");
            engine.play_with_replaygain(
                std::path::PathBuf::from(&request.path),
                &device,
                aurora_dsp::DspConfig::default(),
                request
                    .replaygain_track_db
                    .map(|t| (t, request.replaygain_album_db)),
            )?;
            Ok(ExclusivePlayResult {
                exclusive: false,
                fallback_reason: Some(reason),
                status: status_of(&engine),
            })
        }
        Err(e) => Err(e.into()),
    }
}

#[derive(Debug, Serialize)]
pub struct BitPerfectVerdict {
    /// True when an exclusive stream is playing.
    pub exclusive_active: bool,
    /// The physical measurement, when it ran.
    pub identity: Option<f64>,
    pub frames_compared: Option<u64>,
    /// Honest ADR 0002 verdict ("verified" / "transformed" /
    /// "unverified" — unverified = measurement failed, never assumed).
    pub integrity: String,
    pub note: String,
}

/// Run the physical loopback verification on the playing exclusive
/// stream: capture what the endpoint rendered, compare byte-for-byte
/// with the reference the engine pushed.
#[tauri::command]
pub fn verify_bit_perfect(
    seconds: Option<f64>,
    state: State<AppState>,
) -> Result<BitPerfectVerdict, CommandError> {
    let engine = state.engine.lock().map_err(|_| CommandError::Poisoned)?;
    let duration = std::time::Duration::from_secs_f64(seconds.unwrap_or(2.0).clamp(0.5, 5.0));
    match engine.verify_loopback(duration) {
        Ok(v) => Ok(BitPerfectVerdict {
            exclusive_active: true,
            identity: Some(v.identity),
            frames_compared: Some(v.frames_compared),
            integrity: match v.integrity {
                aurora_core::OutputIntegrity::Verified => "verified".into(),
                aurora_core::OutputIntegrity::Transformed => "transformed".into(),
                aurora_core::OutputIntegrity::Unverified => "unverified".into(),
            },
            note: format!(
                "{:.2}% byte-identical frames over {} frames measured",
                v.identity * 100.0,
                v.frames_compared
            ),
        }),
        Err(e) => Ok(BitPerfectVerdict {
            exclusive_active: false,
            identity: None,
            frames_compared: None,
            integrity: "unverified".into(),
            note: e.to_string(),
        }),
    }
}

''' + anchor
assert anchor in s
s = s.replace(anchor, addition, 1)

old_reg = '''            recover_playback,'''
new_reg = '''            recover_playback,
            play_exclusive_track,
            verify_bit_perfect,'''
assert old_reg in s
s = s.replace(old_reg, new_reg, 1)

io.open(p, 'w', encoding='utf-8', newline='\n').write(s)
print('tauri ok')
