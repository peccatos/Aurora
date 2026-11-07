import type { StoredTrack } from "./library-db";
import type { Playlist } from "./playlist-service";

/** Provider-neutral remote playlist contracts.
 *
 *  Both adapters (browser OAuth backend, native Tauri runtime) produce
 *  these shapes, so matching, reporting and local creation live here
 *  once instead of being duplicated per platform.
 */

export type RemoteProvider = "spotify" | "youtube-music";

export interface RemotePlaylistSummary {
  provider: RemoteProvider;
  id: string;
  name: string;
  trackCount: number | null;
  revision: string | null;
}

export interface RemotePlaylistTrack {
  remoteId: string;
  title: string;
  artists: string[];
  album: string | null;
  durationMs: number | null;
  /** Index within the source playlist; the local copy preserves it. */
  position: number;
}

export interface RemotePlaylistSnapshot extends RemotePlaylistSummary {
  tracks: RemotePlaylistTrack[];
}

export type RemoteMatchKind = "exact" | "artist" | "title_only";

export interface PlaylistImportMatch {
  remote: RemotePlaylistTrack;
  /** Position of this track in the source playlist. */
  sourcePosition: number;
  localTrackId: string;
  matchKind: RemoteMatchKind;
  /** True when this source position resolves to a local track already
   *  claimed by an earlier source position. */
  duplicate: boolean;
}

export interface PlaylistImportUnmatched {
  remote: RemotePlaylistTrack;
  sourcePosition: number;
}

export interface AmbiguousRemoteMatch {
  remote: RemotePlaylistTrack;
  sourcePosition: number;
  candidates: StoredTrack[];
}

/** Counters are per source position, so `matched + unmatched ===
 *  totalRemoteTracks`. `ambiguous` and `duplicateMatches` are overlapping
 *  subsets of `matched`, not separate buckets: an entry can be both
 *  ambiguous and a duplicate. Counting by source position (rather than by
 *  unique local id) keeps every provider entry accounted for exactly
 *  once — a playlist that repeats one song five times reports five
 *  matched positions, one of them a duplicate. */
export interface RemoteMatchCounters {
  totalRemoteTracks: number;
  matched: number;
  unmatched: number;
  ambiguous: number;
  duplicateMatches: number;
  /** Distinct local tracks the matched entries point at — the number of
   *  entries a local playlist actually receives. */
  uniqueLocalTracks: number;
}

export function summarizeMatchCounters(
  result: Pick<MatchRemoteTracksResult, "matched" | "unmatched" | "ambiguous">,
  totalRemoteTracks: number,
): RemoteMatchCounters {
  return {
    totalRemoteTracks,
    matched: result.matched.length,
    unmatched: result.unmatched.length,
    ambiguous: result.ambiguous.length,
    duplicateMatches: result.matched.filter((entry) => entry.duplicate).length,
    uniqueLocalTracks: new Set(result.matched.map((entry) => entry.localTrackId)).size,
  };
}

export interface MatchRemoteTracksResult {
  matched: PlaylistImportMatch[];
  unmatched: PlaylistImportUnmatched[];
  ambiguous: AmbiguousRemoteMatch[];
}

export interface PlaylistImportReport extends RemoteMatchCounters {
  playlist: Playlist;
  sourceProvider: RemoteProvider;
  sourcePlaylistId: string;
  sourceRevision: string | null;
  /** Alias of `totalRemoteTracks`, kept for the report header wording. */
  totalRemotePositions: number;
  matchedEntries: PlaylistImportMatch[];
  unmatchedEntries: PlaylistImportUnmatched[];
  ambiguousEntries: AmbiguousRemoteMatch[];
  /** Local track ids in source order, de-duplicated. */
  localTrackIds: string[];
  /** "Artist — Title" labels for the unmatched source positions. */
  unmatchedLabels: string[];
}

/** NFKC + lower-case + punctuation-folded key. Bracketed suffixes
 *  ("(Remastered 2011)", "[Live]", "(feat. X)") are dropped so provider
 *  annotations do not defeat an otherwise exact title match. `separators`
 *  controls whether list punctuation (`,`, `&`, `feat.`, …) survives:
 *  title keys drop it, so "Song, Live" and "Song Live" agree, while
 *  artist credit keys keep it, because "A, B" is a distinct credit from
 *  "A B". */
function normalizeKeyPart(value: string, keepSeparators: boolean): string {
  const folded = value
    .normalize("NFKC")
    .toLocaleLowerCase()
    .replace(/\([^()]*\)|\[[^\][]*\]/g, " ")
    .replace(/[‐-―]/g, "-");
  return folded
    .replace(keepSeparators ? /[^\p{L}\p{N}&,;\/]+/gu : /[^\p{L}\p{N}]+/gu, " ")
    .replace(/\s+/g, " ")
    .trim();
}

export function normalizeMatchKey(value: string): string {
  return normalizeKeyPart(value, false);
}

/** Credit key for one artist, or for a multi-artist credit joined by a
 *  separator. `splitArtistCredits` is reused so a local "A, B" credit and
 *  the remote ["A","B"] pair share one key. */
function normalizeArtistKey(value: string): string {
  return normalizeKeyPart(value, true);
}

function compareCandidates(a: StoredTrack, b: StoredTrack): number {
  if (a.id !== b.id) return a.id < b.id ? -1 : 1;
  return 0;
}

/** Resolves a title/artist query against an already-built index. Exported
 *  so the matching rules can be exercised without a full snapshot. */
export function resolveRemoteTrackCandidates(
  query: { title: string; artists: readonly string[] },
  index: LocalTrackIndex,
): { kind: RemoteMatchKind | null; candidates: readonly StoredTrack[] } {
  const title = normalizeMatchKey(query.title);
  if (!title) return { kind: null, candidates: [] };
  for (const artist of query.artists) {
    const key = normalizeArtistKey(artist);
    if (!key) continue;
    const candidates = index.byTitleArtist.get(`${title}\u0000${key}`);
    if (candidates?.length) return { kind: "exact", candidates };
  }
  // A multi-artist remote credit may only match a local track in exactly
  // the same order — never a subset — so a shorter credit cannot steal a
  // track that belongs to a fuller one.
  if (query.artists.length > 1) {
    const joined = normalizeArtistKey(query.artists.join(", "));
    const candidates = joined ? index.byTitleAccepted.get(`${title}\u0000${joined}`) : undefined;
    if (candidates?.length) return { kind: "exact", candidates };
  }
  for (const artist of query.artists) {
    const key = normalizeArtistKey(artist);
    if (!key) continue;
    const candidates = index.byTitleAccepted.get(`${title}\u0000${key}`);
    if (candidates?.length) return { kind: "artist", candidates };
  }
  const candidates = index.byTitle.get(title);
  if (candidates?.length) return { kind: "title_only", candidates };
  return { kind: null, candidates: [] };
}

export interface LocalTrackIndex {
  byTitle: ReadonlyMap<string, readonly StoredTrack[]>;
  byTitleArtist: ReadonlyMap<string, readonly StoredTrack[]>;
  /** Title key + the whole local artist credit (every separator kept, in
   *  order) -> the tracks whose credit reads exactly that way. Lets a
   *  remote ["A","B"] pair meet a local "A, B" credit exactly, without
   *  also meeting the local "A, B, C" credit. */
  byTitleAccepted: ReadonlyMap<string, readonly StoredTrack[]>;
}

export function buildLocalTrackIndex(localTracks: readonly StoredTrack[]): LocalTrackIndex {
  const byTitle = new Map<string, StoredTrack[]>();
  const byTitleArtist = new Map<string, StoredTrack[]>();
  const byTitleAccepted = new Map<string, StoredTrack[]>();
  for (const track of localTracks) {
    const title = normalizeMatchKey(track.name ?? "");
    if (!title) continue;
    push(byTitle, title, track);
    const artist = normalizeMatchKey(track.artist ?? "");
    if (artist) push(byTitleArtist, `${title}\u0000${artist}`, track);
    const credit = normalizeArtistKey(track.artist ?? "");
    if (credit) push(byTitleAccepted, `${title}\u0000${credit}`, track);
  }
  for (const list of byTitle.values()) list.sort(compareCandidates);
  for (const list of byTitleArtist.values()) list.sort(compareCandidates);
  for (const list of byTitleAccepted.values()) list.sort(compareCandidates);
  return { byTitle, byTitleArtist, byTitleAccepted };
}

function push<K, V>(map: Map<K, V[]>, key: K, value: V): void {
  const list = map.get(key);
  if (list) list.push(value);
  else map.set(key, [value]);
}

/** Deterministic winner, applied in source-position order:
 *  1. a candidate this playlist has not claimed yet (so a repeated remote
 *     entry consumes the *next* local copy instead of re-adding one, and
 *     two different remote entries never collapse onto one local track
 *     when an unused copy exists);
 *  2. lowest local track id, to break ties identically on every run. */
function positionWinner(
  winners: readonly StoredTrack[],
  claimed: ReadonlySet<string>,
): StoredTrack | null {
  let winner: StoredTrack | null = null;
  for (const candidate of winners) {
    if (claimed.has(candidate.id)) continue;
    if (!winner || compareCandidates(candidate, winner) < 0) winner = candidate;
  }
  if (winner) return winner;
  for (const candidate of winners) {
    if (!winner || compareCandidates(candidate, winner) < 0) winner = candidate;
  }
  return winner;
}

export function matchRemoteTracks(
  snapshot: RemotePlaylistSnapshot,
  localTracks: readonly StoredTrack[],
): MatchRemoteTracksResult;
export function matchRemoteTracks(
  tracks: readonly RemotePlaylistTrack[],
  localTracks: readonly StoredTrack[],
): MatchRemoteTracksResult;
export function matchRemoteTracks(
  source: RemotePlaylistSnapshot | readonly RemotePlaylistTrack[],
  localTracks: readonly StoredTrack[],
): MatchRemoteTracksResult {
  const tracks = Array.isArray(source) ? source : (source as RemotePlaylistSnapshot).tracks;
  const index = buildLocalTrackIndex(localTracks);
  const matched: PlaylistImportMatch[] = [];
  const unmatched: PlaylistImportUnmatched[] = [];
  const ambiguous: AmbiguousRemoteMatch[] = [];
  const claimed = new Set<string>();

  // Source order is the playlist's order — sort by position, never by
  // provider response order.
  const ordered = tracks
    .map((remote, fallback) => ({
      remote,
      sourcePosition: Number.isInteger(remote.position) ? remote.position : fallback,
    }))
    .sort((a, b) => a.sourcePosition - b.sourcePosition);

  for (const { remote, sourcePosition } of ordered) {
    const { kind, candidates } = resolveRemoteTrackCandidates(remote, index);
    if (!kind || !candidates.length) {
      unmatched.push({ remote, sourcePosition });
      continue;
    }
    const winner = positionWinner(candidates, claimed);
    if (!winner) {
      unmatched.push({ remote, sourcePosition });
      continue;
    }
    // Ambiguity is reported, never silently guessed away: several
    // equally good local tracks, none of them claimed yet, so the
    // playlist alone cannot say which copy belongs here. The entry still
    // resolves — to the deterministic winner — so the import is
    // idempotent and `ambiguous` stays a report rather than a hole.
    if (candidates.length > 1 && candidates.every((item) => !claimed.has(item.id))) {
      ambiguous.push({ remote, sourcePosition, candidates: [...candidates] });
    }
    const duplicate = claimed.has(winner.id);
    claimed.add(winner.id);
    matched.push({
      remote,
      sourcePosition,
      localTrackId: winner.id,
      matchKind: kind,
      duplicate,
    });
  }
  return { matched, unmatched, ambiguous };
}

/** Reads the local library for matching. Storage is best-effort by
 *  design (library-db treats persistence as degradable): when IndexedDB
 *  is unavailable the import still runs and reports every position as
 *  unmatched, instead of failing the whole operation. */
async function readLocalTracks(
  getAllTracks: () => Promise<readonly StoredTrack[]>,
): Promise<readonly StoredTrack[]> {
  try {
    return await getAllTracks();
  } catch {
    return [];
  }
}

export function importRemotePlaylist(
  snapshot: RemotePlaylistSnapshot,
  options: {
    name?: string;
    localTracks?: readonly StoredTrack[];
    createPlaylist?: (name: string) => Promise<Playlist>;
    addToPlaylist?: (playlistId: string, trackIds: string[]) => Promise<void>;
  } = {},
): Promise<PlaylistImportReport> {
  return applyRemotePlaylistImport(snapshot, options);
}

/** Creates the local playlist and fills it in source order. Matching is
 *  pure and synchronous; only creation/persisting is async. */
export async function applyRemotePlaylistImport(
  snapshot: RemotePlaylistSnapshot,
  options: {
    name?: string;
    localTracks?: readonly StoredTrack[];
    createPlaylist?: (name: string) => Promise<Playlist>;
    addToPlaylist?: (playlistId: string, trackIds: string[]) => Promise<void>;
  } = {},
): Promise<PlaylistImportReport> {
  const [{ getAllTracks }, { createPlaylist, addToPlaylist }] = await Promise.all([
    import("./library-db.ts"),
    import("./playlist-service.ts"),
  ]);
  const result = matchRemoteTracks(
    snapshot,
    options.localTracks ?? (await readLocalTracks(getAllTracks)),
  );
  // Preserve source order, keep one entry per local track.
  const localTrackIds: string[] = [];
  for (const entry of result.matched) {
    if (!localTrackIds.includes(entry.localTrackId)) localTrackIds.push(entry.localTrackId);
  }
  const create = options.createPlaylist ?? createPlaylist;
  const add = options.addToPlaylist ?? addToPlaylist;
  const name = options.name?.trim() || snapshot.name?.trim() || "Imported playlist";
  const playlist = await create(name);
  await add(playlist.id, localTrackIds);
  const counters = summarizeMatchCounters(result, snapshot.tracks.length);
  return {
    ...counters,
    totalRemotePositions: counters.totalRemoteTracks,
    playlist: {
      ...playlist,
      name: playlist.name?.trim() ? playlist.name : name,
      trackIds: localTrackIds,
      source: {
        provider: snapshot.provider,
        playlistId: snapshot.id,
        revision: snapshot.revision,
        importedAt: Date.now(),
        lastSyncedAt: null,
      },
    },
    sourceProvider: snapshot.provider,
    sourcePlaylistId: snapshot.id,
    sourceRevision: snapshot.revision,
    matchedEntries: result.matched,
    unmatchedEntries: result.unmatched,
    ambiguousEntries: result.ambiguous,
    localTrackIds,
    unmatchedLabels: result.unmatched.map((entry) =>
      entry.remote.artists.length
        ? `${entry.remote.artists.join(", ")} — ${entry.remote.title}`
        : entry.remote.title,
    ),
  };
}

/** End-to-end browser path: pick a provider, list its playlists, fetch
 *  one snapshot, then import it into the local library. */
export interface RemoteImportFlow {
  providers: readonly RemoteProvider[];
  list(provider: RemoteProvider): Promise<{
    items: RemotePlaylistSummary[];
    nextCursor: string | null;
  }>;
  snapshot(provider: RemoteProvider, playlistId: string): Promise<RemotePlaylistSnapshot>;
}

export function remoteImportFlow(): Promise<RemoteImportFlow> {
  return import("./web-provider-adapter.ts").then((adapter) => ({
    providers: ["spotify", "youtube-music"] as const,
    list: (provider) => adapter.listRemotePlaylists(provider),
    snapshot: (provider, playlistId) => adapter.getRemotePlaylist(provider, playlistId),
  }));
}
