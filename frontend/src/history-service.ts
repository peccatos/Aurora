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
