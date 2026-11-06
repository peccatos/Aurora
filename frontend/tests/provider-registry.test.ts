import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { canSearch, getProvider, PROVIDERS, searchWithAdapter, type ProviderAdapter } from "../src/provider-registry.ts";

void describe("provider capability registry", () => {
  it("declares public metadata/preview and OAuth-required providers", () => {
    assert.equal(getProvider("itunes").availability, "available");
    assert.deepEqual(getProvider("itunes").capabilities, ["metadata-search", "preview-playback"]);
    assert.equal(getProvider("musicbrainz").capabilities.includes("preview-playback"), false);
    assert.equal(getProvider("spotify").availability, "oauth-required");
    assert.equal(getProvider("youtube-music").availability, "oauth-required");
    assert.equal(getProvider("soundcloud").availability, "unsupported");
    assert.equal(PROVIDERS.length, 7);
  });

  it("fails closed for OAuth-required and unsupported adapters", async () => {
    const adapter: ProviderAdapter = { provider: "spotify", search: async () => [] };
    await assert.rejects(searchWithAdapter(adapter, "track"), /oauth-required/);
    await assert.rejects(searchWithAdapter({ provider: "soundcloud", search: async () => [] }, "track"), /unsupported/);
    assert.deepEqual(await searchWithAdapter(adapter, "   "), []);
  });

  it("allows only explicitly searchable capabilities", () => {
    assert.equal(canSearch(getProvider("itunes")), true);
    assert.equal(canSearch(getProvider("musicbrainz")), true);
    assert.equal(canSearch(getProvider("spotify")), false);
  });
});
