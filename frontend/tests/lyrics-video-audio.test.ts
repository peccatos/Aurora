import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { activeLineIndex, parseLrc, parseLyricsText } from "../src/lyrics-service.ts";
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
});

describe("safe video URLs", () => {
  it("allows only exact HTTPS YouTube and Vimeo hosts", () => {
    assert.equal(validateVideoUrl("https://www.youtube.com/watch?v=x"), null);
    assert.equal(validateVideoUrl("http://youtube.com/watch?v=x"), "Video URL must use HTTPS");
    assert.equal(validateVideoUrl("https://evil.youtube.com/watch?v=x"), "Video host is not supported");
    assert.equal(validateVideoUrl("https://youtube.com@evil.example/"), "Credentials are not allowed in video URLs");
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
