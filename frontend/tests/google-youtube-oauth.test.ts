import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { OAuthPendingSessionStore } from "../src/oauth.ts";
import {
  GOOGLE_YOUTUBE_SCOPES, buildGoogleAuthorizationUrl, normalizeGoogleTokenResponse,
  normalizeYouTubePlaylistPage, normalizeYouTubePlaylistItemPage,
} from "../src/google-youtube-oauth.ts";

void describe("Google/YouTube OAuth adapter contracts", () => {
  it("builds a Google authorization URL with state and PKCE", async () => {
    const session = await new OAuthPendingSessionStore().begin("google-youtube", "https://app.example/oauth/callback", 60_000, 1_000);
    const url = buildGoogleAuthorizationUrl({ clientId: "client.apps.googleusercontent.com", redirectUri: session.redirectUri }, session);
    assert.equal(url.origin + url.pathname, "https://accounts.google.com/o/oauth2/v2/auth");
    assert.equal(url.searchParams.get("response_type"), "code");
    assert.equal(url.searchParams.get("client_id"), "client.apps.googleusercontent.com");
    assert.equal(url.searchParams.get("redirect_uri"), session.redirectUri);
    assert.equal(url.searchParams.get("state"), session.state.value);
    assert.equal(url.searchParams.get("code_challenge"), session.pkce.challenge);
    assert.equal(url.searchParams.get("code_challenge_method"), "S256");
    assert.equal(url.searchParams.get("access_type"), "offline");
    assert.deepEqual(url.searchParams.get("scope")?.split(" "), [...GOOGLE_YOUTUBE_SCOPES]);
  });

  it("rejects a mismatched or insecure redirect", async () => {
    const session = await new OAuthPendingSessionStore().begin("google-youtube", "https://app.example/callback", 60_000, 1_000);
    assert.throws(() => buildGoogleAuthorizationUrl({ clientId: "x", redirectUri: "https://other.example/callback" }, session), /does not match/);
    assert.throws(() => buildGoogleAuthorizationUrl({ clientId: "x", redirectUri: "javascript:alert(1)" }, session), /redirect URI/);
  });

  it("normalizes token response and keeps refresh token optional", () => {
    assert.deepEqual(normalizeGoogleTokenResponse({ access_token: " access ", expires_in: "3600", scope: "openid https://www.googleapis.com/auth/youtube openid" }, 1_000), {
      accessToken: "access", tokenType: "Bearer", expiresInSeconds: 3600,
      expiresAt: "1970-01-01T01:00:01.000Z", refreshToken: null,
      scopes: ["openid", "https://www.googleapis.com/auth/youtube"],
    });
    assert.throws(() => normalizeGoogleTokenResponse({ access_token: "x", expires_in: 0 }), /expiry/);
  });

  it("normalizes playlist pages and preserves pagination contracts", () => {
    const page = normalizeYouTubePlaylistPage({
      nextPageToken: "next", pageInfo: { totalResults: 1, resultsPerPage: 50 }, items: [{
        id: "PL1", snippet: { title: "Mix", description: "Songs", channelId: "UC1", publishedAt: "2025-01-01T00:00:00Z", thumbnails: { high: { url: "https://img.example/mix" } } }, contentDetails: { itemCount: 2 },
      }],
    });
    assert.equal(page.nextPageToken, "next");
    assert.equal(page.items[0].provider, "google-youtube");
    assert.equal(page.items[0].itemCount, 2);
    assert.equal(page.items[0].thumbnailUrl, "https://img.example/mix");

    const items = normalizeYouTubePlaylistItemPage({ items: [{ id: "PLI1", snippet: { playlistId: "PL1", position: 0, title: "Video", publishedAt: "2025-01-01T00:00:00Z", resourceId: { videoId: "v1" } } }] });
    assert.deepEqual(items.items[0], { id: "PLI1", playlistId: "PL1", position: 0, videoId: "v1", title: "Video", channelTitle: null, publishedAt: "2025-01-01T00:00:00Z" });
  });
});
