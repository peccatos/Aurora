import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { activeLineIndex, getLyrics, parseLrc, parseLyricsText } from "../src/lyrics-service.ts";
import { validateVideoUrl } from "../src/video-url.ts";
import { importDuplicateKey, validateAudioFile } from "../src/audio-import.ts";

describe("lyrics parsing", () => {
  it("expands multiple timestamps and sorts timed lines", () => {
    assert.deepEqual(parseLrc("[00:10.50]later\n[00:02]early\n[00:02.25][00:04]shared"), [
      { timeS: 2, text: "early" },
      { timeS: 2.25, text: "shared" },
      { timeS: 4, text: "shared" },
      { timeS: 10.5, text: "later" },
    ]);
    assert.equal(activeLineIndex(parseLrc("[00:02]early\n[00:04]late"), 3.8), 0);
  });

  it("parses plain text without creating fake timestamps", () => {
    assert.deepEqual(parseLyricsText("first\nsecond"), [
      { timeS: null, text: "first" },
      { timeS: null, text: "second" },
    ]);
  });

  it("fetches and selects a usable network result when storage is unavailable", async () => {
    const originalFetch = globalThis.fetch;
    globalThis.fetch = async () => new Response(JSON.stringify([
      { id: 1, trackName: "Other", artistName: "Other", duration: 100, instrumental: false, plainLyrics: "wrong", syncedLyrics: null },
      { id: 2, trackName: "Song", artistName: "Artist", duration: 180, instrumental: false, plainLyrics: null, syncedLyrics: "[00:01.00]hello" },
    ]), { status: 200, headers: { "content-type": "application/json" } });
    try {
      const result = await getLyrics("Artist", "Song", 180);
      assert.deepEqual(result?.lines, [{ timeS: 1, text: "hello" }]);
      assert.equal(result?.synced, true);
    } finally {
      globalThis.fetch = originalFetch;
    }
  });

  it("returns null for malformed or unsuccessful lyric responses", async () => {
    const originalFetch = globalThis.fetch;
    globalThis.fetch = async (input) => new Response(String(input).includes("malformed") ? "not-json" : "", { status: String(input).includes("malformed") ? 200 : 503 });
    try {
      assert.equal(await getLyrics("Artist", "Malformed", null), null);
      assert.equal(await getLyrics("Artist", "Unavailable", null), null);
    } finally {
      globalThis.fetch = originalFetch;
    }
  });
});

describe("safe video URLs", () => {
  it("allows direct HTTPS YouTube and Vimeo video URLs", () => {
    assert.equal(validateVideoUrl("https://www.youtube.com/watch?v=x"), null);
    assert.equal(validateVideoUrl("https://youtu.be/x?t=30"), null);
    assert.equal(validateVideoUrl("https://vimeo.com/123456"), null);
    assert.equal(validateVideoUrl("http://youtube.com/watch?v=x"), "Video URL must use HTTPS");
    assert.equal(validateVideoUrl("https://evil.youtube.com/watch?v=x"), "Video host is not supported");
    assert.equal(validateVideoUrl("https://youtube.com@evil.example/"), "Credentials are not allowed in video URLs");
    assert.equal(validateVideoUrl("https://youtube.com/"), "Enter a direct video URL");
    assert.equal(validateVideoUrl("https://vimeo.com/channels/staffpicks"), "Enter a direct video URL");
    assert.equal(validateVideoUrl("https://www.youtube.com/watch"), "Enter a direct video URL");
    assert.equal(validateVideoUrl("//www.youtube.com/watch?v=x"), "Enter a complete HTTPS video URL");
    assert.equal(validateVideoUrl(`https://youtu.be/${"x".repeat(500)}`), "Video URL must be 500 characters or fewer");
    assert.equal(validateVideoUrl(""), null);
  });
});

describe("audio import validation", () => {
  it("rejects empty/oversized/unsupported files and deduplicates stable file identity", () => {
    assert.equal(validateAudioFile({ name: "x.mp3", size: 0, type: "audio/mpeg" }), "The audio file is empty");
    assert.equal(validateAudioFile({ name: "x.exe", size: 10, type: "audio/unknown" }), "Unsupported audio format");
    assert.equal(validateAudioFile({ name: "x.mp3", size: 512 * 1024 * 1024 + 1, type: "audio/mpeg" }), "Audio files must be 512 MB or smaller");
    assert.equal(importDuplicateKey({ name: "A.MP3", size: 4, lastModified: 2 }), importDuplicateKey({ name: "a.mp3", size: 4, lastModified: 2 }));
  });
});
