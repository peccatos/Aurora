import { invoke, platform } from "./invoke";

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
  if (typeof value.title !== "string" || typeof value.webpage_url !== "string") throw new Error("Invalid metadata response");
  const webpage = validateAllowedUrl(value.webpage_url);
  let thumbnail: string | null = null;
  if (typeof value.thumbnail === "string") thumbnail = validateAllowedUrl(value.thumbnail).href;
  return {
    title: value.title.slice(0, 500),
    uploader: typeof value.uploader === "string" ? value.uploader.slice(0, 300) : null,
    durationSeconds: typeof value.duration === "number" && Number.isFinite(value.duration) ? value.duration : null,
    webpageUrl: webpage.href,
    thumbnailUrl: thumbnail,
  };
}

export async function getMediaMetadata(rawUrl: string): Promise<MediaMetadata> {
  const url = validateAllowedUrl(rawUrl).href;
  if (platform === "tauri") {
    const result = await invoke<{ json: string }>("yt_dlp_metadata", { url });
    return normalizeMetadata(JSON.parse(result.json) as unknown);
  }
  const base = import.meta.env?.VITE_MEDIA_API_BASE_URL?.trim();
  if (!base) throw new Error("Media API is not configured in this browser deployment");
  const endpoint = new URL("/api/media/metadata", base);
  if (endpoint.protocol !== "https:") throw new Error("Media API must use HTTPS");
  const response = await fetch(endpoint, {
    method: "POST",
    credentials: "include",
    headers: { "Content-Type": "application/json", Accept: "application/json" },
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
