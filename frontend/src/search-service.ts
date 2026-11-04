/** Track search & add from external catalogs (план §6.2/6.4).
 *
 * Sources (open, key-less, CORS-friendly):
 *  - iTunes Search API: search by title/artist, 30s preview URLs,
 *    artwork. The preview is a real m4a stream the kernel can decode.
 *  - MusicBrainz: authoritative metadata (recording, artist, release).
 *
 * Adding a result = fetch the preview audio bytes → register with the
 * wasm kernel → queue. Full tracks stay the user's local files: we
 * honestly add what the catalog gives (a preview), never pretend a
 * 30-second clip is the full song.
 */

import { platform, registerWebFile, invoke } from "./invoke";
import { setQueue, playQueueIndex, setToast } from "./state";

export interface SearchHit {
  source: "itunes" | "deezer" | "musicbrainz";
  title: string;
  artist: string;
  album: string | null;
  durationMs: number | null;
  artworkUrl: string | null;
  /** 30s preview (iTunes only). */
  previewUrl: string | null;
  /** MusicBrainz ids for dedup/matching. */
  mbid?: string;
}

// ---------------------------------------------------------------------------
// iTunes Search
// ---------------------------------------------------------------------------

interface ITunesResult {
  trackId: number;
  trackName: string;
  artistName: string;
  collectionName?: string;
  trackTimeMillis?: number;
  artworkUrl100?: string;
  previewUrl?: string;
}

async function searchItunes(query: string, limit = 20): Promise<SearchHit[]> {
  const url = `https://itunes.apple.com/search?term=${encodeURIComponent(query)}&entity=song&limit=${limit}`;
  const res = await fetchWithTimeout(url, 5000);
  if (!res.ok) throw new Error(`iTunes: HTTP ${res.status}`);
  const data = (await res.json()) as { results: ITunesResult[] };
  return data.results.map((r) => ({
    source: "itunes" as const,
    title: r.trackName,
    artist: r.artistName,
    album: r.collectionName ?? null,
    durationMs: r.trackTimeMillis ?? null,
    artworkUrl: r.artworkUrl100
      ? r.artworkUrl100.replace("100x100", "256x256")
      : null,
    previewUrl: r.previewUrl ?? null,
  }));
}

// ---------------------------------------------------------------------------
// MusicBrainz (metadata; no previews)
// ---------------------------------------------------------------------------

interface MbRecording {
  id: string;
  title: string;
  "first-release-date"?: string;
  artistCredits?: { name: string }[];
  releases?: { title: string }[];
  length?: number; // ms
}

async function searchMusicbrainz(query: string, limit = 20): Promise<SearchHit[]> {
  const url = `https://musicbrainz.org/ws/2/recording/?query=${encodeURIComponent(query)}&fmt=json&limit=${limit}`;
  // NOTE: a custom User-Agent header is FORBIDDEN in browsers (spec:
  // forbidden header name) — setting it makes fetch reject with a
  // TypeError. MusicBrainz accepts anonymous browser requests; rate
  // limits (1 req/s) are handled by the 400ms input debounce.
  const res = await fetchWithTimeout(url, 5000);
  if (!res.ok) throw new Error(`MusicBrainz: HTTP ${res.status}`);
  const data = (await res.json()) as { recordings?: MbRecording[] };
  return (data.recordings ?? []).map((r) => ({
    source: "musicbrainz" as const,
    title: r.title,
    artist: r.artistCredits?.map((a) => a.name).join(", ") ?? "Unknown",
    album: r.releases?.[0]?.title ?? null,
    durationMs: r.length ?? null,
    artworkUrl: null,
    previewUrl: null,
    mbid: r.id,
  }));
}

// ---------------------------------------------------------------------------
// Combined search
// ---------------------------------------------------------------------------

/** Fetch with a hard timeout: a blocked/unreachable catalog must not
 *  hang the whole search promise (allSettled only settles rejections,
 *  not pending-forever fetches). */
async function fetchWithTimeout(url: string, ms: number): Promise<Response> {
  const ctrl = new AbortController();
  const timer = setTimeout(() => ctrl.abort(), ms);
  try {
    return await fetch(url, { signal: ctrl.signal });
  } finally {
    clearTimeout(timer);
  }
}

// ---------------------------------------------------------------------------
// Deezer public API (no key): 30s mp3 previews + cover art.
// NOTE: api.deezer.com sends NO CORS headers — direct browser requests
// fail with a TypeError (allSettled swallows it). Results DO come
// through on networks/proxies that add CORS, and the same code serves
// any future server-side proxy. Not the primary source for that reason.
// ---------------------------------------------------------------------------

interface DeezerTrack {
  title: string;
  artist: { name: string };
  album: { title?: string; cover_medium?: string; cover_big?: string };
  duration: number; // seconds
  preview: string; // 30s mp3 URL
}

async function searchDeezer(query: string, limit = 20): Promise<SearchHit[]> {
  const url = `https://api.deezer.com/search?q=${encodeURIComponent(query)}&limit=${limit}`;
  const res = await fetchWithTimeout(url, 5000);
  if (!res.ok) throw new Error(`Deezer: HTTP ${res.status}`);
  const data = (await res.json()) as { data?: DeezerTrack[] };
  return (data.data ?? []).map((t) => ({
    source: "deezer" as const,
    title: t.title,
    artist: t.artist?.name ?? "Unknown",
    album: t.album?.title ?? null,
    durationMs: t.duration ? t.duration * 1000 : null,
    artworkUrl: t.album?.cover_medium ?? t.album?.cover_big ?? null,
    previewUrl: t.preview || null,
  }));
}

export async function searchTracks(query: string): Promise<SearchHit[]> {
  if (!query.trim()) return [];
  const [itunes, mb, deezer] = await Promise.allSettled([
    searchItunes(query),
    searchMusicbrainz(query),
    searchDeezer(query),
  ]);
  const hits: SearchHit[] = [];
  if (itunes.status === "fulfilled") hits.push(...itunes.value);
  if (deezer.status === "fulfilled") hits.push(...deezer.value);
  if (mb.status === "fulfilled") hits.push(...mb.value);

  // Dedup: same title+artist (case-insensitive), preview-bearing
  // sources first (iTunes, then Deezer; MB is metadata-only).
  const seen = new Set<string>();
  return hits.filter((h) => {
    const key = `${h.title.toLowerCase()}|${h.artist.toLowerCase()}`;
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

// ---------------------------------------------------------------------------
// Play / add
// ---------------------------------------------------------------------------

/** In-flight preview bytes cache (id -> blob), so replays do not
 *  re-download. */
const previewBytes = new Map<string, ArrayBuffer>();

/** Download the 30s preview and add it to the queue as a playable
 *  track. Honest: it is a preview clip, labeled in the title. */
export async function addPreviewToQueue(hit: SearchHit): Promise<boolean> {
  if (platform !== "web") {
    setToast("Search & preview is web-only for now");
    return false;
  }
  if (!hit.previewUrl) {
    setToast("No preview available for this track");
    return false;
  }
  try {
    let bytes = previewBytes.get(hit.previewUrl);
    if (!bytes) {
      const res = await fetchWithTimeout(hit.previewUrl, 8000);
      if (!res.ok) throw new Error(`preview HTTP ${res.status}`);
      bytes = await res.arrayBuffer();
      previewBytes.set(hit.previewUrl, bytes);
    }
    const file = new File([bytes], `${hit.title}.m4a`, { type: "audio/mp4" });
    const path = await registerWebFile(file);
    await setQueue([
      {
        path,
        title: `${hit.title} (preview)`,
        artist: hit.artist,
        replaygain_track_db: null,
        replaygain_album_db: null,
      },
    ]);
    await playQueueIndex(0);
    setToast(`Preview: ${hit.title} — ${hit.artist}`);
    return true;
  } catch (e) {
    setToast(`Preview failed: ${String(e)}`);
    return false;
  }
}

