/** Spotify OAuth and Web API contracts. Network transport and secret storage are
 * intentionally injected by a future backend adapter; no credentials belong here.
 */

import type { OAuthCapability, OAuthProviderId } from "./oauth.ts";

export const SPOTIFY_PROVIDER: OAuthProviderId = "spotify";
export const SPOTIFY_AUTHORIZE_URL = "https://accounts.spotify.com/authorize";
export const SPOTIFY_TOKEN_URL = "https://accounts.spotify.com/api/token";
export const SPOTIFY_REVOKE_URL = "https://accounts.spotify.com/revoke";
export const SPOTIFY_API_BASE_URL = "https://api.spotify.com/v1";

export interface SpotifyAuthorizationInput {
  clientId: string;
  redirectUri: string;
  state: string;
  codeChallenge: string;
  scopes: readonly SpotifyScope[];
  showDialog?: boolean;
}

export type SpotifyScope =
  | "user-read-private"
  | "user-read-email"
  | "playlist-read-private"
  | "playlist-read-collaborative"
  | "playlist-modify-private"
  | "playlist-modify-public";

export function buildSpotifyAuthorizationUrl(input: SpotifyAuthorizationInput): URL {
  const clientId = nonEmpty(input.clientId, "client ID");
  const state = nonEmpty(input.state, "state");
  const challenge = nonEmpty(input.codeChallenge, "PKCE challenge");
  if (!/^[A-Za-z0-9_-]{43}$/.test(challenge)) throw new Error("Invalid Spotify PKCE challenge");
  const redirect = new URL(input.redirectUri);
  if (redirect.protocol !== "https:" && !(redirect.protocol === "http:" && ["localhost", "127.0.0.1", "[::1]"].includes(redirect.hostname))) {
    throw new Error("Invalid Spotify redirect URI");
  }
  const url = new URL(SPOTIFY_AUTHORIZE_URL);
  url.searchParams.set("response_type", "code");
  url.searchParams.set("client_id", clientId);
  url.searchParams.set("redirect_uri", redirect.href);
  url.searchParams.set("state", state);
  url.searchParams.set("code_challenge_method", "S256");
  url.searchParams.set("code_challenge", challenge);
  url.searchParams.set("scope", [...new Set(input.scopes)].join(" "));
  if (input.showDialog !== undefined) url.searchParams.set("show_dialog", String(input.showDialog));
  return url;
}

export interface SpotifyTokenResponse {
  accessToken: string;
  refreshToken: string | null;
  tokenType: "Bearer";
  scope: readonly SpotifyScope[];
  expiresAt: number;
}

const knownScopes = new Set<SpotifyScope>([
  "user-read-private", "user-read-email", "playlist-read-private",
  "playlist-read-collaborative", "playlist-modify-private", "playlist-modify-public",
]);

export function normalizeSpotifyTokenResponse(value: unknown, now = Date.now()): SpotifyTokenResponse {
  const raw = record(value, "Spotify token response");
  const accessToken = nonEmpty(stringField(raw.access_token, "access_token"), "access token");
  const expiresIn = raw.expires_in;
  if (typeof expiresIn !== "number" || !Number.isFinite(expiresIn) || expiresIn <= 0 || expiresIn > 31_536_000) {
    throw new Error("Invalid Spotify expires_in");
  }
  const type = nonEmpty(stringField(raw.token_type, "token_type"), "token type");
  if (type.toLowerCase() !== "bearer") throw new Error("Unsupported Spotify token type");
  const refreshToken = raw.refresh_token == null ? null : nonEmpty(stringField(raw.refresh_token, "refresh_token"), "refresh token");
  const scopeText = raw.scope == null ? "" : stringField(raw.scope, "scope");
  return {
    accessToken,
    refreshToken,
    tokenType: "Bearer",
    scope: [...new Set(scopeText.split(/\s+/).filter((scope): scope is SpotifyScope => knownScopes.has(scope as SpotifyScope)))],
    expiresAt: now + expiresIn * 1000,
  };
}

export interface SpotifyTokenRequest {
  url: string;
  method: "POST";
  headers: Readonly<Record<string, string>>;
  body: URLSearchParams;
}

export function buildSpotifyAuthorizationCodeExchange(input: {
  clientId: string;
  code: string;
  redirectUri: string;
  codeVerifier: string;
}): SpotifyTokenRequest {
  return tokenRequest({
    grant_type: "authorization_code",
    client_id: nonEmpty(input.clientId, "client ID"),
    code: nonEmpty(input.code, "authorization code"),
    redirect_uri: new URL(input.redirectUri).href,
    code_verifier: nonEmpty(input.codeVerifier, "PKCE verifier"),
  });
}

export function buildSpotifyRefreshRequest(input: { clientId: string; refreshToken: string }): SpotifyTokenRequest {
  return tokenRequest({
    grant_type: "refresh_token",
    client_id: nonEmpty(input.clientId, "client ID"),
    refresh_token: nonEmpty(input.refreshToken, "refresh token"),
  });
}

/** Spotify public OAuth has no standard revoke endpoint. This contract signals
 * local credential removal; it must not be mistaken for remote revocation.
 */
export function buildSpotifyRevokeRequest(input: { clientId: string; refreshToken: string }): SpotifyTokenRequest {
  const request = tokenRequest({
    grant_type: "revoke",
    client_id: nonEmpty(input.clientId, "client ID"),
    token: nonEmpty(input.refreshToken, "refresh token"),
    token_type_hint: "refresh_token",
  });
  return { ...request, url: SPOTIFY_REVOKE_URL };
}

export interface SpotifyUser { id: string; displayName: string; email: string | null; avatarUrl: string | null; }
export function normalizeSpotifyUser(value: unknown): SpotifyUser {
  const raw = record(value, "Spotify user");
  const images = Array.isArray(raw.images) ? raw.images : [];
  const avatarUrl = images.map((entry) => record(entry, "Spotify image").url).find((entry): entry is string => typeof entry === "string" && isHttpUrl(entry)) ?? null;
  return {
    id: nonEmpty(stringField(raw.id, "id"), "user ID"),
    displayName: typeof raw.display_name === "string" && raw.display_name.trim() ? raw.display_name.trim() : nonEmpty(stringField(raw.id, "id"), "user ID"),
    email: typeof raw.email === "string" ? raw.email : null,
    avatarUrl,
  };
}

export interface SpotifyPlaylistTrack {
  id: string | null;
  uri: string | null;
  title: string;
  artists: readonly string[];
  album: string | null;
  durationMs: number | null;
}
export interface SpotifyPlaylist {
  id: string;
  name: string;
  description: string | null;
  ownerId: string;
  trackCount: number;
  snapshotId: string | null;
  tracks: readonly SpotifyPlaylistTrack[];
  next: string | null;
}
export interface SpotifyPage<T> { items: readonly T[]; next: string | null; cursor: string | null; }

export function normalizeSpotifyPlaylistPage(value: unknown): SpotifyPage<SpotifyPlaylist> {
  const page = record(value, "Spotify playlist page");
  if (!Array.isArray(page.items)) throw new Error("Invalid Spotify playlist items");
  const items = page.items.map((item) => {
    const raw = record(item, "Spotify playlist");
    const owner = record(raw.owner, "Spotify playlist owner");
    const tracksPage = record(raw.tracks, "Spotify playlist tracks");
    const trackEntries = Array.isArray(tracksPage.items) ? tracksPage.items : [];
    const tracks = trackEntries.map((entry): SpotifyPlaylistTrack => {
      const track = record(record(entry, "Spotify playlist track entry").track, "Spotify track");
      const artists = Array.isArray(track.artists) ? track.artists : [];
      const duration = track.duration_ms;
      return {
        id: nullableString(track.id), uri: nullableString(track.uri),
        title: typeof track.name === "string" ? track.name : "",
        artists: artists.map((artist) => stringField(record(artist, "Spotify artist").name, "artist name")),
        album: track.album && typeof track.album === "object" ? nullableString((track.album as Record<string, unknown>).name) : null,
        durationMs: typeof duration === "number" && Number.isFinite(duration) && duration >= 0 ? duration : null,
      };
    });
    const count = typeof tracksPage.total === "number" && Number.isFinite(tracksPage.total) ? tracksPage.total : tracks.length;
    return {
      id: nonEmpty(stringField(raw.id, "playlist id"), "playlist ID"),
      name: stringField(raw.name, "playlist name"),
      description: nullableString(raw.description),
      ownerId: stringField(owner.id, "owner id"),
      trackCount: Math.max(0, Math.floor(count)),
      snapshotId: nullableString(raw.snapshot_id), tracks,
      next: safeSpotifyUrl(tracksPage.next),
    };
  });
  const next = safeSpotifyUrl(page.next);
  return { items, next, cursor: next ? new URL(next).searchParams.get("offset") : null };
}

export function spotifyCapabilities(scopes: readonly SpotifyScope[]): readonly OAuthCapability[] {
  const result: OAuthCapability[] = ["identity"];
  if (scopes.some((scope) => scope === "playlist-read-private" || scope === "playlist-read-collaborative")) result.push("playlists_read");
  if (scopes.some((scope) => scope === "playlist-modify-private" || scope === "playlist-modify-public")) result.push("playlists_write");
  return result;
}

function tokenRequest(fields: Record<string, string>): SpotifyTokenRequest {
  return {
    url: SPOTIFY_TOKEN_URL, method: "POST",
    headers: { "content-type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams(fields),
  };
}
function nonEmpty(value: string, field: string): string {
  const normalized = value.trim();
  if (!normalized || normalized.length > 4096) throw new Error(`Invalid Spotify ${field}`);
  return normalized;
}
function stringField(value: unknown, field: string): string {
  if (typeof value !== "string") throw new Error(`Invalid Spotify ${field}`);
  return value;
}
function nullableString(value: unknown): string | null { return typeof value === "string" ? value : null; }
function record(value: unknown, field: string): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error(`Invalid ${field}`);
  return value as Record<string, unknown>;
}
function isHttpUrl(value: string): boolean {
  try { const url = new URL(value); return url.protocol === "https:" || url.protocol === "http:"; } catch { return false; }
}
function safeSpotifyUrl(value: unknown): string | null {
  if (value == null) return null;
  if (typeof value !== "string" || !isHttpUrl(value)) throw new Error("Invalid Spotify pagination URL");
  const url = new URL(value);
  if (url.hostname !== "api.spotify.com" || url.protocol !== "https:") throw new Error("Unexpected Spotify pagination host");
  return url.href;
}
