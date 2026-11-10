/** DTO contracts with the Rust backend (aurora-tauri commands). */

export interface DeviceDto {
  key: string;
  name: string;
}

export interface EqBand {
  kind: "peaking" | "low_shelf" | "high_shelf" | "low_pass" | "high_pass";
  freq_hz: number;
  q: number;
  gain_db: number;
  enabled: boolean;
}

export interface ReverbParams {
  room_size: number;
  damping: number;
  wet: number;
  width: number;
  input_lowpass: number;
}

export interface DspConfig {
  eq: { bands: EqBand[]; preamp_db: number };
  volume_db: number;
  replaygain: "off" | "track" | "album";
  reverb: ReverbParams;
  speed: number;
  tools: {
    balance: number;
    phase_invert_left: boolean;
    stereo_width: number;
    soft_clip: number;
  };
  crossfeed?: {
    enabled: boolean;
    cutoff_hz: number;
    feed_db: number;
  };
  convolution?: {
    enabled: boolean;
    ir_id: number;
    wet: number;
    dry: number;
    predelay_ms: number;
    ir_gain_db: number;
  };
  room_correction?: {
    enabled: boolean;
    filters: {
      kind: "pk" | "ls" | "hs";
      fc_hz: number;
      gain_db: number;
      q: number;
    }[];
  };
}

export interface QueueTrack {
  path: string;
  title?: string | null;
  artist?: string | null;
  replaygain_track_db?: number | null;
  replaygain_album_db?: number | null;
}

export interface QueueDto {
  tracks: QueueTrack[];
  current: number | null;
  display: string[];
}

export interface TrackDto {
  id: string;
  path: string;
  title: string | null;
  artist: string | null;
  album: string | null;
  genre: string | null;
  year: number | null;
  track_number: number | null;
  format: string;
  duration_ms: number | null;
  replaygain_track_db: number | null;
  replaygain_album_db: number | null;
  bpm?: number | null;
  lyrics: string | null;
  lyrics_language: string | null;
  lyrics_translation: string | null;
  lyrics_translation_language: string | null;
  artwork_data_url: string | null;
  source_url: string | null;
  source_provider: string | null;
  video_url: string | null;
  metadata_source: string | null;
  display: string;
  duration_display: string;
}

export interface ScanSummary {
  discovered: number;
  added: number;
  updated: number;
  pruned: number;
  failed: number;
}

export interface LibraryStatus {
  open: boolean;
  path: string | null;
  track_count: number;
}

export interface PlaylistDto {
  id: string;
  name: string;
  track_count: number;
}

export interface PlaybackStatus {
  state: "idle" | "playing" | "draining" | "paused";
  fault?: string | null;
  sample_rate: number | null;
  channels: number | null;
  bits_per_sample: number | null;
  transformations: string[];
  integrity: "verified" | "transformed" | "unverified";
  duration_s: number | null;
  frames_played: number;
  underruns: number;
}

export interface ExclusivePlayResult {
  exclusive: boolean;
  fallback_reason: string | null;
  status: PlaybackStatus;
}

export interface BitPerfectVerdict {
  exclusive_active: boolean;
  identity: number | null;
  frames_compared: number | null;
  integrity: string;
  note: string;
}

export interface VstPluginDto {
  path: string;
  name: string;
  vendor: string;
  version: string;
  category: string;
  class_id: string;
  input_channels: number;
  output_channels: number;
}

export interface VstSlot {
  path: string;
  class_id: string;
}

export interface VstParamDto {
  id: number;
  title: string;
  value: number;
}

export interface VstPresetDto {
  name: string;
  path: string;
}
