/** Import quality metric: aggregates one remote-playlist match run into
 *  a compact report and persists a short per-import history so matching
 *  quality can be tracked across real imports (fixture tests prove the
 *  rules; this answers "what % of a real playlist actually matched").
 *
 *  Pure aggregation lives here so remote-playlist-import can call it
 *  without changing any public signature.
 */

import type {
  MatchRemoteTracksResult,
  RemoteMatchKind,
} from "./remote-playlist-import.ts";

export interface UnmatchedTrackSummary {
  title: string;
  artists: string;
}

export interface ImportQualityReport {
  totalRemote: number;
  matched: number;
  /** Matched source positions per match kind ("exact" | "artist" |
   *  "title_only"). Kinds sum to `matched` and never overlap. */
  matchedByKind: Record<RemoteMatchKind, number>;
  unmatched: number;
  unmatchedTracks: UnmatchedTrackSummary[];
  /** matched / totalRemote; 0 when totalRemote is 0. */
  matchRate: number;
}

/** The storage ceiling for the unmatched list — a 500-track import must
 *  not dump 500 rows into a report or localStorage. */
export const UNMATCHED_CAP = 50;
export const HISTORY_KEY = "aurora.import-quality.v1";
export const HISTORY_CAP = 20;

export interface ImportQualityHistoryEntry {
  date: number;
  provider: string;
  playlistName: string;
  report: ImportQualityReport;
}

type MatchInput = Pick<MatchRemoteTracksResult, "matched" | "unmatched">;

function labelArtists(artists: readonly string[]): string {
  return artists.join(", ");
}

/** Aggregates a match run (or the entries of a PlaylistImportReport,
 *  which carry the same fields) into a quality report. Pure: no storage,
 *  no clock. */
export function summarizeImportQuality(
  matches: MatchInput,
  totalRemoteTracks?: number,
): ImportQualityReport {
  const matchedByKind: Record<RemoteMatchKind, number> = {
    exact: 0,
    artist: 0,
    title_only: 0,
  };
  for (const entry of matches.matched) {
    if (entry.matchKind in matchedByKind) matchedByKind[entry.matchKind] += 1;
  }
  const totalRemote =
    totalRemoteTracks ?? matches.matched.length + matches.unmatched.length;
  return {
    totalRemote,
    matched: matches.matched.length,
    matchedByKind,
    unmatched: matches.unmatched.length,
    unmatchedTracks: matches.unmatched.slice(0, UNMATCHED_CAP).map((entry) => ({
      title: entry.remote.title,
      artists: labelArtists(entry.remote.artists),
    })),
    matchRate: totalRemote > 0 ? matches.matched.length / totalRemote : 0,
  };
}

/** Percent label for the UI ("83%" rather than 0.833…). */
export function matchRatePercent(report: ImportQualityReport): string {
  return `${Math.round(report.matchRate * 100)}%`;
}

function storage(): Storage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function sanitizeReport(value: unknown): ImportQualityReport | null {
  if (!isRecord(value)) return null;
  const kinds = { exact: 0, artist: 0, title_only: 0 };
  const rawKinds = isRecord(value.matchedByKind) ? value.matchedByKind : {};
  for (const kind of Object.keys(kinds) as RemoteMatchKind[]) {
    const count = rawKinds[kind];
    if (typeof count === "number" && Number.isFinite(count) && count >= 0) {
      kinds[kind] = count;
    }
  }
  const num = (key: string): number => {
    const raw = value[key];
    return typeof raw === "number" && Number.isFinite(raw) && raw >= 0 ? raw : 0;
  };
  const tracks = Array.isArray(value.unmatchedTracks)
    ? value.unmatchedTracks.filter(
        (item): item is UnmatchedTrackSummary =>
          isRecord(item) &&
          typeof item.title === "string" &&
          typeof item.artists === "string",
      )
    : [];
  return {
    totalRemote: num("totalRemote"),
    matched: num("matched"),
    matchedByKind: kinds,
    unmatched: num("unmatched"),
    unmatchedTracks: tracks.slice(0, UNMATCHED_CAP),
    matchRate: num("totalRemote") > 0 ? num("matched") / num("totalRemote") : 0,
  };
}

/** Reads the import-quality history, newest first. Malformed entries and
 *  malformed storage JSON are discarded (accounts.ts pattern). */
export function readImportHistory(): ImportQualityHistoryEntry[] {
  const raw = storage()?.getItem(HISTORY_KEY);
  if (!raw) return [];
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return [];
  }
  if (!Array.isArray(parsed)) return [];
  const entries: ImportQualityHistoryEntry[] = [];
  for (const item of parsed) {
    if (!isRecord(item)) continue;
    const report = sanitizeReport(item.report);
    if (!report) continue;
    if (typeof item.date !== "number" || !Number.isFinite(item.date)) continue;
    entries.push({
      date: item.date,
      provider: typeof item.provider === "string" ? item.provider : "",
      playlistName: typeof item.playlistName === "string" ? item.playlistName : "",
      report,
    });
  }
  return entries.slice(0, HISTORY_CAP);
}

/** Prepends one entry (newest first) and trims to HISTORY_CAP. Pure
 *  apart from the capped list; storage write is done by saveHistory. */
export function appendImportHistory(
  history: readonly ImportQualityHistoryEntry[],
  entry: ImportQualityHistoryEntry,
): ImportQualityHistoryEntry[] {
  return [entry, ...history].slice(0, HISTORY_CAP);
}

function saveHistory(history: readonly ImportQualityHistoryEntry[]): void {
  storage()?.setItem(HISTORY_KEY, JSON.stringify(history));
}

/** Records one import into persistent history. Returns the stored entry
 *  (date stamped) so the caller can display it without a re-read. */
export function recordImportQuality(
  input: MatchInput,
  meta: { provider: string; playlistName: string; date?: number },
): ImportQualityHistoryEntry {
  const entry: ImportQualityHistoryEntry = {
    date: meta.date ?? Date.now(),
    provider: meta.provider,
    playlistName: meta.playlistName,
    report: summarizeImportQuality(input, input.matched.length + input.unmatched.length),
  };
  saveHistory(appendImportHistory(readImportHistory(), entry));
  return entry;
}

export function clearImportHistory(): void {
  storage()?.removeItem(HISTORY_KEY);
}
