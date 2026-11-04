/** Track metadata (tags) for the web shell.
 *
 * music-metadata parses ID3v2/FLAC/Vorbis/MP4/Opus/APE tags plus
 * stream properties (duration, sample rate, channels, bit depth) from
 * a browser File via a web stream — no server, no wasm changes.
 *
 * Honesty: parsing failures degrade to filename-derived names (the old
 * behavior); a track with no tags shows the stem, never an error.
 */

import { parseBlob } from "music-metadata";

export interface TrackTags {
  title: string | null;
  artist: string | null;
  album: string | null;
  /** Track number in the album, when tagged. */
  trackNo: number | null;
  year: number | null;
  genre: string | null;
  durationMs: number | null;
  sampleRate: number | null;
  channels: number | null;
  bitsPerSample: number | null;
  /** Codec/container ("mp3", "flac", …), when recognized. */
  codec: string | null;
  /** Bitrate in kbps for lossy, when reported. */
  bitrateKbps: number | null;
  /** Embedded cover art (ID3v2 APIC / FLAC picture / MP4 covr), when
   *  present: MIME format + raw picture bytes. */
  picture: { format: string; data: Uint8Array } | null;
}

export async function readTrackTags(file: File): Promise<TrackTags> {
  // Bounded parse: music-metadata can hang indefinitely on some files
  // in browser streams (a deadlock in the duration pass). A tag read
  // that cannot finish in 5s is worth nothing — the filename fallback
  // covers it (honest degradation, never a stuck ingest).
  try {
    const mm = await Promise.race([
      parseBlob(file, { duration: true }),
      new Promise<never>((_, reject) =>
        setTimeout(() => reject(new Error("tag parse timeout")), 5000),
      ),
    ]);
    const c = mm.common;
    return {
      title: c.title?.trim() || null,
      artist: c.artist?.trim() || c.albumartist?.trim() || null,
      album: c.album?.trim() || null,
      trackNo: c.track?.no ?? null,
      year: c.year ?? null,
      genre: c.genre?.[0] ?? null,
      durationMs: mm.format.duration ? Math.round(mm.format.duration * 1000) : null,
      sampleRate: mm.format.sampleRate ?? null,
      channels: mm.format.numberOfChannels ?? null,
      bitsPerSample: mm.format.bitsPerSample ?? null,
      codec: mm.format.container?.toLowerCase() ?? null,
      bitrateKbps: mm.format.bitrate ? Math.round(mm.format.bitrate / 1000) : null,
      picture: c.picture?.[0]?.data
        ? { format: c.picture[0].format, data: c.picture[0].data }
        : null,
    };
  } catch {
    // Corrupt/unknown tags: honest empties, filename fallback stays.
    return {
      title: null,
      artist: null,
      album: null,
      trackNo: null,
      year: null,
      genre: null,
      durationMs: null,
      sampleRate: null,
      channels: null,
      bitsPerSample: null,
      codec: null,
      bitrateKbps: null,
      picture: null,
    };
  }
}
