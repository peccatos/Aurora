/** Provider capability boundary. This registry describes integrations only; it
 * never authenticates users or fetches protected media. A provider marked
 * `oauth-required` remains unavailable until an explicit auth adapter exists.
 */

export type ProviderId =
  | "itunes"
  | "deezer"
  | "musicbrainz"
  | "archive-org"
  | "spotify"
  | "youtube-music"
  | "soundcloud";

/** Canonical IDs shared with the native provider registry. */
export function canonicalProviderId(value: string): ProviderId {
  const id = value.trim().toLowerCase();
  if (id === "google-youtube" || id === "youtube") return "youtube-music";
  if (descriptors.some((provider) => provider.id === id)) return id as ProviderId;
  throw new Error(`Unknown provider: ${value}`);
}

export type ProviderAvailability = "available" | "oauth-required" | "unsupported";
export type ProviderCapability =
  | "metadata-search"
  | "preview-playback"
  | "licensed-full-track"
  | "playlist-import";

export interface ProviderDescriptor {
  readonly id: ProviderId;
  readonly name: string;
  readonly availability: ProviderAvailability;
  readonly capabilities: readonly ProviderCapability[];
  readonly limitation: string | null;
}

/** Common normalized output contract for search adapters. */
export interface ProviderTrack {
  readonly provider: ProviderId;
  readonly title: string;
  readonly artist: string;
  readonly album: string | null;
  readonly durationMs: number | null;
  readonly artworkUrl: string | null;
  readonly previewUrl: string | null;
  readonly externalId?: string;
}

/** Provider adapter contract; adapters must return normalized catalog data,
 * never imply that a preview or metadata result is a full-track stream. */
export interface ProviderAdapter {
  readonly provider: ProviderId;
  search(query: string): Promise<readonly ProviderTrack[]>;
}

const descriptors: readonly ProviderDescriptor[] = [
  { id: "itunes", name: "iTunes", availability: "available", capabilities: ["metadata-search", "preview-playback"], limitation: "Preview clips only" },
  { id: "deezer", name: "Deezer", availability: "available", capabilities: ["metadata-search", "preview-playback"], limitation: "Preview clips only; browser CORS may block requests" },
  { id: "musicbrainz", name: "MusicBrainz", availability: "available", capabilities: ["metadata-search"], limitation: "Metadata only; no audio playback" },
  { id: "archive-org", name: "Internet Archive", availability: "available", capabilities: ["metadata-search", "licensed-full-track"], limitation: "Only legally shared/public-domain items" },
  { id: "spotify", name: "Spotify", availability: "oauth-required", capabilities: ["metadata-search", "playlist-import"], limitation: "OAuth and account integration are not implemented; protected playback is not supported" },
  { id: "youtube-music", name: "YouTube Music", availability: "oauth-required", capabilities: ["metadata-search", "playlist-import"], limitation: "OAuth and account integration are not implemented; protected playback is not supported" },
  { id: "soundcloud", name: "SoundCloud", availability: "unsupported", capabilities: [], limitation: "No provider adapter is implemented" },
];

export const PROVIDERS: readonly ProviderDescriptor[] = descriptors;

export function getProvider(id: ProviderId): ProviderDescriptor {
  const provider = descriptors.find((candidate) => candidate.id === id);
  if (!provider) throw new Error(`Unknown provider: ${String(id)}`);
  return provider;
}

/** Fail closed for providers that cannot be searched without an adapter. */
export function canSearch(provider: ProviderDescriptor): boolean {
  return provider.availability === "available" && provider.capabilities.includes("metadata-search");
}

/** Execute an adapter only when its provider is explicitly available. */
export async function searchWithAdapter(
  adapter: ProviderAdapter,
  query: string,
): Promise<readonly ProviderTrack[]> {
  if (!query.trim()) return [];
  const provider = getProvider(adapter.provider);
  if (!canSearch(provider)) throw new Error(`Provider ${provider.id} is ${provider.availability}`);
  return adapter.search(query.trim());
}
