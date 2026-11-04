/** Platform-aware file picking (ADR 0005 shells).
 *
 * Desktop (Tauri): the native dialog — returns absolute paths.
 * Web: an <input type="file"> — returns browser File objects; the
 * caller routes them through the web track pipeline (registerWebTrack).
 *
 * The old direct `@tauri-apps/plugin-dialog` imports CRASHED on the web
 * (plugin internals call `invoke` which does not exist in a plain
 * browser) — every "Open file…" button silently died with a pageerror.
 */

import { platform } from "./invoke";

export interface PickedFiles {
  /** Desktop: absolute paths. Web: EMPTY (see files). */
  paths: string[];
  /** Web: browser File objects. Desktop: empty. */
  files: File[];
}

const AUDIO_ACCEPT = "audio/*,.flac,.wav,.wave,.mp3,.m4a,.alac,.aac,.ogg,.oga,.opus,.aif,.aiff,.wv";

/** Pick audio files. Multiple-select. Web callers: pass the picked
 *  files into registerWebTrack/ingest (Home page pipeline). */
export async function pickAudioFiles(): Promise<PickedFiles> {
  if (platform === "tauri") {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const picked = await open({
      multiple: true,
      filters: [
        {
          name: "Audio",
          extensions: [
            "flac", "wav", "wave", "mp3", "m4a", "alac", "aac",
            "ogg", "oga", "opus", "aif", "aiff", "wv",
          ],
        },
      ],
    });
    const paths = Array.isArray(picked) ? picked : typeof picked === "string" ? [picked] : [];
    return { paths, files: [] };
  }
  // Web: hidden input + a real user click (dialog opens on it).
  return new Promise((resolve) => {
    const input = document.createElement("input");
    input.type = "file";
    input.accept = AUDIO_ACCEPT;
    input.multiple = true;
    input.style.display = "none";
    input.addEventListener("change", () => {
      const files = Array.from(input.files ?? []);
      input.remove();
      resolve({ paths: [], files });
    });
    // Cancel: no change event — clean up on focus return.
    window.addEventListener(
      "focus",
      () => setTimeout(() => {
        if (input.isConnected) {
          input.remove();
          resolve({ paths: [], files: [] });
        }
      }, 1000),
      { once: true },
    );
    document.body.append(input);
    input.click();
  });
}

/** Pick a directory (library scan). Web: not supported — honest null
 *  (the web shell has no filesystem library; IndexedDB is the store). */
export async function pickDirectory(): Promise<string | null> {
  if (platform === "tauri") {
    const { open } = await import("@tauri-apps/plugin-dialog");
    return (await open({ multiple: false, directory: true })) as string | null;
  }
  return null;
}
