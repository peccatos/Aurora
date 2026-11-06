import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { OAuthPendingSessionStore, createOAuthState, createPkcePair, hasOAuthCapability, normalizeOAuthAccount, parseOAuthCallback, validateOAuthState } from "../src/oauth.ts";

void describe("OAuth protocol helpers", () => {
  it("creates unique state and checks equality and expiry", () => {
    const state = createOAuthState(1000, 5000);
    assert.notEqual(state.value, createOAuthState(1000, 5000).value);
    assert.equal(validateOAuthState(state, state.value, 5000), true);
    assert.equal(validateOAuthState(state, `${state.value}x`, 5000), false);
    assert.equal(validateOAuthState(state, state.value, 6001), false);
    assert.equal(validateOAuthState(state, state.value, 4999), false);
  });

  it("generates RFC 7636-compatible S256 PKCE values", async () => {
    const pair = await createPkcePair();
    assert.match(pair.verifier, /^[A-Za-z0-9_-]{43}$/);
    assert.match(pair.challenge, /^[A-Za-z0-9_-]{43}$/);
    assert.equal(pair.method, "S256");
    const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(pair.verifier));
    const expected = btoa(String.fromCharCode(...new Uint8Array(digest)))
      .replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/g, "");
    assert.equal(pair.challenge, expected);
  });

  it("normalizes accounts and filters unknown capabilities", () => {
    const account = normalizeOAuthAccount({
      provider: " Example ", subject: " user-1 ", displayName: " User ",
      capabilities: ["identity", "identity", "admin", "playlists_read"],
      connectedAt: "2025-01-01T00:00:00Z",
    });
    assert.deepEqual(account, {
      provider: "example", subject: "user-1", displayName: "User", avatarUrl: null,
      capabilities: ["identity", "playlists_read"], connectedAt: "2025-01-01T00:00:00.000Z",
    });
    assert.equal(hasOAuthCapability(account, "playlists_read"), true);
    assert.equal(hasOAuthCapability(account, "playlists_write"), false);
  });

  it("supports one-time pending sessions without persistence", async () => {
    const store = new OAuthPendingSessionStore();
    const session = await store.begin(" Spotify ", "https://app.example/callback", 1000, 5000);
    assert.equal(session.provider, "spotify");
    assert.equal(session.redirectUri, "https://app.example/callback");
    assert.equal(store.consume(session.state.value, 5000)?.pkce.verifier, session.pkce.verifier);
    assert.equal(store.consume(session.state.value, 5000), null);
  });

  it("rejects expired or insecure authorization sessions", async () => {
    const store = new OAuthPendingSessionStore();
    await assert.rejects(store.begin("spotify", "javascript:alert(1)"), /redirect URI/);
    const session = await store.begin("spotify", "http://localhost/callback", 1000, 5000);
    assert.equal(store.consume(session.state.value, 6001), null);
  });

  it("parses success and provider error callback results", () => {
    assert.deepEqual(parseOAuthCallback("https://app/cb?code=abc&state=xyz"), { kind: "success", code: "abc", state: "xyz" });
    assert.deepEqual(parseOAuthCallback("https://app/cb?error=access_denied&error_description=Nope&state=xyz"), { kind: "error", error: "access_denied", description: "Nope", state: "xyz" });
    assert.throws(() => parseOAuthCallback("https://app/cb?code=abc"), /missing code or state/);
  });

  it("rejects malformed identity, state TTL, and timestamps", () => {
    assert.throws(() => createOAuthState(0), /TTL/);
    assert.throws(() => normalizeOAuthAccount({ provider: " ", subject: "x" }), /provider/);
    assert.throws(() => normalizeOAuthAccount({ provider: "x", subject: "y", connectedAt: "invalid" }), /connectedAt/);
  });
});
