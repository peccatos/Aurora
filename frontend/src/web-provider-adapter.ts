import { registerProviderAuthAdapter } from "./accounts.ts";
import { normalizeOAuthAccount } from "./oauth.ts";
import { platform } from "./invoke.ts";
import { normalizeSpotifyPlaylistPage } from "./spotify-oauth.ts";
import { normalizeYouTubePlaylistPage, normalizeYouTubePlaylistItemPage } from "./google-youtube-oauth.ts";
import type { RemotePlaylistSnapshot, RemotePlaylistSummary, RemoteProvider, RemotePlaylistTrack } from "./remote-playlist-import.ts";

declare global {
  interface ImportMeta {
    readonly env?: {
      readonly VITE_AURORA_OAUTH_BASE_URL?: string;
      readonly VITE_AURORA_MEDIA_BASE_URL?: string;
    };
  }
}

const WEB_PROVIDERS = ["spotify", "youtube-music"] as const;
type WebProvider = (typeof WEB_PROVIDERS)[number];

/** Test seam: the build-time env is read-only and per-module, so tests
 *  (which have no Vite `import.meta.env`) install the base URL here. */
let apiBaseOverride: string | null = null;

/** Overrides the configured OAuth service base URL. Pass `null` to fall
 *  back to `VITE_AURORA_OAUTH_BASE_URL`. Intended for tests. */
export function setApiBaseForTests(value: string | null): void {
  apiBaseOverride = value;
}

function configuredApiBase(): URL | null {
  // Same-origin by default: the web shell is served by the API service itself,
  // which keeps the session/CSRF cookies first-party. An absolute base URL
  // (legacy split deployment) can still be forced via build-time env.
  const raw = apiBaseOverride
    ?? (import.meta.env?.VITE_AURORA_OAUTH_BASE_URL
      || (typeof window !== "undefined" ? window.location.origin : ""));
  if (!raw) return null;
  try {
    const url = new URL(raw);
    if (url.protocol !== "https:") return null;
    url.pathname = url.pathname.replace(/\/+$/, "");
    url.search = "";
    url.hash = "";
    return url;
  } catch {
    return null;
  }
}

/** Name of the readable cookie the API sets alongside the HttpOnly session cookie. */
const CSRF_COOKIE = "__Host-aurora_csrf";

/**
 * Reads the CSRF token from its deliberately readable (non-HttpOnly, `__Host-` scoped)
 * cookie and echoes it in `X-CSRF-Token`. Read at call time so a rotated token is picked
 * up without a reload, and never cached in JS-accessible storage.
 */
export function csrfToken(): string | null {
  if (typeof document === "undefined") return null;
  for (const part of document.cookie.split(";")) {
    const trimmed = part.trim();
    if (trimmed.startsWith(`${CSRF_COOKIE}=`)) {
      const value = trimmed.slice(CSRF_COOKIE.length + 1);
      return value.length > 0 ? value : null;
    }
  }
  return null;
}

const AUTH_HOSTS: Record<WebProvider, string> = {
  spotify: "accounts.spotify.com",
  "youtube-music": "accounts.google.com",
};

const FORBIDDEN_KEYS = new Set([
  "accesstoken",
  "refreshtoken",
  "authorizationcode",
  "code",
  "verifier",
  "codeverifier",
  "pkceverifier",
  "clientsecret",
  "token",
  "secret",
]);

function normalizedKey(key: string): string {
  return key.toLowerCase().replace(/[^a-z0-9]/g, "");
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function assertNoCredentialFields(value: unknown): void {
  if (Array.isArray(value)) {
    for (const item of value) assertNoCredentialFields(item);
    return;
  }
  if (!isObject(value)) return;
  for (const [key, nested] of Object.entries(value)) {
    if (FORBIDDEN_KEYS.has(normalizedKey(key))) throw new Error("OAuth response contained a forbidden credential field");
    assertNoCredentialFields(nested);
  }
}

async function readJson(response: Response): Promise<unknown> {
  try {
    return await response.json();
  } catch {
    throw new Error("OAuth service returned invalid JSON");
  }
}

function authorizationUrl(provider: WebProvider, value: unknown): string {
  if (!isObject(value) || typeof value.authorization_url !== "string") {
    throw new Error("OAuth service did not return an authorization URL");
  }
  assertNoCredentialFields(value);
  let url: URL;
  try { url = new URL(value.authorization_url); } catch { throw new Error("OAuth service returned an invalid authorization URL"); }
  if (url.protocol !== "https:" || url.hostname !== AUTH_HOSTS[provider]) {
    throw new Error("OAuth service returned an untrusted authorization URL");
  }
  return url.href;
}

async function requestJson(
  apiBase: URL,
  path: string,
  init?: RequestInit,
  query?: Record<string, string | null | undefined>,
): Promise<unknown> {
  const url = new URL(apiBase.href);
  url.pathname = `${apiBase.pathname.replace(/\/+$/, "")}/${path.replace(/^\/+/, "")}`;
  // Assigned through searchParams (never through `pathname`, which would
  // percent-encode the "?" into the path itself).
  for (const [key, value] of Object.entries(query ?? {})) {
    if (value !== null && value !== undefined && value !== "") url.searchParams.set(key, value);
  }
  const method = (init?.method ?? "GET").toUpperCase();
  // Echo the session-bound token from the readable cookie whenever present.
  // Non-GET calls must carry it (server rejects otherwise); the playlist
  // detail endpoint additionally requires it on GET as a read-driving
  // defense, so it is attached uniformly rather than method-gated.
  const token = csrfToken();
  const response = await fetch(url, {
    ...init,
    credentials: "include",
    headers: {
      Accept: "application/json",
      ...(token ? { "X-CSRF-Token": token } : {}),
      ...(init?.headers ?? {}),
    },
  });
  if (!response.ok) {
    if (response.status === 404) return undefined;
    throw new Error(`OAuth service request failed (${response.status})`);
  }
  return response.status === 204 ? null : readJson(response);
}

function register(provider: WebProvider, apiBase: URL): void {
  registerProviderAuthAdapter({
    provider,
    beginAuthorization: async () => {
      // `POST .../begin` is state-changing: it rotates the session and its CSRF token, so
      // it must present the token from the previous handshake. The very first call has no
      // session yet and therefore needs none.
      const value = await requestJson(apiBase, `/api/oauth/${provider}/begin`, { method: "POST" });
      return authorizationUrl(provider, value);
    },
    getStatus: async () => {
      const value = await requestJson(apiBase, `/api/oauth/${provider}/status`);
      if (value === null || value === undefined) return null;
      assertNoCredentialFields(value);
      if (!isObject(value)) throw new Error("OAuth service returned an invalid account response");
      return normalizeOAuthAccount({
        provider,
        subject: typeof value.subject === "string" ? value.subject : "",
        displayName: typeof value.displayName === "string" ? value.displayName : null,
        avatarUrl: typeof value.avatarUrl === "string" ? value.avatarUrl : null,
        capabilities: Array.isArray(value.capabilities) ? value.capabilities.filter((item): item is string => typeof item === "string") : [],
        connectedAt: typeof value.connectedAt === "string" ? value.connectedAt : undefined,
      });
    },
    revoke: async () => {
      await requestJson(apiBase, `/api/oauth/${provider}`, { method: "DELETE" });
    },
  });
}

function normalizeRemoteTrack(item: unknown, position: number): RemotePlaylistTrack | null {
  if (!isObject(item)) return null;
  const id = typeof item.id === "string" ? item.id : typeof item.remoteId === "string" ? item.remoteId : null;
  const title = typeof item.title === "string" ? item.title : typeof item.name === "string" ? item.name : null;
  if (id === null || title === null) return null;
  const rawArtists = Array.isArray(item.artists) ? item.artists : [];
  const artists = rawArtists.flatMap((artist): string[] => {
    if (typeof artist === "string") return artist.trim() ? [artist] : [];
    return isObject(artist) && typeof artist.name === "string" ? [artist.name] : [];
  });
  return {
    remoteId: id,
    title,
    artists,
    album: typeof item.album === "string" ? item.album : null,
    durationMs: typeof item.durationMs === "number" && Number.isFinite(item.durationMs) ? item.durationMs : null,
    position: typeof item.position === "number" && Number.isInteger(item.position) && item.position >= 0 ? item.position : position,
  };
}

/** Tracks may arrive flat or wrapped in a provider entry ("track"/"item"),
 *  and the page cursor is either a bare string or a URL. */
function normalizeRemoteTracks(value: Record<string, unknown>): RemotePlaylistTrack[] {
  const raw = Array.isArray(value.tracks) ? value.tracks : [];
  return raw.flatMap((entry, position) => {
    const source = isObject(entry) && isObject(entry.track)
      ? entry.track
      : isObject(entry) && isObject(entry.item)
        ? entry.item
        : entry;
    const track = normalizeRemoteTrack(source, position);
    return track ? [track] : [];
  });
}

function nextCursorOf(value: unknown): string | null {
  if (typeof value !== "string") return null;
  try {
    return new URL(value).searchParams.get("offset") ?? new URL(value).searchParams.get("pageToken") ?? value;
  } catch {
    return value;
  }
}

export async function listRemotePlaylists(provider: RemoteProvider): Promise<{ items: RemotePlaylistSummary[]; nextCursor: string | null }> {
  const apiBase = configuredApiBase();
  if (!apiBase) throw new Error("OAuth service is not configured");
  const value = await requestJson(apiBase, `/api/oauth/${provider}/playlists`);
  if (provider === "spotify") {
    const page = normalizeSpotifyPlaylistPage(value);
    return { items: page.items.map((p) => ({ provider, id: p.id, name: p.name, trackCount: p.trackCount, revision: p.snapshotId })), nextCursor: page.cursor };
  }
  const page = normalizeYouTubePlaylistPage(value);
  return { items: page.items.map((p) => ({ provider, id: p.id, name: p.title, trackCount: p.itemCount, revision: null })), nextCursor: page.nextPageToken };
}

export async function getRemotePlaylist(provider: RemoteProvider, playlistId: string): Promise<RemotePlaylistSnapshot> {
  if (!/^[A-Za-z0-9._:-]{1,256}$/.test(playlistId)) throw new Error("Invalid playlist ID");
  const apiBase = configuredApiBase();
  if (!apiBase) throw new Error("OAuth service is not configured");
  const value = await requestJson(apiBase, `/api/oauth/${provider}/playlists/${encodeURIComponent(playlistId)}`);
  assertNoCredentialFields(value);
  if (!isObject(value) || !Array.isArray(value.tracks)) throw new Error("OAuth service returned an invalid playlist snapshot");
  const tracks = normalizeRemoteTracks(value);
  if (!tracks.length && value.tracks.length) throw new Error("OAuth service returned an invalid playlist snapshot");
  return {
    provider,
    id: playlistId,
    name: typeof value.name === "string" && value.name.trim() ? value.name : "Imported playlist",
    trackCount: tracks.length,
    revision: typeof value.revision === "string" ? value.revision : null,
    tracks,
  };
}

/** Paged snapshot fetch: walks the provider cursor until the playlist is
 *  complete, preserving provider order and re-numbering positions
 *  contiguously across pages. */
export async function fetchRemotePlaylistSnapshot(
  provider: RemoteProvider,
  playlistId: string,
  options: { maxPages?: number; cursor?: string | null; pageSize?: number } = {},
): Promise<RemotePlaylistSnapshot> {
  const pageSize = options.pageSize ?? 100;
  const maxPages = options.maxPages ?? Math.ceil(10_000 / pageSize);
  const apiBase = requireApiBase();
  const path = `/api/oauth/${provider}/playlists/${encodeURIComponent(playlistId)}`;
  let cursor = options.cursor ?? null;
  let snapshot: RemotePlaylistSnapshot | null = null;
  const tracks: RemotePlaylistTrack[] = [];
  // Every cursor the walk has *issued a request for*, including the
  // initial one. A provider that echoes the same cursor back is stopped
  // before its page is fetched a second time, so no page is appended
  // twice and the loop can never spin forever.
  const requestedCursors = new Set<string>();

  for (let page = 0; page < maxPages; page++) {
    if (requestedCursors.has(cursor ?? "")) break;
    requestedCursors.add(cursor ?? "");
    const request = await requestJson(apiBase, path, undefined, cursor ? { cursor } : undefined);
    assertNoCredentialFields(request);
    if (!isObject(request) || !Array.isArray(request.tracks)) throw new Error("OAuth service returned an invalid playlist snapshot");
    const normalized = normalizeRemoteTracks(request);
    if (!normalized.length && request.tracks.length) throw new Error("OAuth service returned an invalid playlist snapshot");
    for (const track of normalized) tracks.push({ ...track, position: tracks.length });
    if (!snapshot) {
      snapshot = {
        provider,
        id: playlistId,
        name: typeof request.name === "string" && request.name.trim() ? request.name : "Imported playlist",
        trackCount: null,
        revision: typeof request.revision === "string" ? request.revision : null,
        tracks: [],
      };
    }
    const next = nextCursorOf(request.nextCursor) ?? nextCursorOf(request.next) ?? nextCursorOf(request.nextPageToken);
    if (!next) break;
    cursor = next;
  }
  if (!snapshot) throw new Error("OAuth service returned an invalid playlist snapshot");
  return { ...snapshot, trackCount: tracks.length, tracks };
}

function requireApiBase(): URL {
  const apiBase = configuredApiBase();
  if (!apiBase) throw new Error("OAuth service is not configured");
  return apiBase;
}

/** Browser OAuth is enabled only as a server-session adapter. */
export function registerWebProviderAdapters(): void {
  if (platform !== "web") return;
  const apiBase = configuredApiBase();
  if (!apiBase) return;
  for (const provider of WEB_PROVIDERS) register(provider, apiBase);
}
