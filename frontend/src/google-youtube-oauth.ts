import type { OAuthCapability, PendingOAuthSession } from "./oauth.ts";

export const GOOGLE_YOUTUBE_PROVIDER = "google-youtube";
export const GOOGLE_AUTHORIZATION_ENDPOINT = "https://accounts.google.com/o/oauth2/v2/auth";
export const GOOGLE_TOKEN_ENDPOINT = "https://oauth2.googleapis.com/token";

/** The minimum scopes required for account identity and playlist synchronization. */
export const GOOGLE_YOUTUBE_SCOPES = [
  "openid",
  "https://www.googleapis.com/auth/userinfo.profile",
  "https://www.googleapis.com/auth/youtube.readonly",
  "https://www.googleapis.com/auth/youtube",
] as const;

export type GoogleYouTubeScope = (typeof GOOGLE_YOUTUBE_SCOPES)[number];

export interface GoogleAuthorizationConfig {
  clientId: string;
  redirectUri: string;
  scopes?: readonly string[];
}

function required(value: string, name: string, maxLength = 2048): string {
  const normalized = value.trim();
  if (!normalized || normalized.length > maxLength) throw new Error(`Invalid Google OAuth ${name}`);
  return normalized;
}

function assertRedirectUri(value: string): string {
  const uri = new URL(required(value, "redirect URI"));
  const localHttp = uri.protocol === "http:" &&
    (uri.hostname === "localhost" || uri.hostname === "127.0.0.1" || uri.hostname === "[::1]");
  if (uri.protocol !== "https:" && !localHttp) throw new Error("Invalid Google OAuth redirect URI");
  return uri.href;
}

/** Build Google's authorization URL. PKCE verifier/state remain in the pending session. */
export function buildGoogleAuthorizationUrl(
  config: GoogleAuthorizationConfig,
  session: PendingOAuthSession,
): URL {
  if (session.provider !== GOOGLE_YOUTUBE_PROVIDER && session.provider !== "youtube") {
    throw new Error("OAuth session is not for Google/YouTube");
  }
  const redirectUri = assertRedirectUri(config.redirectUri);
  if (redirectUri !== session.redirectUri) throw new Error("OAuth redirect URI does not match session");
  const clientId = required(config.clientId, "client ID", 512);
  const scopes = [...new Set((config.scopes ?? GOOGLE_YOUTUBE_SCOPES).map((scope) => required(scope, "scope", 256)))];
  if (!scopes.length) throw new Error("Google OAuth requires at least one scope");

  const url = new URL(GOOGLE_AUTHORIZATION_ENDPOINT);
  url.searchParams.set("client_id", clientId);
  url.searchParams.set("redirect_uri", redirectUri);
  url.searchParams.set("response_type", "code");
  url.searchParams.set("scope", scopes.join(" "));
  url.searchParams.set("state", session.state.value);
  url.searchParams.set("code_challenge", session.pkce.challenge);
  url.searchParams.set("code_challenge_method", session.pkce.method);
  url.searchParams.set("access_type", "offline");
  url.searchParams.set("prompt", "consent");
  return url;
}

export interface NormalizedGoogleToken {
  accessToken: string;
  tokenType: "Bearer";
  expiresInSeconds: number;
  expiresAt: string;
  refreshToken: string | null;
  scopes: string[];
}

/** Normalize an untrusted token endpoint response without retaining provider-specific fields. */
export function normalizeGoogleTokenResponse(
  input: unknown,
  now = Date.now(),
): NormalizedGoogleToken {
  if (!input || typeof input !== "object") throw new Error("Invalid Google OAuth token response");
  const value = input as Record<string, unknown>;
  const accessToken = typeof value.access_token === "string" ? required(value.access_token, "access token") : "";
  if (!accessToken) throw new Error("Google OAuth token response is missing access token");
  const expiresIn = typeof value.expires_in === "number" ? value.expires_in : Number(value.expires_in);
  if (!Number.isSafeInteger(expiresIn) || expiresIn <= 0 || expiresIn > 31_536_000) {
    throw new Error("Invalid Google OAuth token expiry");
  }
  if (!Number.isFinite(now)) throw new Error("Invalid token timestamp");
  const rawScopes = typeof value.scope === "string" ? value.scope.split(/\s+/) : [];
  return {
    accessToken,
    tokenType: "Bearer",
    expiresInSeconds: expiresIn,
    expiresAt: new Date(now + expiresIn * 1000).toISOString(),
    refreshToken: typeof value.refresh_token === "string" && value.refresh_token.trim() ? value.refresh_token : null,
    scopes: [...new Set(rawScopes.map((scope) => scope.trim()).filter(Boolean))],
  };
}

export const GOOGLE_YOUTUBE_CAPABILITIES: readonly OAuthCapability[] = [
  "identity", "playlists_read", "playlists_write",
];

export interface YouTubePlaylist {
  provider: typeof GOOGLE_YOUTUBE_PROVIDER;
  id: string;
  title: string;
  description: string;
  channelId: string | null;
  publishedAt: string | null;
  itemCount: number | null;
  thumbnailUrl: string | null;
}

export interface YouTubePlaylistItem {
  id: string;
  playlistId: string;
  position: number | null;
  videoId: string | null;
  title: string;
  channelTitle: string | null;
  publishedAt: string | null;
}

export interface YouTubePage<T> {
  items: T[];
  nextPageToken: string | null;
  prevPageToken: string | null;
  totalResults: number | null;
  resultsPerPage: number | null;
}

function optionalText(value: unknown): string | null {
  return typeof value === "string" && value.trim() ? value.trim() : null;
}

function thumbnail(value: unknown): string | null {
  if (!value || typeof value !== "object") return null;
  const url = (value as { url?: unknown }).url;
  return typeof url === "string" && /^https?:\/\//.test(url) ? url : null;
}

export function normalizeYouTubePlaylist(input: unknown): YouTubePlaylist {
  if (!input || typeof input !== "object") throw new Error("Invalid YouTube playlist");
  const item = input as Record<string, unknown>;
  const snippet = item.snippet && typeof item.snippet === "object" ? item.snippet as Record<string, unknown> : {};
  const details = item.contentDetails && typeof item.contentDetails === "object" ? item.contentDetails as Record<string, unknown> : {};
  const id = required(typeof item.id === "string" ? item.id : "", "playlist ID", 256);
  const publishedAt = optionalText(snippet.publishedAt);
  if (publishedAt && Number.isNaN(Date.parse(publishedAt))) throw new Error("Invalid YouTube playlist date");
  const count = details.itemCount;
  const itemCount = typeof count === "number" && Number.isSafeInteger(count) && count >= 0 ? count : null;
  const thumbs = snippet.thumbnails && typeof snippet.thumbnails === "object" ? snippet.thumbnails as Record<string, unknown> : {};
  return {
    provider: GOOGLE_YOUTUBE_PROVIDER,
    id,
    title: required(typeof snippet.title === "string" ? snippet.title : "Untitled playlist", "playlist title", 512),
    description: optionalText(snippet.description) ?? "",
    channelId: optionalText(snippet.channelId),
    publishedAt: publishedAt ? new Date(publishedAt).toISOString() : null,
    itemCount,
    thumbnailUrl: thumbnail(thumbs.high) ?? thumbnail(thumbs.medium) ?? thumbnail(thumbs.default),
  };
}

export function normalizeYouTubePlaylistPage(input: unknown): YouTubePage<YouTubePlaylist> {
  if (!input || typeof input !== "object") throw new Error("Invalid YouTube playlist page");
  const page = input as Record<string, unknown>;
  const raw = Array.isArray(page.items) ? page.items : [];
  return normalizePage(raw.map(normalizeYouTubePlaylist), page);
}

export function normalizeYouTubePlaylistItem(input: unknown): YouTubePlaylistItem {
  if (!input || typeof input !== "object") throw new Error("Invalid YouTube playlist item");
  const item = input as Record<string, unknown>;
  const snippet = item.snippet && typeof item.snippet === "object" ? item.snippet as Record<string, unknown> : {};
  const resource = snippet.resourceId && typeof snippet.resourceId === "object" ? snippet.resourceId as Record<string, unknown> : {};
  const position = snippet.position;
  return {
    id: required(typeof item.id === "string" ? item.id : "", "playlist item ID", 256),
    playlistId: required(typeof snippet.playlistId === "string" ? snippet.playlistId : "", "playlist ID", 256),
    position: typeof position === "number" && Number.isSafeInteger(position) && position >= 0 ? position : null,
    videoId: optionalText(resource.videoId),
    title: optionalText(snippet.title) ?? "Untitled video",
    channelTitle: optionalText(snippet.videoOwnerChannelTitle),
    publishedAt: optionalText(snippet.publishedAt),
  };
}

export function normalizeYouTubePlaylistItemPage(input: unknown): YouTubePage<YouTubePlaylistItem> {
  if (!input || typeof input !== "object") throw new Error("Invalid YouTube playlist item page");
  const page = input as Record<string, unknown>;
  const raw = Array.isArray(page.items) ? page.items : [];
  return normalizePage(raw.map(normalizeYouTubePlaylistItem), page);
}

function normalizePage<T>(items: T[], page: Record<string, unknown>): YouTubePage<T> {
  const pageInfo = page.pageInfo && typeof page.pageInfo === "object" ? page.pageInfo as Record<string, unknown> : {};
  const numberOrNull = (value: unknown) => typeof value === "number" && Number.isSafeInteger(value) && value >= 0 ? value : null;
  return {
    items,
    nextPageToken: optionalText(page.nextPageToken),
    prevPageToken: optionalText(page.prevPageToken),
    totalResults: numberOrNull(pageInfo.totalResults),
    resultsPerPage: numberOrNull(pageInfo.resultsPerPage),
  };
}
