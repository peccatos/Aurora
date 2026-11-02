/** Embedded cover art: data-URL cache (path -> url | null). */

import { invoke } from "@tauri-apps/api/core";

const cache = new Map<string, string | null>();

export async function getArtwork(path: string): Promise<string | null> {
  if (cache.has(path)) return cache.get(path) ?? null;
  try {
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
  const glyph = container.querySelector(".art-glyph") as HTMLElement | null;
  if (!path) {
    if (glyph) glyph.style.display = "";
    return;
  }
  const url = await getArtwork(path);
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
