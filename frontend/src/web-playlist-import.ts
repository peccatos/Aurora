import type { StoredTrack } from "./library-db";
import type { Playlist } from "./playlist-service";

export interface PlaylistImportEntry {
  title: string;
  artist: string | null;
  location: string | null;
}

export interface WebPlaylistImportReport {
  playlist: Playlist;
  matched: number;
  unmatched: string[];
  /** Number of parsed entries in the file (matched + unmatched + skipped
   *  duplicate entries). */
  totalEntries: number;
}

function clean(value: string): string {
  return value
    .normalize("NFKC")
    .toLocaleLowerCase()
    .replace(/\.[a-z0-9]{2,5}$/, "")
    .replace(/[‐-―]/g, "-")
    .replace(/[^\p{L}\p{N}]+/gu, " ")
    .trim();
}

function stem(path: string): string {
  const leaf = path.split(/[\\/]/).pop() ?? path;
  return leaf.replace(/\.[^.]+$/, "");
}

function fromFilename(value: string): { title: string; artist: string | null } {
  const name = stem(value).trim();
  const split = name.indexOf(" - ");
  return split > 0
    ? { artist: name.slice(0, split).trim(), title: name.slice(split + 3).trim() }
    : { artist: null, title: name };
}

export function parseM3u(text: string): PlaylistImportEntry[] {
  const out: PlaylistImportEntry[] = [];
  let pending: { title: string; artist: string | null } | null = null;
  for (const raw of text.replace(/^\uFEFF/, "").split(/\r?\n/)) {
    const line = raw.trim();
    if (!line || line.startsWith("#EXT-X-") || line === "#EXTM3U") continue;
    if (line.startsWith("#EXTINF:")) {
      const comma = line.indexOf(",");
      if (comma < 0) continue;
      const label = line.slice(comma + 1).trim();
      const parsed = fromFilename(label);
      pending = { title: parsed.title, artist: parsed.artist };
      continue;
    }
    if (line.startsWith("#")) continue;
    const parsed = pending ?? fromFilename(line);
    out.push({ title: parsed.title, artist: parsed.artist, location: line });
    pending = null;
  }
  return out;
}

export function parsePls(text: string): PlaylistImportEntry[] {
  const rows = new Map<number, { title?: string; file?: string }>();
  for (const raw of text.split(/\r?\n/)) {
    const match = /^\s*(File|Title)(\d+)\s*=\s*(.*)\s*$/i.exec(raw);
    if (!match) continue;
    const index = Number(match[2]);
    const row = rows.get(index) ?? {};
    if (match[1].toLowerCase() === "file") row.file = match[3];
    else row.title = match[3];
    rows.set(index, row);
  }
  return [...rows.entries()]
    .sort(([a], [b]) => a - b)
    .filter(([, row]) => row.file)
    .map(([, row]) => {
      const parsed = fromFilename(row.title || row.file!);
      return { title: parsed.title, artist: parsed.artist, location: row.file! };
    });
}

function parseCsvLine(line: string): string[] {
  const cells: string[] = [];
  let cell = "";
  let quoted = false;
  for (let i = 0; i < line.length; i++) {
    const char = line[i];
    if (char === '"' && line[i + 1] === '"' && quoted) {
      cell += '"';
      i++;
    } else if (char === '"') quoted = !quoted;
    else if (char === "," && !quoted) {
      cells.push(cell.trim());
      cell = "";
    } else cell += char;
  }
  cells.push(cell.trim());
  return cells;
}

function csvRecords(text: string): string[] {
  const records: string[] = [];
  let record = "";
  let quoted = false;
  for (const line of text.replace(/^\uFEFF/, "").split(/\r?\n/)) {
    if (record) record += "\n";
    record += line;
    for (let i = 0; i < line.length; i++) {
      if (line[i] === '"' && line[i + 1] === '"' && quoted) i++;
      else if (line[i] === '"') quoted = !quoted;
    }
    if (!quoted) {
      if (record.trim()) records.push(record);
      record = "";
    }
  }
  if (record.trim()) records.push(record);
  return records;
}

function parseCsv(text: string): PlaylistImportEntry[] {
  const records = csvRecords(text);
  if (!records.length) return [];
  const header = parseCsvLine(records[0]).map(clean);
  const titleIndex = header.findIndex((h) => /^(track )?name$|^title$/.test(h));
  const artistIndex = header.findIndex((h) => /^(artist|artists|artist name)$/.test(h));
  const locationIndex = header.findIndex((h) => /^(path|file|uri|url|location)$/.test(h));
  const hasHeader = titleIndex >= 0 || artistIndex >= 0;
  const start = hasHeader ? 1 : 0;
  return records.slice(start).flatMap((record) => {
    const cells = parseCsvLine(record);
    const title = hasHeader ? cells[titleIndex >= 0 ? titleIndex : 0] : cells[0];
    if (!title?.trim()) return [];
    const fallback = fromFilename(title);
    return [{
      title: fallback.title,
      artist: hasHeader && artistIndex >= 0 ? cells[artistIndex] || null : fallback.artist,
      location: hasHeader && locationIndex >= 0 ? cells[locationIndex] || null : null,
    }];
  });
}

function parseJson(text: string): PlaylistImportEntry[] {
  const value: unknown = JSON.parse(text);
  const raw: unknown[] = Array.isArray(value)
    ? value
    : value && typeof value === "object" && Array.isArray((value as { items?: unknown }).items)
      ? (value as { items: unknown[] }).items
      : [];
  return raw.flatMap((item) => {
    if (!item || typeof item !== "object") return [];
    const obj = item as Record<string, unknown>;
    const track = obj.track && typeof obj.track === "object" ? obj.track as Record<string, unknown> : obj;
    const title = typeof track.name === "string" ? track.name : typeof track.title === "string" ? track.title : "";
    if (!title) return [];
    const artists = track.artists;
    const artist = Array.isArray(artists)
      ? artists.map((a) => typeof a === "string" ? a : a && typeof a === "object" && typeof (a as { name?: unknown }).name === "string" ? (a as { name: string }).name : "").filter(Boolean).join(", ")
      : typeof track.artist === "string" ? track.artist : null;
    return [{ title, artist: artist || null, location: typeof track.uri === "string" ? track.uri : typeof track.url === "string" ? track.url : null }];
  });
}

export function parsePlaylistFile(name: string, text: string): PlaylistImportEntry[] {
  const ext = name.toLowerCase().split(".").pop() ?? "";
  if (ext === "m3u" || ext === "m3u8" || ext === "txt") return parseM3u(text);
  if (ext === "pls") return parsePls(text);
  if (ext === "csv") return parseCsv(text);
  if (ext === "json") return parseJson(text);
  throw new Error("Unsupported playlist format");
}

function matchEntry(entry: PlaylistImportEntry, tracks: StoredTrack[]): StoredTrack | null {
  const title = clean(entry.title);
  const artist = clean(entry.artist ?? "");
  if (!title) return null;
  const exact = tracks.find((track) => clean(track.name) === title && artist && clean(track.artist ?? "") === artist);
  if (exact) return exact;
  return tracks.find((track) => clean(track.name) === title) ?? null;
}

export async function importWebPlaylist(file: File, requestedName?: string): Promise<WebPlaylistImportReport> {
  const [{ getAllTracks }, { addToPlaylist, createPlaylist }] = await Promise.all([
    import("./library-db"),
    import("./playlist-service"),
  ]);
  const entries = parsePlaylistFile(file.name, await file.text());
  const tracks = await getAllTracks();
  const ids: string[] = [];
  const unmatched: string[] = [];
  for (const entry of entries) {
    const match = matchEntry(entry, tracks);
    if (match) {
      if (!ids.includes(match.id)) ids.push(match.id);
    } else {
      unmatched.push(entry.artist ? `${entry.artist} — ${entry.title}` : entry.title);
    }
  }
  const fallbackName = file.name.replace(/\.[^.]+$/, "").trim() || "Imported playlist";
  const playlist = await createPlaylist(requestedName?.trim() || fallbackName);
  await addToPlaylist(playlist.id, ids);
  return { playlist: { ...playlist, trackIds: ids }, matched: ids.length, unmatched, totalEntries: entries.length };
}
