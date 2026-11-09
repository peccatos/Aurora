/** Line-by-line lyrics translation via a LibreTranslate-compatible
 *  endpoint (default: translate.disroot.org).
 *
 *  Consent model: OFF by default. The user opts in on the player card /
 *  settings — the note says the text goes to an external service. The
 *  endpoint is configurable via localStorage 'aurora.translation.endpoint'
 *  (full URL including /translate) + 'aurora.translation.enabled'.
 *
 *  Honesty: results are persisted through saveTrackEnrichment
 *  (lyrics_translation + language) so the work is done once; the in-memory
 *  cache is per-session only.
 */

export const DEFAULT_TRANSLATION_ENDPOINT = "https://translate.disroot.org/translate";
export const TRANSLATION_ENDPOINT_KEY = "aurora.translation.endpoint";
export const TRANSLATION_ENABLED_KEY = "aurora.translation.enabled";

const MAX_LINES_PER_REQUEST = 20;
const MAX_BYTES_PER_REQUEST = 10 * 1024;

/** Picker languages (LibreTranslate-common, ISO 639-1). */
export const TRANSLATION_LANGUAGES: ReadonlyArray<{ code: string; label: string }> = [
  { code: "ru", label: "Русский" },
  { code: "en", label: "English" },
  { code: "uk", label: "Українська" },
  { code: "de", label: "Deutsch" },
  { code: "fr", label: "Français" },
  { code: "es", label: "Español" },
  { code: "tr", label: "Türkçe" },
  { code: "pl", label: "Polski" },
  { code: "pt", label: "Português" },
  { code: "it", label: "Italiano" },
  { code: "ja", label: "日本語" },
  { code: "zh", label: "中文" },
];

export function translationEnabled(): boolean {
  try {
    return localStorage.getItem(TRANSLATION_ENABLED_KEY) === "true";
  } catch {
    return false;
  }
}

export function setTranslationEnabled(enabled: boolean): void {
  try {
    localStorage.setItem(TRANSLATION_ENABLED_KEY, enabled ? "true" : "false");
  } catch {
    /* storage unavailable — the call sites already handle off-by-default */
  }
}

export function translationEndpoint(): string {
  try {
    const stored = localStorage.getItem(TRANSLATION_ENDPOINT_KEY)?.trim();
    if (stored) return stored;
  } catch {
    /* fall through to default */
  }
  return DEFAULT_TRANSLATION_ENDPOINT;
}

export function setTranslationEndpoint(endpoint: string): void {
  try {
    localStorage.setItem(TRANSLATION_ENDPOINT_KEY, endpoint.trim());
  } catch {
    /* storage unavailable */
  }
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

export interface TranslateOptions {
  /** Injectable for tests (defaults to globalThis.fetch). */
  fetchFn?: typeof fetch;
  /** Override the configured endpoint (defaults to translationEndpoint()). */
  endpoint?: string;
}

/** Session cache: source-lines+target → translated lines. */
const cache = new Map<string, string[]>();

function cacheKey(lines: readonly string[], target: string): string {
  // Cheap stable hash (FNV-1a) of the joined text + target.
  const input = `${lines.join("\u0000")}\u0001${target}`;
  let hash = 0x811c9dc5;
  for (let i = 0; i < input.length; i++) {
    hash ^= input.charCodeAt(i);
    hash = Math.imul(hash, 0x01000193) >>> 0;
  }
  return `tr:${hash.toString(16)}:${input.length}`;
}

/** Batch lines: at most MAX_LINES_PER_REQUEST and MAX_BYTES_PER_REQUEST
 *  of UTF-8 payload per request. Returns [batches, placeholder output]. */
function batchLines(lines: readonly string[]): [string[][], string[]] {
  const batches: string[][] = [];
  const out: string[] = [];
  let current: string[] = [];
  let currentBytes = 0;
  const byteLen = (s: string) => new TextEncoder().encode(s).length;
  for (const line of lines) {
    const b = byteLen(line);
    if (current.length > 0 && (current.length >= MAX_LINES_PER_REQUEST || currentBytes + b > MAX_BYTES_PER_REQUEST)) {
      batches.push(current);
      current = [];
      currentBytes = 0;
    }
    current.push(line);
    currentBytes += b;
  }
  if (current.length) batches.push(current);
  for (const line of lines) out.push("");
  return [batches, out];
}

interface LibreTranslateResponse {
  translatedText?: string | string[];
}

/** One POST; throws on network/HTTP/shape failure. */
async function translateBatch(
  lines: readonly string[],
  target: string,
  endpoint: string,
  fetchFn: typeof fetch,
): Promise<string[]> {
  const body = JSON.stringify({ q: lines, source: "auto", target, format: "text" });
  const res = await fetchFn(endpoint, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body,
  });
  if (!res.ok) throw new Error(`Translation service returned HTTP ${res.status}`);
  const data = (await res.json()) as LibreTranslateResponse;
  const text = data.translatedText;
  if (Array.isArray(text)) {
    if (text.length !== lines.length) throw new Error("Translation service returned a mismatched line count");
    return text;
  }
  if (typeof text !== "string") throw new Error("Translation service returned no text");
  // Some endpoints collapse q-array input into one string — unacceptable
  // for per-line mapping: reject rather than misalign the lyrics.
  throw new Error("Translation service did not return per-line results");
}

/** Translate lyrics line-by-line. Batches (≤20 lines, ≤10KB payload),
 *  retries once on network failure, session-caches the result. */
export async function translateLyrics(
  lines: string[],
  targetLang: string,
  opts: TranslateOptions = {},
): Promise<string[]> {
  if (!lines.length) return [];
  if (!targetLang.trim()) throw new Error("Target language is required");
  const key = cacheKey(lines, targetLang);
  const cached = cache.get(key);
  if (cached) return cached.slice();

  const fetchFn = opts.fetchFn ?? globalThis.fetch;
  const endpoint = opts.endpoint ?? translationEndpoint();
  const [batches, out] = batchLines(lines);
  let offset = 0;
  for (const batch of batches) {
    let translated: string[] | null = null;
    let lastError: unknown = null;
    for (let attempt = 0; attempt < 2; attempt++) {
      try {
        translated = await translateBatch(batch, targetLang, endpoint, fetchFn);
        break;
      } catch (error) {
        lastError = error;
      }
    }
    if (!translated) {
      throw lastError instanceof Error ? lastError : new Error("Translation request failed");
    }
    for (let i = 0; i < translated.length; i++) out[offset + i] = translated[i];
    offset += batch.length;
  }
  cache.set(key, out);
  return out.slice();
}

/** Split raw lyrics text (plain or LRC) into translatable lines: LRC
 *  timestamps are stripped so the text alone is sent; translation joins
 *  back with the original structure (same line count). */
export function lyricsToTranslateLines(text: string): string[] {
  return text
    .split(/\r?\n/)
    .map((line) => line.replace(/\[[^\]]*\]/g, "").trim())
    .map((line) => line || "♪");
}
