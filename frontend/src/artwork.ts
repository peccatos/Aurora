/** Embedded cover art: data-URL cache (path -> url | null). */

import { invoke, platform } from "./invoke";
import { trackArtworkUrl } from "./artwork-store";
import { getWebTrackByPath } from "./pages/home";

const cache = new Map<string, string | null>();

export function invalidateArtwork(path: string): void {
  cache.delete(path);
}

export async function getArtwork(path: string): Promise<string | null> {
  if (cache.has(path)) return cache.get(path) ?? null;
  try {
    if (platform === "web" && path.startsWith("web://")) {
      const webTrack = getWebTrackByPath(path);
      if (webTrack?.artwork_data_url) {
        cache.set(path, webTrack.artwork_data_url);
        return webTrack.artwork_data_url;
      }
      const url = await trackArtworkUrl(path.slice("web://".length));
      cache.set(path, url);
      return url;
    }
    const url = await invoke<string | null>("get_track_artwork", { path });
    cache.set(path, url);
    return url;
  } catch {
    cache.set(path, null);
    return null;
  }
}

/** Render artwork into a container: <img> when available, glyph otherwise. */
export async function renderArtwork(
  container: HTMLElement,
  path: string | null,
): Promise<void> {
  container.querySelectorAll("img").forEach((n) => n.remove());
  const generation = (Number(container.dataset.artGeneration) || 0) + 1;
  container.dataset.artGeneration = String(generation);
  const glyph = container.querySelector(".art-glyph") as HTMLElement | null;
  if (!path) {
    if (glyph) glyph.style.display = "";
    return;
  }
  const url = await getArtwork(path);
  if (container.dataset.artGeneration !== String(generation)) return;
  if (!url) {
    if (glyph) glyph.style.display = "";
    return;
  }
  if (glyph) glyph.style.display = "none";
  const img = document.createElement("img");
  img.src = url;
  img.alt = "Cover art";
  img.className = "art-img";
  container.appendChild(img);
}
