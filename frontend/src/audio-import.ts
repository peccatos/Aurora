const AUDIO_EXTENSIONS = new Set(["flac", "wav", "wave", "mp3", "m4a", "alac", "aac", "ogg", "oga", "opus", "aif", "aiff", "wv"]);
export const MAX_IMPORT_BYTES = 512 * 1024 * 1024;
export function audioExtension(name: string): string { return name.split(".").pop()?.toLowerCase() ?? ""; }
export function validateAudioFile(file: Pick<File, "name" | "size" | "type">): string | null {
  const ext = audioExtension(file.name);
  if (!AUDIO_EXTENSIONS.has(ext)) return "Unsupported audio format";
  if (file.size <= 0) return "The audio file is empty";
  if (file.size > MAX_IMPORT_BYTES) return "Audio files must be 512 MB or smaller";
  return null;
}
export function importDuplicateKey(file: Pick<File, "name" | "size" | "lastModified">): string { return `${file.name.toLowerCase()}|${file.size}|${file.lastModified}`; }
