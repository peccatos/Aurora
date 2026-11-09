import { describe, it, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import {
  summarizeImportQuality,
  matchRatePercent,
  readImportHistory,
  appendImportHistory,
  recordImportQuality,
  clearImportHistory,
  HISTORY_KEY,
  HISTORY_CAP,
  UNMATCHED_CAP,
  type ImportQualityHistoryEntry,
} from "../src/import-metrics.ts";
import { importRemotePlaylist } from "../src/remote-playlist-import.ts";
import { localTrack, playlistRecorder, remoteTrack, snapshot } from "./remote-playlist-fixtures.ts";

/** Minimal Storage stub for Node (there is no DOM). */
function stubStorage(): { store: Map<string, string> } & Storage {
  const store = new Map<string, string>();
  const impl: Storage = {
    get length() {
      return store.size;
    },
    clear: () => store.clear(),
    getItem: (key) => (store.has(key) ? store.get(key)! : null),
    key: (index) => [...store.keys()][index] ?? null,
    removeItem: (key) => void store.delete(key),
    setItem: (key, value) => void store.set(key, value),
  };
  return Object.assign(impl, { store });
}

function matchEntry(
  remoteId: string,
  title: string,
  artists: string[],
  position: number,
  kind: "exact" | "artist" | "title_only",
  localTrackId: string,
) {
  return {
    remote: remoteTrack(remoteId, title, artists, position),
    sourcePosition: position,
    localTrackId,
    matchKind: kind,
    duplicate: false,
  };
}

void describe("summarizeImportQuality", () => {
  it("returns zeroed counters and rate 0 for an empty playlist", () => {
    const report = summarizeImportQuality({ matched: [], unmatched: [] });
    assert.equal(report.totalRemote, 0);
    assert.equal(report.matched, 0);
    assert.equal(report.unmatched, 0);
    assert.deepEqual(report.matchedByKind, { exact: 0, artist: 0, title_only: 0 });
    assert.deepEqual(report.unmatchedTracks, []);
    assert.equal(report.matchRate, 0);
  });

  it("reports rate 1 for an all-matched playlist and breaks kinds down", () => {
    const report = summarizeImportQuality({
      matched: [
        matchEntry("r1", "A", ["X"], 0, "exact", "a1"),
        matchEntry("r2", "B", ["X"], 1, "artist", "b2"),
        matchEntry("r3", "C", ["X"], 2, "title_only", "c3"),
        matchEntry("r4", "D", ["X"], 3, "exact", "a1"),
      ],
      unmatched: [],
    });
    assert.equal(report.matched, 4);
    assert.deepEqual(report.matchedByKind, { exact: 2, artist: 1, title_only: 1 });
    assert.equal(report.matchRate, 1);
    assert.equal(matchRatePercent(report), "100%");
  });

  it("computes the rate for a mixed playlist and labels unmatched tracks", () => {
    const report = summarizeImportQuality({
      matched: [matchEntry("r1", "A", ["X"], 0, "exact", "a1")],
      unmatched: [
        { remote: remoteTrack("r2", "Missing", ["Y", "Z"], 1), sourcePosition: 1 },
        { remote: remoteTrack("r3", "No Artist", [], 2), sourcePosition: 2 },
      ],
    });
    assert.equal(report.totalRemote, 3);
    assert.equal(report.matched, 1);
    assert.equal(report.unmatched, 2);
    assert.equal(report.matchRate, 1 / 3);
    assert.deepEqual(report.unmatchedTracks, [
      { title: "Missing", artists: "Y, Z" },
      { title: "No Artist", artists: "" },
    ]);
  });

  it("caps the unmatched list at the module cap", () => {
    const unmatched = Array.from({ length: UNMATCHED_CAP + 30 }, (_, i) => ({
      remote: remoteTrack(`r${i}`, `Track ${i}`, [], i),
      sourcePosition: i,
    }));
    const report = summarizeImportQuality({ matched: [], unmatched });
    assert.equal(report.unmatched, unmatched.length);
    assert.equal(report.unmatchedTracks.length, UNMATCHED_CAP);
    assert.equal(report.unmatchedTracks[UNMATCHED_CAP - 1].title, `Track ${UNMATCHED_CAP - 1}`);
  });

  it("prefers an explicit totalRemoteTracks over the entry sums", () => {
    const report = summarizeImportQuality({ matched: [], unmatched: [] }, 7);
    assert.equal(report.totalRemote, 7);
    assert.equal(report.matchRate, 0);
  });
});

void describe("import quality history (localStorage)", () => {
  let storage: { store: Map<string, string> } & Storage;

  beforeEach(() => {
    storage = stubStorage();
    (globalThis as { localStorage?: Storage }).localStorage = storage;
  });
  afterEach(() => {
    delete (globalThis as { localStorage?: Storage }).localStorage;
  });

  it("reads an empty history as [] when storage is unset or malformed", () => {
    assert.deepEqual(readImportHistory(), []);
    storage.setItem(HISTORY_KEY, "{not json");
    assert.deepEqual(readImportHistory(), []);
    storage.setItem(HISTORY_KEY, JSON.stringify({ no: "array" }));
    assert.deepEqual(readImportHistory(), []);
    // Malformed entries are dropped, valid ones survive.
    storage.setItem(
      HISTORY_KEY,
      JSON.stringify([
        { date: "not-a-number", provider: "spotify", playlistName: "X", report: {} },
        "garbage",
      ]),
    );
    assert.deepEqual(readImportHistory(), []);
  });

  it("appends newest-first and caps the history at HISTORY_CAP", () => {
    let history: ImportQualityHistoryEntry[] = [];
    for (let i = 0; i < HISTORY_CAP + 5; i++) {
      history = appendImportHistory(history, {
        date: i,
        provider: "spotify",
        playlistName: `p${i}`,
        report: summarizeImportQuality({ matched: [], unmatched: [] }),
      });
    }
    assert.equal(history.length, HISTORY_CAP);
    assert.equal(history[0].playlistName, `p${HISTORY_CAP + 4}`);
    // appendImportHistory is pure: writing happens in recordImportQuality.
    assert.equal(storage.getItem(HISTORY_KEY), null);
  });

  it("recordImportQuality persists one entry and rereads it back", () => {
    const entry = recordImportQuality(
      {
        matched: [matchEntry("r1", "A", ["X"], 0, "exact", "a1")],
        unmatched: [{ remote: remoteTrack("r2", "B", ["Y"], 1), sourcePosition: 1 }],
      },
      { provider: "youtube-music", playlistName: "Mix", date: 1234 },
    );
    assert.equal(entry.date, 1234);
    const history = readImportHistory();
    assert.equal(history.length, 1);
    assert.equal(history[0].provider, "youtube-music");
    assert.equal(history[0].playlistName, "Mix");
    assert.equal(history[0].report.matched, 1);
    assert.equal(history[0].report.unmatched, 1);
    assert.equal(history[0].report.matchRate, 0.5);
    assert.equal(history[0].report.unmatchedTracks.length, 1);
  });

  it("rotates the history on overflow (oldest entry dropped)", () => {
    for (let i = 0; i < HISTORY_CAP + 1; i++) {
      recordImportQuality(
        { matched: [], unmatched: [] },
        { provider: "spotify", playlistName: `p${i}`, date: i },
      );
    }
    const history = readImportHistory();
    assert.equal(history.length, HISTORY_CAP);
    assert.equal(history[0].playlistName, `p${HISTORY_CAP}`);
    assert.equal(history[HISTORY_CAP - 1].playlistName, "p1");
  });

  it("clearImportHistory empties storage", () => {
    recordImportQuality({ matched: [], unmatched: [] }, { provider: "spotify", playlistName: "X" });
    clearImportHistory();
    assert.deepEqual(readImportHistory(), []);
  });

  it("survives absence of localStorage without throwing", () => {
    delete (globalThis as { localStorage?: Storage }).localStorage;
    assert.deepEqual(readImportHistory(), []);
    const entry = recordImportQuality({ matched: [], unmatched: [] }, { provider: "spotify", playlistName: "X" });
    assert.equal(entry.report.totalRemote, 0);
  });
});

void describe("flow-level quality wiring", () => {
  it("applyRemotePlaylistImport/importRemotePlaylist emits the quality report", async () => {
    const local = [
      localTrack("a1", "Exact Hit", "Artist"),
      // A single remote artist string carrying list punctuation only
      // meets the identical local credit through the separator-keeping
      // key (byTitleArtist is built with the folding key) -> "artist".
      localTrack("b2", "Artist Hit", "A, B"),
      localTrack("c3", "Title Hit", "Someone Else"),
    ];
    const recorder = playlistRecorder();
    const report = await importRemotePlaylist(
      snapshot([
        remoteTrack("r1", "Exact Hit", ["Artist"], 0),
        remoteTrack("r2", "Artist Hit", ["A, B"], 1),
        remoteTrack("r3", "Title Hit", ["Anyone"], 2),
        remoteTrack("r4", "Missing", ["Nobody"], 3),
      ]),
      { localTracks: local, ...recorder },
    );
    assert.equal(report.quality.totalRemote, 4);
    assert.equal(report.quality.matched, 3);
    assert.equal(report.quality.unmatched, 1);
    assert.deepEqual(report.quality.matchedByKind, { exact: 1, artist: 1, title_only: 1 });
    assert.equal(report.quality.matchRate, 0.75);
    assert.deepEqual(report.quality.unmatchedTracks, [{ title: "Missing", artists: "Nobody" }]);
  });
});
