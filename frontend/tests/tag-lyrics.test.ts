import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { detectLyricsLanguage, extractLyricsText } from "../src/tag-reader.ts";
import { pairLrcSidecars, pairLrcSidecarsWithText } from "../src/lrc-sidecar.ts";
import { parseLrc } from "../src/lyrics-service.ts";

function fakeFile(name: string, text = ""): File {
  return new File([text], name, { type: "text/plain" });
}

describe("embedded lyrics extraction", () => {
  it("uses plain USLT text when present", () => {
    assert.equal(extractLyricsText([{ text: "  verse\nchorus  " }]), "verse\nchorus");
    assert.equal(extractLyricsText([{ text: "" }, { text: null }]), null);
    assert.equal(extractLyricsText([]), null);
  });

  it("converts syncText entries into LRC with timestamps", () => {
    const out = extractLyricsText([
      { syncText: [
        { text: "first", timestamp: 1.5 },
        { text: "second", timestamp: 62 },
      ] },
    ]);
    assert.equal(out, "[00:01.50]first\n[01:02.00]second");
    // The player's LRC parser round-trips it back to timed lines.
    const lines = parseLrc(out ?? "");
    assert.deepEqual(lines.map((l) => l.timeS), [1.5, 62]);
    assert.deepEqual(lines.map((l) => l.text), ["first", "second"]);
  });

  it("prefers text over syncText in the same tag", () => {
    assert.equal(
      extractLyricsText([{ text: "plain", syncText: [{ text: "synced" }] }]),
      "plain",
    );
  });

  it("detects Cyrillic vs Latin scripts", () => {
    assert.equal(detectLyricsLanguage("Привет, как дела"), "ru");
    assert.equal(detectLyricsLanguage("Hello my friend"), "en");
    assert.equal(detectLyricsLanguage("♪ ♪ ♪"), null);
    assert.equal(detectLyricsLanguage("Привет Hello Привет"), "ru");
  });
});

describe("LRC sidecar pairing", () => {
  it("pairs same-stem audio and .lrc files; different stems never match", () => {
    const audio = [fakeFile("song.flac"), fakeFile("other.mp3")];
    const all = [...audio, fakeFile("song.lrc"), fakeFile("different.lrc")];
    const pairs = pairLrcSidecars(audio, all);
    assert.deepEqual([...pairs.keys()], ["song.flac"]);
    assert.equal(pairs.get("song.flac")?.name, "song.lrc");

    const noMatch = pairLrcSidecars([fakeFile("song.flac")], [fakeFile("song.flac"), fakeFile("other.lrc")]);
    assert.equal(noMatch.size, 0);
  });

  it("matches stems case-insensitively and one .lrc serves one track", () => {
    const audio = [fakeFile("Song.FLAC"), fakeFile("song.mp3")];
    const all = [...audio, fakeFile("SONG.lrc")];
    const pairs = pairLrcSidecars(audio, all);
    assert.equal(pairs.size, 1);
    assert.equal(pairs.get("Song.FLAC")?.name, "SONG.lrc");
  });

  it("reads paired sidecar text and stores it as the track's lyrics source", async () => {
    const lrcBody = "[00:01.00]hello\n[00:02.00]world";
    const audio = [fakeFile("song.flac")];
    const all = [...audio, fakeFile("song.lrc", lrcBody), fakeFile("no-text.lrc", "   ")];
    const withText = await pairLrcSidecarsWithText(audio, all);
    assert.deepEqual([...withText.entries()], [["song.flac", lrcBody]]);
    // parseLrc renders it — the player's synced view works out of the box.
    assert.equal(parseLrc(withText.get("song.flac") ?? "").length, 2);
  });

  it("skips unreadable sidecar files without failing the ingest", async () => {
    const audio = [fakeFile("song.flac")];
    const all = [...audio, fakeFile("song.lrc")];
    const readFile = async (): Promise<string> => { throw new Error("read error"); };
    const withText = await pairLrcSidecarsWithText(audio, all, readFile);
    assert.equal(withText.size, 0);
  });
});
