/** Drag-n-drop audio files from Explorer into the app window.
 *  Tauri's drag-drop events carry absolute file paths (DOM events don't). */

import { invoke } from "@tauri-apps/api/core";
import { app, setQueue, setToast } from "./state";

const AUDIO_EXT = new Set([
  "flac", "wav", "wave", "mp3", "m4a", "alac", "aac",
  "ogg", "oga", "opus", "aif", "aiff", "wv",
]);

function audioOnly(paths: string[]): string[] {
  return paths.filter((p) => {
    const ext = p.split(".").pop()?.toLowerCase() ?? "";
    return AUDIO_EXT.has(ext);
  });
}

export async function registerDrop() {
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  const win = getCurrentWindow();
  await win.onDragDropEvent((event) => {
    if (event.payload.type !== "drop") return;
    const paths = audioOnly(event.payload.paths);
    if (!paths.length) return;
    app.queueTracks = paths.map((path) => ({ path }));
    app.queueCurrent = 0;
    app.selectedPath = paths[0];
    void setQueue(app.queueTracks).then(() => {
      void invoke("play_queue_index", { index: 0 });
      const name = paths[0].split(/[\\/]/).pop();
      setToast(paths.length > 1 ? `Dropped ${paths.length} tracks` : `Loaded: ${name}`);
    });
  });
}
