//! Audio source: probing a file and decoding it into PCM chunks.

use std::fs::File;
use std::path::Path;

use aurora_core::{AudioFormat, PcmFormat, SampleFormat};
use symphonia::core::audio::sample::SampleFormat as SymphoniaSampleFormat;
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::formats::TrackType;
use symphonia::core::formats::probe::Hint;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

use crate::error::{DecodeError, SourceError};
use crate::pcm::PcmChunk;

/// Static description of a probed audio source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceInfo {
    pub pcm_format: PcmFormat,
    /// Container/codec family, when recognized.
    pub audio_format: AudioFormat,
    /// Total duration in milliseconds, when the container declares it.
    pub duration_ms: Option<u64>,
    /// Total playable frames, when known from the container.
    pub total_frames: Option<u64>,
}

/// An open, decodable audio source.
pub struct AudioSource {
    format: Box<dyn symphonia::core::formats::FormatReader>,
    decoder: Box<dyn symphonia::core::codecs::audio::AudioDecoder>,
    track_id: u32,
    info: SourceInfo,
    /// The first decoded chunk, produced by the format probe in `open`;
    /// yielded by `next_chunk` before pulling new packets.
    first_chunk: Option<PcmChunk>,
}

impl AudioSource {
    /// Open a file and probe it for a decodable audio track.
    pub fn open(path: &Path) -> Result<Self, SourceError> {
        let file = File::open(path)?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());

        // Extension hint accelerates probing but is not required.
        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }

        let mut format = symphonia::default::get_probe()
            .probe(
                &hint,
                mss,
                FormatOptions::default(),
                MetadataOptions::default(),
            )
            .map_err(|_| SourceError::UnsupportedFormat)?;

        let track_id = format
            .default_track(TrackType::Audio)
            .ok_or(SourceError::NoAudioTrack)?
            .id;

        let params = format
            .tracks()
            .iter()
            .find(|t| t.id == track_id)
            .and_then(|t| t.codec_params.as_ref())
            .and_then(|p| p.audio())
            .ok_or_else(|| SourceError::IncompleteTrack("missing audio codec parameters".into()))?
            .clone();

        // Track facts captured before `format` is boxed away.
        let (track_num_frames, track_duration, track_time_base) = {
            let track = format
                .tracks()
                .iter()
                .find(|t| t.id == track_id)
                .ok_or(SourceError::NoAudioTrack)?;
            (track.num_frames, track.duration, track.time_base)
        };

        let mut decoder = symphonia::default::get_codecs()
            .make_audio_decoder(&params, &AudioDecoderOptions::default())
            .map_err(|e| SourceError::UnsupportedCodec(e.to_string()))?;

        // Decode the first packet to learn the actual decoder output format.
        // Codec parameters lie for compressed formats: MP3/Vorbis/FLAC
        // decode to f32 even when the container declares integer PCM.
        let (pcm_format, first_chunk) = {
            let mut first_chunk = None;
            let mut actual_format = None;
            while let Some(packet) = format.next_packet().ok().flatten() {
                if packet.track_id != track_id {
                    continue;
                }
                match decoder.decode(&packet) {
                    Ok(decoded) => {
                        actual_format = chunk_format_from_decoded(&decoded);
                        if actual_format.is_some() {
                            first_chunk = chunk_from_decoded(&decoded);
                        }
                        break;
                    }
                    // A first corrupt packet is tolerable: fall back to
                    // parameters and hope the next chunk reveals the truth.
                    Err(_) => continue,
                }
            }
            match actual_format {
                Some(f) => (f, first_chunk),
                None => (
                    pcm_format_from_params(&params).ok_or_else(|| {
                        SourceError::IncompleteTrack("incomplete audio parameters".into())
                    })?,
                    None,
                ),
            }
        };

        // Duration: prefer the container-declared frame count (exact for
        // FLAC/WAV); fall back to timebase-derived duration.
        let sample_rate = u64::from(pcm_format.sample_rate);
        let total_frames = track_num_frames;
        let duration_ms = total_frames
            .filter(|_| sample_rate > 0)
            .map(|frames| frames * 1000 / sample_rate)
            .or_else(|| {
                track_duration.map(|d| {
                    let millis = track_time_base
                        .map(|tb| tb.calc_duration_saturating(d).as_millis())
                        .unwrap_or_default();
                    u64::try_from(millis).unwrap_or(0)
                })
            });

        let info = SourceInfo {
            pcm_format,
            audio_format: audio_format_from_extension(path),
            duration_ms,
            total_frames,
        };

        Ok(Self {
            format,
            decoder,
            track_id,
            info,
            first_chunk,
        })
    }

    pub fn info(&self) -> &SourceInfo {
        &self.info
    }

    pub fn pcm_format(&self) -> PcmFormat {
        self.info.pcm_format
    }

    /// Decode the next packet into a PCM chunk.
    ///
    /// Recoverable errors skip the packet internally up to a bounded number
    /// of consecutive failures; beyond that the source reports the failure
    /// instead of silently looping over a corrupt region.
    pub fn next_chunk(&mut self) -> Result<PcmChunk, DecodeError> {
        if let Some(chunk) = self.first_chunk.take() {
            return Ok(chunk);
        }

        const MAX_CONSECUTIVE_SKIPS: usize = 16;

        let mut skips = 0;
        loop {
            let packet = match self.format.next_packet() {
                Ok(Some(p)) => p,
                Ok(None) => return Err(DecodeError::EndOfStream),
                Err(e) => return Err(e.into()),
            };

            if packet.track_id != self.track_id {
                continue;
            }

            match self.decoder.decode(&packet) {
                Ok(decoded) => {
                    return chunk_from_decoded(&decoded).ok_or_else(|| {
                        DecodeError::Unrecoverable("decoded buffer yielded no PCM".into())
                    });
                }
                Err(e) => {
                    let err: DecodeError = e.into();
                    match err {
                        DecodeError::Recoverable(msg) => {
                            skips += 1;
                            tracing::debug!(skips, %msg, "skipping corrupt packet");
                            if skips >= MAX_CONSECUTIVE_SKIPS {
                                return Err(DecodeError::Unrecoverable(format!(
                                    "too many consecutive corrupt packets ({skips})"
                                )));
                            }
                            continue;
                        }
                        other => return Err(other),
                    }
                }
            }
        }
    }

    /// Seek to an absolute position (seconds) in the stream.
    ///
    /// Accurate mode: symphonia seeks to the packet at or before the
    /// position. The decoded-format probe state is unaffected; the first
    /// decoded chunk after the seek continues from the new position.
    /// The buffered first_chunk (already consumed in normal playback) is
    /// dropped by construction: seek is only valid after playback began.
    pub fn seek(
        &mut self,
        to: symphonia::core::formats::SeekTo,
    ) -> Result<symphonia::core::formats::SeekedTo, symphonia::core::errors::Error> {
        // Drop any pending probe chunk: after a seek it is stale.
        self.first_chunk = None;
        self.format
            .seek(symphonia::core::formats::SeekMode::Accurate, to)
    }

    /// Whether the first (probe) chunk is still pending — true before any
    /// next_chunk call, false after.
    pub fn has_pending_first_chunk(&self) -> bool {
        self.first_chunk.is_some()
    }

    /// Decode the whole source into a buffer (bounded by `limit_bytes`).
    ///
    /// For tests and offline analysis; the playback path streams via
    /// `next_chunk` instead.
    pub fn decode_all(&mut self, limit_bytes: usize) -> Result<Vec<PcmChunk>, DecodeError> {
        let mut chunks = Vec::new();
        let mut total = 0usize;
        loop {
            match self.next_chunk() {
                Ok(chunk) => {
                    total += chunk.as_bytes().len();
                    if total > limit_bytes {
                        return Err(DecodeError::Unrecoverable(
                            "decode_all limit exceeded".into(),
                        ));
                    }
                    chunks.push(chunk);
                }
                Err(DecodeError::EndOfStream) => return Ok(chunks),
                Err(e) => return Err(e),
            }
        }
    }
}

/// Waveform peaks for the seek-bar overview: `count` absolute peak values
/// (0..1) sampled across the WHOLE file. Decodes the full stream (f32,
/// mono-mixed); bounded by the duration so a 10-minute FLAC is well under
/// a second of work. Called once per track (cache at the command layer).
pub fn waveform_peaks(path: &Path, count: usize) -> Result<Vec<f32>, SourceError> {
    let mut source = AudioSource::open(path)?;
    let duration_s = source.info().duration_ms.unwrap_or(0) as f64 / 1000.0;
    if duration_s <= 0.0 || count == 0 {
        return Ok(Vec::new());
    }
    let mut peaks = vec![0f32; count];
    // Rough frames-per-bin: total frames estimated from duration + rate.
    let rate = f64::from(source.pcm_format().sample_rate);
    let total_frames_est = (duration_s * rate).max(1.0);

    let mut frame_idx: u64 = 0;
    loop {
        let chunk = match source.next_chunk() {
            Ok(c) => c,
            Err(DecodeError::EndOfStream) => break,
            Err(_) => break,
        };
        let samples = chunk.to_f32_interleaved();
        let nch = usize::from(source.pcm_format().channels.max(1));
        let frames = samples.len() / nch.max(1);
        for f in 0..frames {
            let bin = ((frame_idx as f64 / total_frames_est) * count as f64) as usize;
            let bin = bin.min(count - 1);
            let mut m = 0f32;
            for ch in 0..nch {
                let v = samples[f * nch + ch];
                m = m.max(v.abs());
            }
            if m > peaks[bin] {
                peaks[bin] = m;
            }
            frame_idx += 1;
        }
    }
    // Normalize to 0..1 (full-scale stays 1.0).
    let max = peaks.iter().cloned().fold(0f32, f32::max);
    if max > 0.0 {
        for p in peaks.iter_mut() {
            *p /= max;
        }
    }
    Ok(peaks)
}

/// Extract a PcmChunk from a decoded symphonia buffer, preserving the native
/// sample representation (bytes are copied interleaved without conversion).
fn chunk_format_from_decoded(
    decoded: &symphonia::core::audio::GenericAudioBufferRef<'_>,
) -> Option<PcmFormat> {
    let spec = decoded.spec();
    let symphonia_format = symphonia_sample_format(decoded);
    let sample_format = match symphonia_format {
        SymphoniaSampleFormat::U8
        | SymphoniaSampleFormat::U16
        | SymphoniaSampleFormat::U24
        | SymphoniaSampleFormat::U32 => SampleFormat::UnsignedInt,
        SymphoniaSampleFormat::S8
        | SymphoniaSampleFormat::S16
        | SymphoniaSampleFormat::S24
        | SymphoniaSampleFormat::S32 => SampleFormat::SignedInt,
        SymphoniaSampleFormat::F32 | SymphoniaSampleFormat::F64 => SampleFormat::Float,
    };
    Some(PcmFormat {
        sample_rate: spec.rate(),
        channels: spec.channels().count() as u16,
        bits_per_sample: bits_per_sample(symphonia_format),
        sample_format,
        channel_layout: aurora_core::ChannelLayout::Interleaved,
    })
}

fn chunk_from_decoded(
    decoded: &symphonia::core::audio::GenericAudioBufferRef<'_>,
) -> Option<PcmChunk> {
    let format = chunk_format_from_decoded(decoded)?;
    let mut data = Vec::new();
    decoded.copy_bytes_to_vec_interleaved(&mut data);
    PcmChunk::from_bytes(format, data)
}

fn symphonia_sample_format(
    buf: &symphonia::core::audio::GenericAudioBufferRef<'_>,
) -> SymphoniaSampleFormat {
    use symphonia::core::audio::GenericAudioBufferRef as R;
    match buf {
        R::U8(_) => SymphoniaSampleFormat::U8,
        R::U16(_) => SymphoniaSampleFormat::U16,
        R::U24(_) => SymphoniaSampleFormat::U24,
        R::U32(_) => SymphoniaSampleFormat::U32,
        R::S8(_) => SymphoniaSampleFormat::S8,
        R::S16(_) => SymphoniaSampleFormat::S16,
        R::S24(_) => SymphoniaSampleFormat::S24,
        R::S32(_) => SymphoniaSampleFormat::S32,
        R::F32(_) => SymphoniaSampleFormat::F32,
        R::F64(_) => SymphoniaSampleFormat::F64,
    }
}

fn bits_per_sample(format: SymphoniaSampleFormat) -> u16 {
    match format {
        SymphoniaSampleFormat::U8 | SymphoniaSampleFormat::S8 => 8,
        SymphoniaSampleFormat::U16 | SymphoniaSampleFormat::S16 => 16,
        SymphoniaSampleFormat::U24 | SymphoniaSampleFormat::S24 => 24,
        SymphoniaSampleFormat::U32 | SymphoniaSampleFormat::S32 | SymphoniaSampleFormat::F32 => 32,
        SymphoniaSampleFormat::F64 => 64,
    }
}

fn pcm_format_from_params(
    params: &symphonia::core::codecs::audio::AudioCodecParameters,
) -> Option<PcmFormat> {
    // Prefer the codec's declared sample format; formats without a static
    // declaration (MP3 etc.) decode to signed int at the reported depth,
    // defaulting to 16 — the common case.
    let sample_format = params
        .sample_format
        .map(|f| match f {
            SymphoniaSampleFormat::U8
            | SymphoniaSampleFormat::U16
            | SymphoniaSampleFormat::U24
            | SymphoniaSampleFormat::U32 => SampleFormat::UnsignedInt,
            SymphoniaSampleFormat::S8
            | SymphoniaSampleFormat::S16
            | SymphoniaSampleFormat::S24
            | SymphoniaSampleFormat::S32 => SampleFormat::SignedInt,
            SymphoniaSampleFormat::F32 | SymphoniaSampleFormat::F64 => SampleFormat::Float,
        })
        .unwrap_or(SampleFormat::SignedInt);

    let bits = params.bits_per_sample.map(|b| b as u16).unwrap_or(16);

    let channels = params.channels.as_ref().map(|c| c.count()).unwrap_or(0) as u16;
    if channels == 0 {
        return None;
    }

    Some(PcmFormat {
        sample_rate: params.sample_rate?,
        channels,
        bits_per_sample: bits,
        sample_format,
        channel_layout: aurora_core::ChannelLayout::Interleaved,
    })
}

fn audio_format_from_extension(path: &Path) -> AudioFormat {
    path.extension()
        .and_then(|e| e.to_str())
        .map(AudioFormat::from_extension)
        .unwrap_or(AudioFormat::Unknown)
}
