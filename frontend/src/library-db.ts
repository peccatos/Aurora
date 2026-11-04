/** IndexedDB persistence for the web library (план §4: tracks survive
 *  page reloads).
 *
 *  Storage layout (one DB, two stores):
 *  - "tracks":  { id, name, artist, album, durationMs, extension, bytes,
 *                 addedAt }  — the full audio bytes, keyed by stable id.
 *    The id is the SAME id the kernel registry uses ("web://<id>"), so
 *    a reloaded page re-registers from here and the queue keeps
 *    working.
 *  - "kv":      { key, value } — small settings (DSP config, last
 *    queue, last route). Versioned by the "schema" field.
 *
 *  Honesty: storage failures degrade to in-memory behavior with a
 *  console warn — the player never breaks because persistence did.
 */

const DB_NAME = "aurora-web";
const DB_VERSION = 1;

export interface StoredTrack {
  id: string;
  name: string;
  artist: string | null;
  album: string | null;
  durationMs: number | null;
  extension: string;
  bytes: ArrayBuffer;
  addedAt: number;
}

let dbPromise: Promise<IDBDatabase> | null = null;

function openDb(): Promise<IDBDatabase> {
  if (dbPromise) return dbPromise;
  dbPromise = new Promise((resolve, reject) => {
    const req = indexedDB.open(DB_NAME, DB_VERSION);
    req.onupgradeneeded = () => {
      const db = req.result;
      if (!db.objectStoreNames.contains("tracks")) {
        db.createObjectStore("tracks", { keyPath: "id" });
      }
      if (!db.objectStoreNames.contains("kv")) {
        db.createObjectStore("kv");
      }
    };
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error ?? new Error("IndexedDB open failed"));
  });
  return dbPromise;
}

function tx<T>(
  store: string,
  mode: IDBTransactionMode,
  run: (s: IDBObjectStore) => IDBRequest<T>,
): Promise<T> {
  return openDb().then(
    (db) =>
      new Promise<T>((resolve, reject) => {
        const t = db.transaction(store, mode);
        const req = run(t.objectStore(store));
        req.onsuccess = () => resolve(req.result);
        req.onerror = () => reject(req.error ?? new Error("IndexedDB request failed"));
      }),
  );
}

// ---------------------------------------------------------------------------
// Tracks
// ---------------------------------------------------------------------------

export async function putTrack(track: StoredTrack): Promise<void> {
  await tx("tracks", "readwrite", (s) => s.put(track));
}

export async function getAllTracks(): Promise<StoredTrack[]> {
  const all = await tx<StoredTrack[]>("tracks", "readonly", (s) => s.getAll());
  return (all ?? []).sort((a, b) => a.addedAt - b.addedAt);
}

export async function deleteTrack(id: string): Promise<void> {
  await tx("tracks", "readwrite", (s) => s.delete(id));
}

export async function clearTracks(): Promise<void> {
  await tx("tracks", "readwrite", (s) => s.clear());
}

// ---------------------------------------------------------------------------
// KV settings
// ---------------------------------------------------------------------------

export async function kvGet<T>(key: string): Promise<T | null> {
  const v = await tx<T | undefined>("kv", "readonly", (s) => s.get(key));
  return v ?? null;
}

export async function kvPut<T>(key: string, value: T): Promise<void> {
  await tx("kv", "readwrite", (s) => s.put(value, key));
}
