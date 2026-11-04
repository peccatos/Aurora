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

/** Search CC-licensed audio items. `query` matches title/creator. */
export async function searchFreeTracks(query: string, limit = 12): Promise<FreeTrackHit[]> {
  const q = [
    "mediatype:audio",
    "(collection:netlabels OR collection:audio_music OR collection:etree)",
    "licenseurl:(*creativecommons*)",
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

/** Attribution line (CC-BY honesty): "Creator — Title (license link)". */
export function attribution(hit: FreeTrackHit): string {
  const lic = hit.license?.includes("/by-nc") ? "CC BY-NC" : hit.license?.includes("/by-sa") ? "CC BY-SA" : hit.license?.includes("/by/") ? "CC BY" : hit.license ? "CC" : "archive.org";
  return `${hit.creator ?? "Unknown"} — ${hit.title} · ${lic}`;
}
