/** LRC sidecar pairing (web ingest): when the user drops audio files and
 *  same-stem .lrc files in one gesture, a sidecar text becomes the track's
 *  lyrics (the player already renders LRC natively).
 *
 *  Tauri/disk sidecar reading is out of scope — this pairs only files
 *  that arrived together in the same FileList.
 */

export const LRC_EXTENSION = "lrc";

function fileStem(name: string): string {
  return name.replace(/\.[^.]+$/, "").trim().toLocaleLowerCase();
}

function isLrc(file: Pick<File, "name">): boolean {
  return file.name.split(".").pop()?.toLowerCase() === LRC_EXTENSION;
}

/** Map audio file name → paired .lrc File (same stem). One .lrc matches
 *  at most one audio file; extra .lrc files are ignored (honest no-op). */
export function pairLrcSidecars(audioFiles: File[], allFiles: File[]): Map<string, File> {
  const pairs = new Map<string, File>();
  const lrcByStem = new Map<string, File[]>();
  for (const file of allFiles) {
    if (!isLrc(file)) continue;
    const stem = fileStem(file.name);
    lrcByStem.set(stem, [...(lrcByStem.get(stem) ?? []), file]);
  }
  for (const audio of audioFiles) {
    const lrc = lrcByStem.get(fileStem(audio.name))?.shift();
    if (lrc) pairs.set(audio.name, lrc);
  }
  return pairs;
}

/** Read the paired sidecars' text (best-effort: a read failure skips the
 *  file, never blocks the ingest). Returns audio file name → LRC text. */
export async function pairLrcSidecarsWithText(
  audioFiles: File[],
  allFiles: File[],
  readFile: (file: File) => Promise<string> = (f) => f.text(),
): Promise<Map<string, string>> {
  const out = new Map<string, string>();
  for (const [audioName, lrc] of pairLrcSidecars(audioFiles, allFiles)) {
    try {
      const text = (await readFile(lrc)).trim();
      if (text) out.set(audioName, text);
    } catch {
      /* unreadable sidecar — skip */
    }
  }
  return out;
}
