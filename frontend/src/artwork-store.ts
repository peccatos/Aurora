/** Object-URL cache for stored track artwork (spec §1.3).
 *
 *  Artwork lives in IndexedDB as Blobs; rendering needs a URL. Blobs are
 *  URL.createObjectURL'ed once per track and kept in an LRU (max 64) so
 *  re-rendering the home list does not leak object URLs — evicted URLs
 *  are revoked, live ones stay valid for whatever still displays them.
 *
 *  Honesty: a track with no stored artwork returns null — the caller
 *  keeps its fallback icon, nothing is faked.
 */

import { getArtwork, getTrack } from "./library-db";

const MAX_CACHED_URLS = 64;

const urlCache = new Map<string, string>(); // insertion order = LRU order
const pending = new Map<string, Promise<string | null>>();

export function invalidateTrackArtworkUrl(trackId: string): void {
  const url = urlCache.get(trackId);
  if (url) revokeUrl(url);
  urlCache.delete(trackId);
}

function revokeUrl(url: string): void {
  try {
    URL.revokeObjectURL(url);
  } catch {
    // Revoke on a dead URL is harmless; ignore.
  }
}

/** Object URL for a track's stored artwork, or null when the track has
 *  no artwork (or storage failed). Cached per track; LRU-evicts beyond
 *  64 live URLs with proper revocation. */
export async function trackArtworkUrl(trackId: string): Promise<string | null> {
  const cached = urlCache.get(trackId);
  if (cached) {
    // LRU touch: re-insert at the end (most-recently-used).
    urlCache.delete(trackId);
    urlCache.set(trackId, cached);
    return cached;
  }

  // Coalesce concurrent lookups for the same track (re-renders race).
  const inflight = pending.get(trackId);
  if (inflight) return inflight;

  const load = (async (): Promise<string | null> => {
    let blob: Blob | null = null;
    try {
      blob = await getArtwork(trackId);
    } catch {
      blob = null; // storage error: honest null, caller keeps fallback
    }
    try {
      const dataUrl = (await getTrack(trackId))?.artwork_data_url;
      if (dataUrl) return dataUrl;
    } catch {
      /* no persisted data URL available */
    }
    if (!blob) return null;
    const url = URL.createObjectURL(blob);
    urlCache.set(trackId, url);
    // LRU eviction: drop the oldest entries past the cap.
    while (urlCache.size > MAX_CACHED_URLS) {
      const oldest = urlCache.keys().next().value as string | undefined;
      if (oldest === undefined) break;
      const oldUrl = urlCache.get(oldest);
      urlCache.delete(oldest);
      if (oldUrl) revokeUrl(oldUrl);
    }
    return url;
  })();
  pending.set(trackId, load);
  try {
    return await load;
  } finally {
    pending.delete(trackId);
  }
}
