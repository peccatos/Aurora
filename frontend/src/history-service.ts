/** Play history (план §6.5 graft): "Продолжить слушать" shelf data.
 *
 * A local, honest log of what actually played: track id + title +
 * artist + last-played timestamp, persisted in the IndexedDB kv store.
 * The Home page renders the most recent distinct tracks as a shelf.
 */

import { kvGet, kvPut } from "./library-db";

export interface HistoryEntry {
  id: string;
  title: string;
  artist: string | null;
  playedAt: number;
  /** Resume position in seconds at the moment the play was logged
   *  (spec 4.3 "continue listening" progress bar). Optional — an
   *  entry without it renders no bar (honest, not zero). */
  positionS?: number;
}

const KEY = "aw:play-history";
const MAX_ENTRIES = 50;

let cache: HistoryEntry[] | null = null;

async function load(): Promise<HistoryEntry[]> {
  if (cache) return cache;
  try {
    cache = (await kvGet<HistoryEntry[]>(KEY)) ?? [];
  } catch {
    cache = [];
  }
  return cache;
}

/** Record a play: moves the track to the front (dedup by id). */
export async function recordPlay(entry: Omit<HistoryEntry, "playedAt">): Promise<void> {
  const list = await load();
  const next = [
    { ...entry, playedAt: Date.now() },
    ...list.filter((e) => e.id !== entry.id),
  ].slice(0, MAX_ENTRIES);
  cache = next;
  try {
    await kvPut(KEY, next);
  } catch {
    /* best-effort: history is non-critical */
  }
}

/** The "Continue listening" shelf: most recent distinct tracks. */
export async function recentPlays(count = 8): Promise<HistoryEntry[]> {
  const list = await load();
  return list.slice(0, count);
}

/** Wipe the whole history (History page "Clear all"). The cache and the
 *  kv record drop to empty together — after this, load() must not
 *  resurrect old entries from a stale cache. */
export async function clearHistory(): Promise<void> {
  cache = [];
  try {
    await kvPut(KEY, []);
  } catch {
    /* best-effort: the in-memory copy is already gone */
  }
}
