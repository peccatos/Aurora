const VIDEO_HOSTS = new Set(["youtube.com", "www.youtube.com", "youtu.be", "vimeo.com", "www.vimeo.com"]);
const MAX_VIDEO_URL_LENGTH = 500;

function hasYoutubeVideoId(url: URL, host: string): boolean {
  if (host === "youtu.be") return url.pathname.split("/").filter(Boolean).length === 1;
  const path = url.pathname.replace(/\/+$/, "");
  if (path === "/watch") return Boolean(url.searchParams.get("v")?.trim());
  return /^\/(?:shorts|embed|live)\/[^/]+$/.test(path);
}

function hasVimeoVideoId(url: URL): boolean {
  return /^\/\d+(?:\/[^/]*)?$/.test(url.pathname.replace(/\/+$/, ""));
}

export function validateVideoUrl(value: string): string | null {
  const raw = value.trim();
  if (!raw) return null;
  if (raw.length > MAX_VIDEO_URL_LENGTH) return "Video URL must be 500 characters or fewer";
  let url: URL;
  try { url = new URL(raw); } catch { return "Enter a complete HTTPS video URL"; }
  if (url.protocol !== "https:") return "Video URL must use HTTPS";
  if (url.username || url.password) return "Credentials are not allowed in video URLs";
  const host = url.hostname.toLowerCase();
  if (!VIDEO_HOSTS.has(host)) return "Video host is not supported";
  const validVideoPath = host === "vimeo.com" || host === "www.vimeo.com"
    ? hasVimeoVideoId(url)
    : hasYoutubeVideoId(url, host);
  if (!validVideoPath) return "Enter a direct video URL";
  return null;
}
