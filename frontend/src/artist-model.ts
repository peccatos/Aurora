/** Pure helpers shared by the artist route and its tests. */

export interface ArtistTrackLike {
  artist: string | null;
}

/** Artist matching is case-insensitive and whitespace-stable, but does not
 * collapse punctuation: distinct artists must not be merged silently. */
export function normalizeArtist(value: string): string {
  return value.trim().replace(/\s+/g, " ").toLocaleLowerCase();
}

export function tracksForArtist<T extends ArtistTrackLike>(tracks: T[], artist: string): T[] {
  const wanted = normalizeArtist(artist);
  if (!wanted) return [];
  return tracks.filter((track) => track.artist != null && normalizeArtist(track.artist) === wanted);
}

export function artistRouteName(hash: string): string | null {
  const query = hash.split("?")[1] ?? "";
  const name = new URLSearchParams(query).get("name")?.trim() ?? "";
  return name || null;
}
