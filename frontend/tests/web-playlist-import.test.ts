import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { parseM3u, parsePls, parsePlaylistFile } from "../src/web-playlist-import.ts";

void describe("web playlist parsers", () => {
  it("reads M3U EXTINF metadata and filename-only rows", () => {
    assert.deepEqual(parseM3u("\uFEFF#EXTM3U\n#EXTINF:210,Artist - Song\n/music/Artist - Song.mp3\n#EXTINF:bad\n/other/Loose title.flac"), [
      { title: "Song", artist: "Artist", location: "/music/Artist - Song.mp3" },
      { title: "Loose title", artist: null, location: "/other/Loose title.flac" },
    ]);
  });

  it("reads PLS rows ordered numerically", () => {
    assert.deepEqual(parsePls("[playlist]\nFile2=b.mp3\nTitle2=Artist - Second\nFile1=a.mp3\nTitle1=First\nVersion=2"), [
      { title: "First", artist: null, location: "a.mp3" },
      { title: "Second", artist: "Artist", location: "b.mp3" },
    ]);
  });

  it("reads Spotify-style nested JSON and quoted CSV", () => {
    assert.deepEqual(parsePlaylistFile("export.json", JSON.stringify({ items: [{ track: { name: "Song", artists: [{ name: "A, B" }], uri: "spotify:track:x" } }] })), [
      { title: "Song", artist: "A, B", location: "spotify:track:x" },
    ]);
    assert.deepEqual(parsePlaylistFile("export.csv", 'Track Name,Artist,URI\n"Song, Live","A, B",spotify:track:x'), [
      { title: "Song, Live", artist: "A, B", location: "spotify:track:x" },
    ]);
    assert.deepEqual(parsePlaylistFile("multiline.csv", 'Title,Artist\n"Song\nLive",Artist'), [
      { title: "Song\nLive", artist: "Artist", location: null },
    ]);
  });

  it("returns empty for empty input and rejects malformed or unsupported files", () => {
    assert.deepEqual(parsePlaylistFile("empty.m3u", "#EXTM3U\n"), []);
    assert.throws(() => parsePlaylistFile("broken.json", "{"));
    assert.throws(() => parsePlaylistFile("list.xml", "<list/>"), /Unsupported/);
  });
});
