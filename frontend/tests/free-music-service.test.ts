import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { fetchFreeTrackBytes, hasMp3Signature, isCreativeCommonsLicense, validateArchiveDownloadUrl } from "../src/free-music-service.ts";

describe("safe Archive.org full-track imports", () => {
  it("accepts only the exact HTTPS download origin and item path", () => {
    assert.equal(validateArchiveDownloadUrl("https://archive.org/download/item/song.mp3", "item"), null);
    assert.equal(validateArchiveDownloadUrl("http://archive.org/download/item/song.mp3", "item"), "Audio source must use HTTPS");
    assert.equal(validateArchiveDownloadUrl("https://evil.example/download/item/song.mp3", "item"), "Audio source host is not allowed");
    assert.equal(validateArchiveDownloadUrl("https://archive.org/download/other/song.mp3", "item"), "Audio source path is not an archive download");
    assert.equal(validateArchiveDownloadUrl("https://archive.org/download/item/song.mp3?redirect=evil", "item"), "Audio source must not contain query or fragment data");
  });

  it("requires an explicit Creative Commons license", () => {
    assert.equal(isCreativeCommonsLicense("https://creativecommons.org/licenses/by/4.0/"), true);
    assert.equal(isCreativeCommonsLicense(null), false);
    assert.equal(isCreativeCommonsLicense("https://example.test/license"), false);
  });

  it("recognizes ID3 and MPEG frame signatures, but rejects unrelated content", () => {
    assert.equal(hasMp3Signature(new Uint8Array([0x49, 0x44, 0x33, 0, 0])), true);
    assert.equal(hasMp3Signature(new Uint8Array([0, 0xff, 0xfb, 0x90])), true);
    assert.equal(hasMp3Signature(new TextEncoder().encode("<html>not audio</html>")), false);
  });

  it("rejects HTML and oversized responses before importing", async () => {
    const hit = { identifier: "item", title: "Track", creator: "Artist", file: "song.mp3", durationS: null, license: "https://creativecommons.org/licenses/by/4.0/", itemUrl: "https://archive.org/details/item" };
    const originalFetch = globalThis.fetch;
    try {
      globalThis.fetch = async () => new Response("<html>error</html>", { headers: { "content-type": "text/html" } });
      await assert.rejects(fetchFreeTrackBytes(hit, "https://archive.org/download/item/song.mp3"), /unexpected content type/);
      globalThis.fetch = async () => new Response(new Uint8Array([0x49, 0x44, 0x33]), { headers: { "content-type": "audio/mpeg", "content-length": String(512 * 1024 * 1024 + 1) } });
      await assert.rejects(fetchFreeTrackBytes(hit, "https://archive.org/download/item/song.mp3"), /larger than 512 MB/);
    } finally {
      globalThis.fetch = originalFetch;
    }
  });

  it("rejects missing or malformed licenses before making a request", async () => {
    const hit = { identifier: "item", title: "Track", creator: null, file: "song.mp3", durationS: null, license: null, itemUrl: "https://archive.org/details/item" };
    await assert.rejects(fetchFreeTrackBytes(hit, "https://archive.org/download/item/song.mp3"), /license attribution is missing/);
  });
});
