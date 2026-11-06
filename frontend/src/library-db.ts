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
const DB_VERSION = 3;

export interface StoredTrack {
  id: string;
  name: string;
  artist: string | null;
  album: string | null;
  genre?: string | null;
  year?: number | null;
  durationMs: number | null;
  extension: string;
  bytes: ArrayBuffer;
  addedAt: number;
  /** R128 integrated loudness (LUFS), measured at ingest (W8). */
  lufs?: number | null;
  lyrics?: string | null;
  lyrics_language?: string | null;
  lyrics_translation?: string | null;
  lyrics_translation_language?: string | null;
  artwork_data_url?: string | null;
  source_url?: string | null;
  source_provider?: string | null;
  video_url?: string | null;
  metadata_source?: string | null;
  /** JSON-serializable origin and license record for remote imports. */
  provenance?: {
    provider: string;
    identifier: string;
    itemUrl: string;
    downloadUrl: string;
    licenseUrl: string;
    creator: string | null;
    importedAt: string;
  } | null;
  source_file_name?: string | null;
  source_file_size?: number | null;
  source_file_last_modified?: number | null;
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
      // v2: embedded cover art extracted from tags at ingest.
      if (!db.objectStoreNames.contains("artwork")) {
        db.createObjectStore("artwork", { keyPath: "trackId" });
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

export async function getTrack(id: string): Promise<StoredTrack | null> {
  return (await tx<StoredTrack | undefined>("tracks", "readonly", (s) => s.get(id))) ?? null;
}

export async function updateTrackEnrichment(
  id: string,
  enrichment: Partial<Pick<StoredTrack,
    "lyrics" | "lyrics_language" | "lyrics_translation" |
    "lyrics_translation_language" | "artwork_data_url" | "source_url" |
    "source_provider" | "video_url" | "metadata_source" | "source_file_name" | "source_file_size" | "source_file_last_modified" | "name" | "artist" |
    "album" | "genre" | "year"
  >>,
): Promise<void> {
  const db = await openDb();
  await new Promise<void>((resolve, reject) => {
    const transaction = db.transaction("tracks", "readwrite");
    const store = transaction.objectStore("tracks");
    const request = store.get(id);
    request.onsuccess = () => {
      const track = request.result as StoredTrack | undefined;
      if (!track) {
        transaction.abort();
        reject(new Error("Track not found in web library"));
        return;
      }
      store.put({ ...track, ...enrichment });
    };
    request.onerror = () => reject(request.error ?? new Error("Track read failed"));
    transaction.oncomplete = () => resolve();
    transaction.onerror = () => reject(transaction.error ?? new Error("Track update failed"));
    transaction.onabort = () => reject(transaction.error ?? new Error("Track update aborted"));
  });
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
// Artwork (v2): embedded cover art blobs keyed by track id
// ----------------------------------------------------------------------------

/** Stored artwork record: one Blob per track ("trackId" is the keyPath). */
export interface StoredArtwork {
  trackId: string;
  blob: Blob;
}

export async function getArtwork(trackId: string): Promise<Blob | null> {
  const rec = await tx<StoredArtwork | undefined>("artwork", "readonly", (s) =>
    s.get(trackId),
  );
  return rec?.blob ?? null;
}

export async function putArtwork(trackId: string, blob: Blob): Promise<void> {
  await tx("artwork", "readwrite", (s) => s.put({ trackId, blob }));
}

/** Persist user-selected artwork in both web track representations atomically. */
export async function updateWebTrackArtwork(
  trackId: string,
  blob: Blob,
  dataUrl: string,
): Promise<void> {
  const db = await openDb();
  await new Promise<void>((resolve, reject) => {
    const transaction = db.transaction(["tracks", "artwork"], "readwrite");
    const tracks = transaction.objectStore("tracks");
    const request = tracks.get(trackId);
    request.onsuccess = () => {
      const track = request.result as StoredTrack | undefined;
      if (!track) {
        transaction.abort();
        reject(new Error("Track not found in web library"));
        return;
      }
      tracks.put({ ...track, artwork_data_url: dataUrl });
      transaction.objectStore("artwork").put({ trackId, blob });
    };
    request.onerror = () => reject(request.error ?? new Error("Track read failed"));
    transaction.oncomplete = () => resolve();
    transaction.onerror = () => reject(transaction.error ?? new Error("Artwork update failed"));
    transaction.onabort = () => reject(transaction.error ?? new Error("Artwork update aborted"));
  });
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
