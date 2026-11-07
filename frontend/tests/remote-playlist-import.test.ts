import { describe, it } from "node:test";
import assert from "node:assert/strict";
import {
  applyRemotePlaylistImport,
  importRemotePlaylist,
  matchRemoteTracks,
  summarizeMatchCounters,
} from "../src/remote-playlist-import.ts";
import { localTrack, playlistRecorder, remoteTrack, snapshot } from "./remote-playlist-fixtures.ts";

void describe("remote playlist import report counters", () => {
  it("accounts every source position exactly once", () => {
    const local = [
      localTrack("a1", "Matched One", "Artist"),
      localTrack("b2", "Matched Two", "Artist"),
    ];
    const result = matchRemoteTracks(
      snapshot([
        remoteTrack("r1", "Matched One", ["Artist"], 0),
        remoteTrack("r2", "Nowhere", ["Nobody"], 1),
        remoteTrack("r3", "Matched Two", ["Artist"], 2),
        remoteTrack("r4", "Also Nowhere", ["Nobody"], 3),
      ]),
      local,
    );
    const counters = summarizeMatchCounters(result, 4);

    assert.equal(counters.totalRemoteTracks, 4);
    assert.equal(counters.matched, 2);
    assert.equal(counters.unmatched, 2);
    assert.equal(counters.duplicateMatches, 0);
    assert.equal(counters.uniqueLocalTracks, 2);
    assert.equal(counters.matched + counters.unmatched, counters.totalRemoteTracks);
  });

  it("counts a repeated song per position and reports both totals", () => {
    const local = [localTrack("a1", "Song", "Artist"), localTrack("b2", "Song", "Artist")];
    const result = matchRemoteTracks(
      snapshot([
        remoteTrack("r1", "Song", ["Artist"], 0),
        remoteTrack("r2", "Song", ["Artist"], 1),
        remoteTrack("r3", "Song", ["Artist"], 2),
        remoteTrack("r4", "Song", ["Artist"], 3),
        remoteTrack("r5", "Missing", ["Artist"], 4),
      ]),
      local,
    );
    const counters = summarizeMatchCounters(result, 5);

    assert.equal(counters.matched, 4, "one entry per matched source position");
    assert.equal(counters.duplicateMatches, 2, "positions 2 and 3 reuse an id");
    assert.equal(counters.uniqueLocalTracks, 2, "only two local tracks exist");
    assert.equal(counters.unmatched, 1);
    assert.equal(counters.totalRemoteTracks, 5);
    assert.equal(counters.matched + counters.unmatched, counters.totalRemoteTracks);
  });

  it("reports ambiguity as a subset of matched, not as a separate bucket", () => {
    const local = [localTrack("a1", "Song", "Artist"), localTrack("b2", "Song", "Artist")];
    const result = matchRemoteTracks(snapshot([remoteTrack("r1", "Song", ["Artist"], 0)]), local);
    const counters = summarizeMatchCounters(result, 1);

    assert.equal(counters.ambiguous, 1);
    assert.equal(counters.matched, 1);
    assert.equal(counters.matched + counters.unmatched, counters.totalRemoteTracks);
  });

  it("reports an all-unmatched source with zero matches and no local ids", async () => {
    const local = [localTrack("a1", "Something Else", "Nobody")];
    const recorder = playlistRecorder();
    const report = await importRemotePlaylist(
      snapshot([
        remoteTrack("r1", "Nowhere One", ["Nobody"], 0),
        remoteTrack("r2", "Nowhere Two", ["Nobody"], 1),
        remoteTrack("r3", "Nowhere Three", ["Nobody"], 2),
      ]),
      { localTracks: local, ...recorder },
    );

    assert.equal(report.totalRemoteTracks, 3);
    assert.equal(report.matched, 0);
    assert.equal(report.unmatched, 3);
    assert.equal(report.duplicateMatches, 0);
    assert.equal(report.uniqueLocalTracks, 0);
    assert.equal(report.ambiguous, 0);
    assert.deepEqual(report.localTrackIds, []);
    assert.deepEqual(report.matchedEntries, []);
    assert.equal(report.matched + report.unmatched, report.totalRemoteTracks);
    assert.deepEqual(
      report.unmatchedLabels,
      ["Nobody — Nowhere One", "Nobody — Nowhere Two", "Nobody — Nowhere Three"],
    );
    assert.deepEqual(
      report.unmatchedEntries.map((entry) => entry.sourcePosition),
      [0, 1, 2],
    );
    // An all-unmatched import still creates the (empty) playlist so the
    // report is actionable rather than a silent failure.
    assert.deepEqual(recorder.created, [{ id: "local-1", name: "Road trip" }]);
    assert.deepEqual(recorder.added, [{ playlistId: "local-1", trackIds: [] }]);
    assert.deepEqual(report.playlist.trackIds, []);
  });

  it("labels an unmatched entry by title alone when the provider has no artist", async () => {
    const recorder = playlistRecorder();
    const report = await importRemotePlaylist(
      snapshot([remoteTrack("r1", "Untitled Sketch", [], 0)]),
      { localTracks: [], ...recorder },
    );
    assert.deepEqual(report.unmatchedLabels, ["Untitled Sketch"]);
  });

  it("counters survive an empty source playlist", async () => {
    const recorder = playlistRecorder();
    const report = await importRemotePlaylist(snapshot([]), { localTracks: [], ...recorder });
    assert.equal(report.totalRemoteTracks, 0);
    assert.equal(report.matched, 0);
    assert.equal(report.unmatched, 0);
    assert.equal(report.matched + report.unmatched, report.totalRemoteTracks);
  });
});

void describe("local playlist creation from a remote snapshot", () => {
  it("writes matched tracks in source order, de-duplicated", async () => {
    const local = [
      localTrack("a1", "One", "Artist"),
      localTrack("b2", "Two", "Artist"),
      localTrack("c3", "Three", "Artist"),
    ];
    const recorder = playlistRecorder();
    const report = await importRemotePlaylist(
      snapshot([
        remoteTrack("r3", "Three", ["Artist"], 2),
        remoteTrack("r1", "One", ["Artist"], 0),
        remoteTrack("r2", "Two", ["Artist"], 1),
        remoteTrack("r4", "Three", ["Artist"], 3),
      ]),
      { localTracks: local, ...recorder },
    );

    assert.deepEqual(
      report.matchedEntries.map((entry) => entry.sourcePosition),
      [0, 1, 2, 3],
    );
    assert.deepEqual(report.localTrackIds, ["a1", "b2", "c3"]);
    assert.deepEqual(recorder.added, [{ playlistId: "local-1", trackIds: ["a1", "b2", "c3"] }]);
    assert.deepEqual(report.playlist.trackIds, ["a1", "b2", "c3"]);
    assert.equal(report.duplicateMatches, 1);
  });

  it("records the source provider, playlist id and revision on the playlist", async () => {
    const recorder = playlistRecorder();
    const report = await importRemotePlaylist(
      snapshot([remoteTrack("r1", "One", ["Artist"], 0)], {
        provider: "youtube-music",
        id: "PL-42",
        revision: "rev-7",
      }),
      { localTracks: [localTrack("a1", "One", "Artist")], ...recorder },
    );

    assert.equal(report.sourceProvider, "youtube-music");
    assert.equal(report.sourcePlaylistId, "PL-42");
    assert.equal(report.sourceRevision, "rev-7");
    assert.equal(report.playlist.source?.provider, "youtube-music");
    assert.equal(report.playlist.source?.playlistId, "PL-42");
    assert.equal(report.playlist.source?.revision, "rev-7");
    assert.equal(report.playlist.source?.lastSyncedAt, null);
    assert.equal(typeof report.playlist.source?.importedAt, "number");
  });

  it("uses the requested name, then the remote name, then a fallback", async () => {
    const cases = [
      { requested: " My mix ", remote: "Road trip", expected: "My mix" },
      { requested: "   ", remote: "Road trip", expected: "Road trip" },
      { requested: undefined, remote: "   ", expected: "Imported playlist" },
    ];
    for (const item of cases) {
      const recorder = playlistRecorder();
      const report = await applyRemotePlaylistImport(
        snapshot([], { name: item.remote }),
        { name: item.requested, localTracks: [], ...recorder },
      );
      assert.equal(report.playlist.name, item.expected);
    }
  });

  it("reports every position unmatched when the local library is unreadable", async () => {
    // No localTracks option: the import reads the app store, which is
    // unavailable under the test runner. Storage is degradable by design,
    // so this must degrade to an all-unmatched report, not throw.
    const recorder = playlistRecorder();
    const report = await importRemotePlaylist(
      snapshot([remoteTrack("r1", "One", ["Artist"], 0)]),
      recorder,
    );
    assert.equal(report.matched, 0);
    assert.equal(report.unmatched, 1);
    assert.equal(report.totalRemoteTracks, 1);
    assert.deepEqual(report.unmatchedLabels, ["Artist — One"]);
  });
});
