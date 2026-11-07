/** Local playlists (YTM parity): IndexedDB-persisted lists of track
 *  ids with names. No server — a playlist is a JSON document in the
 *  kv store. Track bytes/metadata live in the tracks store; a playlist
 *  only references ids (small, honest, survives reloads).
 */

import { kvGet, kvPut } from "./library-db.ts";

export interface Playlist {
  id: string;
  name: string;
  /** Track ids (library-db tracks store keys), play order. */
  trackIds: string[];
  createdAt: number;
  source?: {
    provider: "spotify" | "youtube-music";
    playlistId: string;
    revision: string | null;
    importedAt: number;
    lastSyncedAt: number | null;
  };
}

const KEY = "aw:playlists";

/** Change listeners: the Home "Your playlists" shelf re-renders itself
 *  when playlists change from ANY route (playlists page, ⋯ menu). */
type PlaylistsListener = () => void;
const listeners = new Set<PlaylistsListener>();

/** Subscribe to playlist mutations. Returns an unsubscribe function. */
export function onPlaylistsChanged(fn: PlaylistsListener): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

function notifyChanged(): void {
  for (const fn of listeners) fn();
}

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
  const snapshot = list.map((playlist) => ({ ...playlist, trackIds: [...playlist.trackIds], source: playlist.source ? { ...playlist.source } : undefined }));
  await kvPut(KEY, snapshot);
  cache = snapshot;
  notifyChanged();
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
    await save(list.map((item) => item.id === id ? { ...item, name: name.trim() } : item));
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
  const additions = trackIds.filter((trackId) => !p.trackIds.includes(trackId));
  await save(list.map((item) => item.id === id ? { ...item, trackIds: [...item.trackIds, ...additions] } : item));
}

export async function removeFromPlaylist(id: string, trackId: string): Promise<void> {
  const list = await load();
  const p = list.find((x) => x.id === id);
  if (!p) return;
  await save(list.map((item) => item.id === id ? { ...item, trackIds: item.trackIds.filter((t) => t !== trackId) } : item));
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
  if (from < 0 || toIndex < 0 || toIndex > p.trackIds.length) return;
  const next = [...p.trackIds];
  next.splice(from, 1);
  const target = Math.min(toIndex, next.length);
  next.splice(target, 0, trackId);
  await save(list.map((item) => item.id === id ? { ...item, trackIds: next } : item));
}
