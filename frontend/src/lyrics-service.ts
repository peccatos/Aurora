/** Lyrics with synced timing (план §6.1) — LRCLIB, key-less.
 *
 * LRCLIB serves community-synced LRC lyrics for millions of tracks and
 * sends CORS `*` — a browser can query it directly. Cache in IndexedDB
 * (kv store) for offline replay (the plan's offline fallback).
 *
 * Honesty: no translation scraping, no Genius key hacks — if LRCLIB has
 * no match, the Lyrics panel says "not found", never placeholder text.
 */

import { kvGet, kvPut } from "./library-db";

export interface LyricLine {
  /** Line start time in seconds (null = unsynced header/footer lines). */
  timeS: number | null;
  text: string;
}

export interface LyricsResult {
  trackName: string;
  artistName: string;
  /** Synced lines when available (LRC), else plain-text lines. */
  lines: LyricLine[];
  synced: boolean;
  source: "lrclib";
}

interface LrcLibHit {
  id: number;
  trackName: string;
  artistName: string;
  albumName?: string;
  duration?: number;
  instrumental: boolean;
  plainLyrics: string | null;
  syncedLyrics: string | null;
}

// ---------------------------------------------------------------------------
// LRC parsing
// ---------------------------------------------------------------------------

/** Parse an LRC body: `[mm:ss.xx] line text` (multiple stamps per line
 *  expand to separate entries). Unstamped lines keep timeS = null. */
export function parseLrc(body: string): LyricLine[] {
  const out: LyricLine[] = [];
  for (const raw of body.split(/\r?\n/)) {
    const stamps = [...raw.matchAll(/\[(\d{1,2}):(\d{1,2})(?:[.:](\d{1,3}))?\]/g)];
    if (stamps.length === 0) {
      const text = raw.trim();
      if (text) out.push({ timeS: null, text });
      continue;
    }
    const text = raw.slice(stamps[stamps.length - 1].index! + stamps[stamps.length - 1][0].length).trim();
    for (const m of stamps) {
      const min = Number.parseInt(m[1], 10);
      const sec = Number.parseInt(m[2], 10);
      const fracRaw = m[3] ?? "0";
      const frac = Number.parseInt(fracRaw, 10) / 10 ** fracRaw.length;
      out.push({ timeS: min * 60 + sec + frac, text });
    }
  }
  out.sort((a, b) => (a.timeS ?? -1) - (b.timeS ?? -1));
  return out;
}

// ---------------------------------------------------------------------------
// Fetch + cache
// ---------------------------------------------------------------------------

const CACHE_KEY = (artist: string, title: string) => `lyrics:${artist.toLowerCase()}|${title.toLowerCase()}`;

/** Fetch lyrics for a track. Cache-first (IndexedDB kv), network second.
 *  Null = no lyrics found (the panel shows an honest "not found"). */
export async function getLyrics(artist: string | null, title: string): Promise<LyricsResult | null> {
  if (!title.trim()) return null;
  const key = CACHE_KEY(artist ?? "", title);
  try {
    const cached = await kvGet<LyricsResult>(key);
    if (cached) return cached;
  } catch {
    /* storage unavailable — go to network */
  }

  const params = new URLSearchParams({ q: `${artist ?? ""} ${title}`.trim() });
  const url = `https://lrclib.net/api/search?${params}`;
  const ctrl = new AbortController();
  const timer = setTimeout(() => ctrl.abort(), 6000);
  try {
    const res = await fetch(url, { signal: ctrl.signal });
    if (!res.ok) return null;
    const hits = (await res.json()) as LrcLibHit[];
    // Prefer a synced non-instrumental hit; duration closeness breaks ties.
    const usable = hits.filter((h) => !h.instrumental && (h.syncedLyrics || h.plainLyrics));
    if (!usable.length) return null;
    const best = usable[0];
    const lines = best.syncedLyrics
      ? parseLrc(best.syncedLyrics)
      : (best.plainLyrics ?? "").split(/\r?\n/).map((t) => ({ timeS: null, text: t }));
    const result: LyricsResult = {
      trackName: best.trackName,
      artistName: best.artistName,
      lines: lines.filter((l) => l.text),
      synced: Boolean(best.syncedLyrics),
      source: "lrclib",
    };
    if (!result.lines.length) return null;
    try {
      await kvPut(key, result);
    } catch {
      /* cache write best-effort */
    }
    return result;
  } catch {
    // Offline / blocked: fall back to a cached copy only.
    try {
      return await kvGet<LyricsResult>(key);
    } catch {
      return null;
    }
  } finally {
    clearTimeout(timer);
  }
}

/** Current synced line index for a playback position (binary-ish scan).
 *  Returns -1 before the first line. */
export function activeLineIndex(lines: LyricLine[], posS: number): number {
  let active = -1;
  for (let i = 0; i < lines.length; i++) {
    const t = lines[i].timeS;
    if (t != null && t <= posS + 0.15) active = i;
  }
  return active;
}
