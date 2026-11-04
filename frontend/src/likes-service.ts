/** Likes / dislikes (spec 3.2): per-track sentiment, persisted in the
 *  IndexedDB kv store ("aw:likes").
 *
 *  Value model mirrors YTM: each track id maps to "like" | "dislike";
 *  absent = no rating. Setting null clears the rating. ids are the
 *  library-track ids (the "web://" path prefix is stripped by callers).
 */

import { kvGet, kvPut } from "./library-db";

export type LikeValue = "like" | "dislike";

const KEY = "aw:likes";

type LikeMap = Record<string, LikeValue>;

let cache: LikeMap | null = null;

async function load(): Promise<LikeMap> {
  if (cache) return cache;
  try {
    cache = (await kvGet<LikeMap>(KEY)) ?? {};
  } catch {
    cache = {};
  }
  return cache;
}

/** Current rating for a track, or null if unrated. */
export async function getLike(id: string): Promise<LikeValue | null> {
  const map = await load();
  return map[id] ?? null;
}

/** Set ("like" | "dislike") or clear (null) a track's rating. */
export async function setLike(id: string, val: LikeValue | null): Promise<void> {
  const map = await load();
  if (val === null) delete map[id];
  else map[id] = val;
  cache = map;
  try {
    await kvPut(KEY, map);
  } catch {
    /* best-effort: likes are non-critical */
  }
}

/** All ids with a "like" rating (order not guaranteed). */
export async function likedIds(): Promise<string[]> {
  const map = await load();
  return Object.entries(map)
    .filter(([, v]) => v === "like")
    .map(([id]) => id);
}
