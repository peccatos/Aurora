/** Local playlists (YTM parity): IndexedDB-persisted lists of track
 *  ids with names. No server — a playlist is a JSON document in the
 *  kv store. Track bytes/metadata live in the tracks store; a playlist
 *  only references ids (small, honest, survives reloads).
 */

import { kvGet, kvPut } from "./library-db";

export interface Playlist {
  id: string;
  name: string;
  /** Track ids (library-db tracks store keys), play order. */
  trackIds: string[];
  createdAt: number;
}

const KEY = "aw:playlists";

let cache: Playlist[] | null = null;

async function load(): Promise<Playlist[]> {
  if (cache) return cache;
  try {
    cache = (await kvGet<Playlist[]>(KEY)) ?? [];
  } catch {
    cache = [];
  }
  return cache;
}

async function save(list: Playlist[]): Promise<void> {
  cache = list;
  try {
    await kvPut(KEY, list);
  } catch {
    /* best-effort; a failed save must not lose the in-session list */
  }
}

export async function listPlaylists(): Promise<Playlist[]> {
  return load();
}

export async function getPlaylist(id: string): Promise<Playlist | null> {
  return (await load()).find((p) => p.id === id) ?? null;
}

export async function createPlaylist(name: string): Promise<Playlist> {
  const list = await load();
  const playlist: Playlist = {
    id: crypto.randomUUID(),
    name: name.trim() || "New playlist",
    trackIds: [],
    createdAt: Date.now(),
  };
  await save([playlist, ...list]);
  return playlist;
}

export async function renamePlaylist(id: string, name: string): Promise<void> {
  const list = await load();
  const p = list.find((x) => x.id === id);
  if (p && name.trim()) {
    p.name = name.trim();
    await save(list);
  }
}

export async function deletePlaylist(id: string): Promise<void> {
  await save((await load()).filter((p) => p.id !== id));
}

/** Append tracks (dedup by id: a playlist never repeats an entry). */
export async function addToPlaylist(id: string, trackIds: string[]): Promise<void> {
  const list = await load();
  const p = list.find((x) => x.id === id);
  if (!p) return;
  for (const t of trackIds) {
    if (!p.trackIds.includes(t)) p.trackIds.push(t);
  }
  await save(list);
}

export async function removeFromPlaylist(id: string, trackId: string): Promise<void> {
  const list = await load();
  const p = list.find((x) => x.id === id);
  if (!p) return;
  p.trackIds = p.trackIds.filter((t) => t !== trackId);
  await save(list);
}

/** Drag-reorder: move a track entry to a new index. */
export async function reorderPlaylist(
  id: string,
  trackId: string,
  toIndex: number,
): Promise<void> {
  const list = await load();
  const p = list.find((x) => x.id === id);
  if (!p) return;
  const from = p.trackIds.indexOf(trackId);
  if (from < 0 || toIndex < 0 || toIndex >= p.trackIds.length) return;
  p.trackIds.splice(from, 1);
  p.trackIds.splice(toIndex, 0, trackId);
  await save(list);
}
