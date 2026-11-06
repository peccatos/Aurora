import { describe, it } from "node:test";
import assert from "node:assert/strict";
import {
  buildSpotifyAuthorizationCodeExchange,
  buildSpotifyAuthorizationUrl,
  buildSpotifyRefreshRequest,
  buildSpotifyRevokeRequest,
  normalizeSpotifyPlaylistPage,
  normalizeSpotifyTokenResponse,
  normalizeSpotifyUser,
  spotifyCapabilities,
} from "../src/spotify-oauth.ts";

void describe("Spotify OAuth adapter contracts", () => {
  it("builds a PKCE authorization URL without credentials", () => {
    const url = buildSpotifyAuthorizationUrl({
      clientId: "public-client",
      redirectUri: "http://localhost/callback",
      state: "state-value",
      codeChallenge: "a".repeat(43),
      scopes: ["user-read-private", "playlist-read-private", "playlist-read-private"],
      showDialog: true,
    });
    assert.equal(url.origin + url.pathname, "https://accounts.spotify.com/authorize");
    assert.equal(url.searchParams.get("client_id"), "public-client");
    assert.equal(url.searchParams.get("scope"), "user-read-private playlist-read-private");
    assert.equal(url.searchParams.get("code_challenge_method"), "S256");
    assert.equal(url.searchParams.get("show_dialog"), "true");
    assert.throws(() => buildSpotifyAuthorizationUrl({
      clientId: "client", redirectUri: "https://app/cb", state: "state",
      codeChallenge: "short", scopes: [],
    }), /PKCE/);
  });

  it("normalizes token responses and preserves refresh-token omission", () => {
    const token = normalizeSpotifyTokenResponse({
      access_token: "access", token_type: "Bearer", expires_in: 3600,
      refresh_token: "refresh", scope: "user-read-private playlist-read-private unknown",
    }, 1_000);
    assert.deepEqual(token, {
      accessToken: "access", refreshToken: "refresh", tokenType: "Bearer",
      scope: ["user-read-private", "playlist-read-private"], expiresAt: 3_601_000,
    });
    assert.equal(normalizeSpotifyTokenResponse({ access_token: "a", token_type: "bearer", expires_in: 1 }).refreshToken, null);
    assert.throws(() => normalizeSpotifyTokenResponse({ access_token: "a", token_type: "Basic", expires_in: 1 }), /token type/);
  });

  it("creates form requests for exchange, refresh, and local revoke boundary", () => {
    const exchange = buildSpotifyAuthorizationCodeExchange({ clientId: "id", code: "code", redirectUri: "https://app/cb", codeVerifier: "verifier" });
    assert.equal(exchange.url, "https://accounts.spotify.com/api/token");
    assert.equal(exchange.body.get("grant_type"), "authorization_code");
    assert.equal(exchange.body.get("code_verifier"), "verifier");
    const refresh = buildSpotifyRefreshRequest({ clientId: "id", refreshToken: "refresh" });
    assert.equal(refresh.body.get("grant_type"), "refresh_token");
    const revoke = buildSpotifyRevokeRequest({ clientId: "id", refreshToken: "refresh" });
    assert.equal(revoke.url, "https://accounts.spotify.com/revoke");
    assert.equal(revoke.body.get("token_type_hint"), "refresh_token");
  });

  it("normalizes identity, capabilities, and paged playlists", () => {
    const user = normalizeSpotifyUser({ id: "u1", display_name: "Ada", email: "ada@example.test", images: [{ url: "https://cdn.test/avatar" }] });
    assert.deepEqual(user, { id: "u1", displayName: "Ada", email: "ada@example.test", avatarUrl: "https://cdn.test/avatar" });
    assert.deepEqual(spotifyCapabilities(["user-read-private", "playlist-read-private", "playlist-modify-public"]), ["identity", "playlists_read", "playlists_write"]);
    const page = normalizeSpotifyPlaylistPage({
      items: [{ id: "p1", name: "Favorites", description: null, owner: { id: "u1" }, snapshot_id: "snap", tracks: {
        total: 1, next: "https://api.spotify.com/v1/playlists/p1/tracks?offset=1", items: [{ track: {
          id: "t1", uri: "spotify:track:t1", name: "Song", duration_ms: 1234, artists: [{ name: "Artist" }], album: { name: "Album" },
        } }],
      } }], next: "https://api.spotify.com/v1/me/playlists?offset=50",
    });
    assert.equal(page.items[0]?.tracks[0]?.title, "Song");
    assert.equal(page.items[0]?.trackCount, 1);
    assert.equal(page.cursor, "50");
    assert.throws(() => normalizeSpotifyPlaylistPage({ items: [], next: "https://evil.test/page" }), /pagination host/);
  });
});
