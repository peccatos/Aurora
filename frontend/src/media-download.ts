import { csrfToken } from "./web-provider-adapter.ts";
import { invoke, platform } from "./invoke.ts";

export interface MediaMetadata {
  title: string;
  uploader: string | null;
  durationSeconds: number | null;
  webpageUrl: string;
  thumbnailUrl: string | null;
}

function validateAllowedUrl(raw: string): URL {
  const url = new URL(raw);
  const allowedHosts = new Set(["archive.org", "www.archive.org", "youtube.com", "www.youtube.com", "youtu.be"]);
  if (url.protocol !== "https:" || !allowedHosts.has(url.hostname.toLowerCase()) || url.username || url.password) {
    throw new Error("URL is not an allowed HTTPS media source");
  }
  return url;
}

function normalizeMetadata(raw: unknown): MediaMetadata {
  if (typeof raw !== "object" || raw === null || Array.isArray(raw)) throw new Error("Invalid metadata response");
  const value = raw as Record<string, unknown>;
  // The Tauri command returns yt-dlp's raw JSON (snake_case); the browser API
  // returns its sanitized DTO (camelCase). Both shapes are accepted, each
  // field validated client-side — backend sanitization is never trusted.
  const title = typeof value.title === "string" ? value.title : null;
  const webpageRaw = typeof value.webpage_url === "string" ? value.webpage_url : typeof value.webpageUrl === "string" ? value.webpageUrl : null;
  const uploaderRaw = typeof value.uploader === "string" ? value.uploader : typeof value.uploader === "string" ? value.uploader : null;
  const durationRaw = typeof value.duration === "number" && Number.isFinite(value.duration) ? value.duration : typeof value.durationSeconds === "number" && Number.isFinite(value.durationSeconds) ? value.durationSeconds : null;
  const thumbnailRaw = typeof value.thumbnail === "string" ? value.thumbnail : typeof value.thumbnailUrl === "string" ? value.thumbnailUrl : null;
  if (title === null || webpageRaw === null) throw new Error("Invalid metadata response");
  const webpage = validateAllowedUrl(webpageRaw);
  const thumbnail = thumbnailRaw !== null ? validateAllowedUrl(thumbnailRaw).href : null;
  return {
    title: title.slice(0, 500),
    uploader: uploaderRaw !== null ? uploaderRaw.slice(0, 300) : null,
    durationSeconds: durationRaw,
    webpageUrl: webpage.href,
    thumbnailUrl: thumbnail,
  };
}

/** Test seam: the module reads its base URL from `import.meta.env`, which the
 *  node test runner does not provide; the hook overrides it (null restores). */
export function setMediaApiBaseForTests(base: string | null): void {
  testApiBase = base;
}
let testApiBase: string | null = null;

export async function getMediaMetadata(rawUrl: string): Promise<MediaMetadata> {
  const url = validateAllowedUrl(rawUrl).href;
  if (platform === "tauri") {
    const result = await invoke<{ json: string }>("yt_dlp_metadata", { url });
    return normalizeMetadata(JSON.parse(result.json) as unknown);
  }
  const base = testApiBase ?? (import.meta.env?.VITE_AURORA_MEDIA_BASE_URL?.trim() || "https://aurora-api-3.onrender.com");
  if (!base) throw new Error("Media API is not configured in this browser deployment");
  const endpoint = new URL("/api/media/metadata", base);
  if (endpoint.protocol !== "https:") throw new Error("Media API must use HTTPS");
  const token = csrfToken();
  if (!token) throw new Error("No session CSRF token available for the media API");
  // State-changing request: the session cookie alone is not enough, the
  // session-bound CSRF token must be echoed in the header.
  const response = await fetch(endpoint, {
    method: "POST",
    credentials: "include",
    headers: {
      "Content-Type": "application/json",
      Accept: "application/json",
      "X-CSRF-Token": token,
    },
    body: JSON.stringify({ url }),
  });
  if (!response.ok) throw new Error(`Media API request failed (${response.status})`);
  return normalizeMetadata(await response.json());
}

export async function downloadMedia(rawUrl: string, outputDir?: string): Promise<string> {
  const url = validateAllowedUrl(rawUrl).href;
  if (platform !== "tauri") {
    throw new Error("Browser downloads require the configured media download API");
  }
  if (!outputDir) throw new Error("Select a download directory first");
  return invoke<string>("yt_dlp_download", { url, outputDir });
}
