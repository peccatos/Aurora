const VIDEO_HOSTS = new Set(["youtube.com", "www.youtube.com", "youtu.be", "vimeo.com", "www.vimeo.com"]);
export function validateVideoUrl(value: string): string | null {
  const raw = value.trim();
  if (!raw) return null;
  let url: URL;
  try { url = new URL(raw); } catch { return "Enter a complete HTTPS video URL"; }
  if (url.protocol !== "https:") return "Video URL must use HTTPS";
  if (url.username || url.password) return "Credentials are not allowed in video URLs";
  const host = url.hostname.toLowerCase();
  if (!VIDEO_HOSTS.has(host)) return "Video host is not supported";
  return null;
}
