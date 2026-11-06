import { invoke, platform } from "./invoke";
import { updateTrackEnrichment } from "./library-db";
import { validateVideoUrl } from "./video-url";

export interface TrackEnrichmentInput {
  track_id: string;
  name?: string;
  artist?: string | null;
  album?: string | null;
  genre?: string | null;
  year?: number | null;
  lyrics?: string | null;
  lyrics_language?: string | null;
  lyrics_translation?: string | null;
  lyrics_translation_language?: string | null;
  artwork_data_url?: string | null;
  source_url?: string | null;
  source_provider?: string | null;
  video_url?: string | null;
  metadata_source?: string | null;
}

/** Persist user edits and provider metadata in the active library.
 * Desktop writes SQLite through Tauri; web writes the IndexedDB track record. */
export async function saveTrackEnrichment(input: TrackEnrichmentInput): Promise<void> {
  if (input.track_id.trim() === "") throw new Error("track_id is required");
  if (input.video_url !== undefined) { const error = validateVideoUrl(input.video_url ?? ""); if (error) throw new Error(error); }
  if (platform === "web") {
    await updateTrackEnrichment(input.track_id, input);
    return;
  }
  const { name: _name, artist: _artist, album: _album, genre: _genre, year: _year, ...desktopInput } = input;
  await invoke<void>("set_track_enrichment", { input: desktopInput });
}
