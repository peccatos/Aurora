/** Free & legal full-track catalog: the Internet Archive audio
 *  collections (netlabels, etree live music, audio_music) filtered to
 *  Creative-Commons licenses — full songs, key-less, CORS-open (all
 *  verified: /download sends Access-Control-Allow-Origin: * and honor
 *  Range requests).
 *
 *  Honesty (план §6.2/6.4): these are LEGAL full tracks (CC licenses +
 *  public domain live recordings shared with permission). Commercial
 *  catalogs (iTunes/Deezer) remain preview-only — that is their
 *  license, and Aurora does not pretend otherwise.
 */

export interface FreeTrackHit {
  /** archive.org item identifier. */
  identifier: string;
  title: string;
  creator: string | null;
  /** Direct playable mp3 file name inside the item. */
  file: string;
  /** Seconds, when known. */
  durationS: number | null;
  license: string | null;
  itemUrl: string;
}

/** Structured provenance retained with every imported remote track. */
export interface TrackProvenance {
  provider: "archive.org";
  identifier: string;
  itemUrl: string;
  downloadUrl: string;
  licenseUrl: string;
  creator: string | null;
  importedAt: string;
}

const MAX_IMPORT_BYTES = 512 * 1024 * 1024;

/** Only the archive download origin and item path are importable. */
export function validateArchiveDownloadUrl(value: string, identifier?: string): string | null {
  let url: URL;
  try { url = new URL(value); } catch { return "Audio source URL is invalid"; }
  if (url.protocol !== "https:") return "Audio source must use HTTPS";
  if (url.username || url.password || url.port || url.hostname !== "archive.org") {
    return "Audio source host is not allowed";
  }
  const match = url.pathname.match(/^\/download\/([^/]+)\/(.+)$/);
  if (!match || match[2].includes("/")) {
    return "Audio source path is not an archive download";
  }
  try {
    // URL.pathname preserves percent escapes, so reject encoded path separators
    // before validating the apparently single-file download path.
    const decodedFile = decodeURIComponent(match[2]);
    if (decodedFile.includes("/") || decodedFile.includes("\\")) {
      return "Audio source path is not an archive download";
    }
  } catch {
    return "Audio source path is not an archive download";
  }
  if (identifier) {
    try {
      if (decodeURIComponent(match[1]) !== identifier) return "Audio source path is not an archive download";
    } catch {
      return "Audio source path is not an archive download";
    }
  }
  if (url.search || url.hash) return "Audio source must not contain query or fragment data";
  return null;
}

export function isCreativeCommonsLicense(value: string | null): boolean {
  if (!value) return false;
  try {
    const url = new URL(value);
    return (url.protocol === "https:" || url.protocol === "http:") &&
      (url.hostname === "creativecommons.org" || url.hostname === "www.creativecommons.org");
  } catch {
    return false;
  }
}

export const maxFreeImportBytes = MAX_IMPORT_BYTES;

const SEARCH = "https://archive.org/advancedsearch.php";
const FL = ["identifier", "title", "creator", "downloads", "licenseurl"];

interface IaDoc {
  identifier: string;
  title?: string | string[];
  creator?: string | string[];
  licenseurl?: string | string[];
}

function first(v: string | string[] | undefined): string | null {
  if (Array.isArray(v)) return v[0] ?? null;
  return v ?? null;
}

/** Search CC-licensed audio items. `query` matches title/creator.
 *  The format filter demands DERIVED MP3s — items whose uploader only
 *  shipped a ZIP have no playable files (a real discovery flaw found
 *  in live testing: hit counts were full of unplayable entries). */
export async function searchFreeTracks(query: string, limit = 12): Promise<FreeTrackHit[]> {
  const q = [
    "mediatype:audio",
    "(collection:netlabels OR collection:audio_music OR collection:etree)",
    "licenseurl:(*creativecommons*)",
    'format:("VBR MP3")',
    `(${query.split(/\s+/).filter(Boolean).map((w) => `"${w}"`).join(" AND ")})`,
  ].join(" AND ");
  const params = new URLSearchParams();
  params.set("q", q);
  for (const f of FL) params.append("fl[]", f);
  params.set("rows", String(limit));
  params.set("output", "json");
  const ctrl = new AbortController();
  const timer = setTimeout(() => ctrl.abort(), 7000);
  try {
    const res = await fetch(`${SEARCH}?${params}`, { signal: ctrl.signal });
    if (!res.ok) return [];
    const data = (await res.json()) as { response?: { docs?: IaDoc[] } };
    const docs = data.response?.docs ?? [];
    const hits: FreeTrackHit[] = [];
    for (const doc of docs) {
      // Resolve the playable file from the item metadata (async, but
      // we return the hits first and let the caller resolve lazily).
      hits.push({
        identifier: doc.identifier,
        title: first(doc.title) ?? doc.identifier,
        creator: first(doc.creator),
        file: "", // resolved by resolveFreeTrack when played
        durationS: null,
        license: first(doc.licenseurl),
        itemUrl: `https://archive.org/details/${doc.identifier}`,
      });
    }
    return hits;
  } catch {
    return []; // offline / blocked: honest empty
  } finally {
    clearTimeout(timer);
  }
}

/** Resolve the playable MP3 file of an item (first derived VBR/MP3
 *  track). Returns the direct download URL. */
export async function resolveFreeTrack(hit: FreeTrackHit): Promise<string | null> {
  if (hit.file) return downloadUrl(hit.identifier, hit.file);
  try {
    const ctrl = new AbortController();
    const timer = setTimeout(() => ctrl.abort(), 7000);
    try {
      const res = await fetch(`https://archive.org/metadata/${hit.identifier}`, {
        signal: ctrl.signal,
      });
      if (!res.ok) return null;
      const meta = (await res.json()) as {
        files?: Array<{ name: string; format: string; length?: string }>;
      };
      const mp3 =
        (meta.files ?? []).find((f) => f.format === "VBR MP3" || f.format === "128Kbps MP3") ??
        (meta.files ?? []).find((f) => (f.format || "").includes("MP3"));
      if (!mp3) return null;
      hit.file = mp3.name;
      // Length like "372.21" (seconds).
      if (mp3.length && !hit.durationS) {
        const s = Number.parseFloat(mp3.length);
        if (Number.isFinite(s)) hit.durationS = Math.round(s);
      }
      return downloadUrl(hit.identifier, mp3.name);
    } finally {
      clearTimeout(timer);
    }
  } catch {
    return null;
  }
}

function downloadUrl(identifier: string, file: string): string {
  return `https://archive.org/download/${identifier}/${encodeURIComponent(file)}`;
}

/** MPEG audio can begin with an ID3 tag or an MPEG frame sync. */
export function hasMp3Signature(bytes: Uint8Array): boolean {
  if (bytes.length >= 3 && bytes[0] === 0x49 && bytes[1] === 0x44 && bytes[2] === 0x33) return true;
  for (let i = 0; i + 1 < Math.min(bytes.length, 4096); i += 1) {
    if (bytes[i] === 0xff && (bytes[i + 1] & 0xe0) === 0xe0) return true;
  }
  return false;
}

/** Fetch a bounded, non-empty CC audio payload without accepting redirects. */
export async function fetchFreeTrackBytes(hit: FreeTrackHit, url: string): Promise<Uint8Array> {
  const validation = validateArchiveDownloadUrl(url, hit.identifier);
  if (validation) throw new Error(validation);
  if (!isCreativeCommonsLicense(hit.license)) throw new Error("Creative Commons license attribution is missing");
  const ctrl = new AbortController();
  const timer = setTimeout(() => ctrl.abort(), 30_000);
  try {
    const res = await fetch(url, { signal: ctrl.signal, redirect: "error" });
    if (!res.ok) throw new Error(`Audio source returned HTTP ${res.status}`);
    const contentType = (res.headers.get("content-type") ?? "").split(";", 1)[0].trim().toLowerCase();
    if (contentType && !["audio/mpeg", "audio/mp3", "application/octet-stream"].includes(contentType)) {
      throw new Error(`Audio source returned unexpected content type: ${contentType}`);
    }
    const lengthHeader = res.headers.get("content-length");
    const length = lengthHeader === null ? null : Number(lengthHeader);
    if (length !== null && (!Number.isSafeInteger(length) || length < 0)) throw new Error("Audio source has an invalid size");
    if (length !== null && length > MAX_IMPORT_BYTES) throw new Error("Audio source is larger than 512 MB");
    const reader = res.body?.getReader();
    if (!reader) throw new Error("Audio source returned no readable body");
    const chunks: Uint8Array[] = [];
    let total = 0;
    try {
      while (true) {
        const { done, value } = await reader.read();
        if (done) break;
        total += value.byteLength;
        if (total > MAX_IMPORT_BYTES) {
          await reader.cancel();
          throw new Error("Audio source is larger than 512 MB");
        }
        chunks.push(value);
      }
    } finally {
      reader.releaseLock();
    }
    if (!total) throw new Error("Audio source is empty");
    if (length !== null && length !== total) throw new Error("Audio source size did not match its content-length");
    const bytes = new Uint8Array(total);
    let offset = 0;
    for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
    if (!hasMp3Signature(bytes)) throw new Error("Audio source is not a valid MP3 stream");
    return bytes;
  } finally {
    clearTimeout(timer);
  }
}

/** Attribution line (CC-BY honesty): "Creator — Title (license link)". */
export function attribution(hit: FreeTrackHit): string {
  const lic = hit.license?.includes("/by-nc") ? "CC BY-NC" : hit.license?.includes("/by-sa") ? "CC BY-SA" : hit.license?.includes("/by/") ? "CC BY" : hit.license ? "CC" : "archive.org";
  return `${hit.creator ?? "Unknown"} — ${hit.title} · ${lic}`;
}
