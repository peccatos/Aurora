/** Server library (owner's own server): the owner adds full tracks and
 *  video files to their server; Aurora streams them legally (they are
 *  the owner's files).
 *
 * CONTRACT (documented for the server side — a static JSON file is
 * enough, no backend code required):
 *   GET {base}/library.json ->
 *   {
 *     "tracks": [
 *       { "url": "https://…/song.mp3", "title": "…", "artist": "…",
 *         "durationMs": 213000, "license": "…" }
 *     ]
 *   }
 * Any audio URL the server lists plays through the kernel (the wasm
 * decoder handles mp3/flac/ogg/wav; the browser fetches the bytes —
 * the server must send CORS headers, e.g.
 * Access-Control-Allow-Origin: *).
 *
 * Configuration: base URL persisted in IndexedDB kv ("aw:server-url"),
 * editable in Settings. Empty = feature hidden (honest: nothing to
 * show without the owner's server).
 */

import { kvGet, kvPut } from "./library-db";

export interface ServerTrack {
  url: string;
  title: string;
  artist: string | null;
  album?: string | null;
  durationMs?: number | null;
  /** e.g. "CC BY" / "own recording" — displayed in the attribution. */
  license?: string | null;
}

let cache: ServerTrack[] | null = null;

export async function getServerUrl(): Promise<string> {
  try {
    return (await kvGet<string>("aw:server-url")) ?? "";
  } catch {
    return "";
  }
}

export async function setServerUrl(url: string): Promise<void> {
  cache = null; // force reload
  await kvPut("aw:server-url", url.trim().replace(/\/+$/, ""));
}

export async function getServerTracks(): Promise<ServerTrack[]> {
  if (cache) return cache;
  const base = await getServerUrl();
  if (!base) return [];
  try {
    const ctrl = new AbortController();
    const timer = setTimeout(() => ctrl.abort(), 8000);
    try {
      const res = await fetch(`${base}/library.json`, { signal: ctrl.signal });
      if (!res.ok) return [];
      const data = (await res.json()) as { tracks?: ServerTrack[] };
      cache = (data.tracks ?? []).filter((t) => typeof t.url === "string");
      return cache;
    } finally {
      clearTimeout(timer);
    }
  } catch {
    return []; // offline / misconfigured: honest empty
  }
}
