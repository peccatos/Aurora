import { describe, it } from "node:test";
import assert from "node:assert/strict";
import {
  matchRemoteTracks,
  normalizeMatchKey,
  resolveRemoteTrackCandidates,
  buildLocalTrackIndex,
} from "../src/remote-playlist-import.ts";
import { localTrack, remoteTrack, snapshot } from "./remote-playlist-fixtures.ts";

void describe("remote playlist matching with duplicates", () => {
  it("counts one matched entry per source position, not per local id", () => {
    const local = [localTrack("a1", "Song", "Artist")];
    const result = matchRemoteTracks(
      snapshot([
        remoteTrack("r1", "Song", ["Artist"], 0),
        remoteTrack("r2", "Song", ["Artist"], 1),
        remoteTrack("r3", "Song", ["Artist"], 2),
      ]),
      local,
    );

    assert.equal(result.matched.length, 3);
    assert.deepEqual(
      result.matched.map((entry) => entry.sourcePosition),
      [0, 1, 2],
    );
    assert.deepEqual(
      result.matched.map((entry) => entry.localTrackId),
      ["a1", "a1", "a1"],
    );
    assert.deepEqual(
      result.matched.map((entry) => entry.duplicate),
      [false, true, true],
    );
    assert.equal(result.unmatched.length, 0);
    assert.equal(result.ambiguous.length, 0);
  });

  it("spreads a repeated remote entry over the local copies before reusing one", () => {
    const local = [
      localTrack("z9", "Song", "Artist"),
      localTrack("a1", "Song", "Artist"),
      localTrack("b2", "Song", "Artist"),
    ];
    const result = matchRemoteTracks(
      snapshot([
        remoteTrack("r1", "Song", ["Artist"], 0),
        remoteTrack("r2", "Song", ["Artist"], 1),
        remoteTrack("r3", "Song", ["Artist"], 2),
      ]),
      local,
    );

    // One local copy per source position while unused copies remain, then
    // the deterministic lowest-id copy for the overflow.
    assert.deepEqual(
      result.matched.map((entry) => [entry.sourcePosition, entry.localTrackId, entry.duplicate]),
      [
        [0, "a1", false],
        [1, "b2", false],
        [2, "z9", false],
      ],
    );
    assert.equal(result.unmatched.length, 0);
    // Ambiguity is reported for the first position but never left as a hole.
    assert.deepEqual(
      result.ambiguous.map((entry) => entry.sourcePosition),
      [0],
    );
    assert.equal(result.ambiguous[0].candidates.length, 3);
  });

  it("reports a duplicate subset without dropping the entry from matched", () => {
    const local = [localTrack("a1", "Song", "Artist"), localTrack("b2", "Song", "Artist")];
    const result = matchRemoteTracks(
      snapshot([
        remoteTrack("r1", "Song", ["Artist"], 0),
        remoteTrack("r2", "Song", ["Artist"], 1),
        remoteTrack("r3", "Song", ["Artist"], 2),
      ]),
      local,
    );

    assert.equal(result.matched.length, 3);
    assert.deepEqual(
      result.matched.map((entry) => entry.duplicate),
      [false, false, true],
    );
    assert.equal(result.matched.filter((entry) => entry.duplicate).length, 1);
    assert.equal(result.unmatched.length, 0);
  });

  it("tracks duplicates per match kind (exact, artist, title only)", () => {
    const local = [
      localTrack("a1", "Exact Song", "Artist"),
      localTrack("b2", "Artist Song", "Artist"),
      localTrack("c3", "Bare Song", null),
    ];
    const result = matchRemoteTracks(
      snapshot([
        remoteTrack("r1", "Exact Song", ["Artist"], 0),
        remoteTrack("r2", "Artist Song", ["Artist"], 1),
        remoteTrack("r3", "Bare Song", ["Nobody"], 2),
      ]),
      local,
    );

    assert.deepEqual(
      result.matched.map((entry) => [entry.remote.remoteId, entry.matchKind]),
      [
        ["r1", "exact"],
        ["r2", "exact"],
        ["r3", "title_only"],
      ],
    );
  });

  it("preserves source order even when the provider paginates out of order", () => {
    const local = [
      localTrack("a1", "One", "Artist"),
      localTrack("b2", "Two", "Artist"),
      localTrack("c3", "Three", "Artist"),
    ];
    const result = matchRemoteTracks(
      snapshot([
        remoteTrack("r3", "Three", ["Artist"], 2),
        remoteTrack("r1", "One", ["Artist"], 0),
        remoteTrack("r2", "Two", ["Artist"], 1),
      ]),
      local,
    );

    assert.deepEqual(
      result.matched.map((entry) => [entry.sourcePosition, entry.localTrackId]),
      [
        [0, "a1"],
        [1, "b2"],
        [2, "c3"],
      ],
    );
  });

  it("matches a remote artist list against a joined local credit exactly", () => {
    const local = [localTrack("a1", "Song", "A, B")];
    const result = matchRemoteTracks(snapshot([remoteTrack("r1", "Song", ["A", "B"], 0)]), local);
    assert.deepEqual(
      result.matched.map((entry) => [entry.localTrackId, entry.matchKind]),
      [["a1", "exact"]],
    );
  });
});

void describe("remote playlist matching: deterministic resolution", () => {
  it("resolves equally good candidates to the lowest local id, every run", () => {
    const local = [
      localTrack("z9", "Song", "Artist"),
      localTrack("m5", "Song", "Artist"),
      localTrack("a1", "Song", "Artist"),
    ];
    const query = { title: "Song", artists: ["Artist"] };
    const index = buildLocalTrackIndex(local);

    const first = resolveRemoteTrackCandidates(query, index);
    assert.equal(first.kind, "exact");
    assert.deepEqual(
      first.candidates.map((candidate) => candidate.id),
      ["a1", "m5", "z9"],
    );

    // Result is stable across repeated resolutions and across input order.
    const shuffled = buildLocalTrackIndex([local[1], local[2], local[0]]);
    assert.deepEqual(
      resolveRemoteTrackCandidates(query, shuffled).candidates.map((candidate) => candidate.id),
      ["a1", "m5", "z9"],
    );
    assert.deepEqual(
      resolveRemoteTrackCandidates(query, index).candidates.map((candidate) => candidate.id),
      ["a1", "m5", "z9"],
    );
  });

  it("never lets a shorter credit steal a track owned by a fuller credit", () => {
    const local = [localTrack("a1", "Song", "A, B")];
    const result = matchRemoteTracks(snapshot([remoteTrack("r1", "Song", ["A"], 0)]), local);
    // "A" alone is not the "A, B" credit, so the entry falls back to the
    // title-only reading rather than silently claiming the feat. track.
    assert.deepEqual(
      result.matched.map((entry) => [entry.localTrackId, entry.matchKind]),
      [["a1", "title_only"]],
    );
  });

  it("prefers an exact credit over a title-only reading", () => {
    const local = [
      localTrack("a1", "Song", "Other"),
      localTrack("b2", "Song", "Artist"),
    ];
    const result = matchRemoteTracks(snapshot([remoteTrack("r1", "Song", ["Artist"], 0)]), local);
    assert.deepEqual(
      result.matched.map((entry) => [entry.localTrackId, entry.matchKind]),
      [["b2", "exact"]],
    );
  });

  it("reports ambiguity rather than guessing when every candidate is free", () => {
    const local = [localTrack("a1", "Song", "Artist"), localTrack("b2", "Song", "Artist")];
    const result = matchRemoteTracks(snapshot([remoteTrack("r1", "Song", ["Artist"], 0)]), local);

    assert.equal(result.ambiguous.length, 1);
    assert.equal(result.ambiguous[0].sourcePosition, 0);
    assert.deepEqual(
      result.ambiguous[0].candidates.map((candidate) => candidate.id),
      ["a1", "b2"],
    );
    // Reported *and* resolved, so the source position is never lost.
    assert.deepEqual(
      result.matched.map((entry) => entry.localTrackId),
      ["a1"],
    );
    assert.equal(result.unmatched.length, 0);
  });

  it("treats trailing duplicates as known duplicates, not ambiguity", () => {
    const local = [localTrack("a1", "Song", "Artist"), localTrack("b2", "Song", "Artist")];
    const result = matchRemoteTracks(
      snapshot([
        remoteTrack("r1", "Song", ["Artist"], 0),
        remoteTrack("r2", "Song", ["Artist"], 1),
        remoteTrack("r3", "Song", ["Artist"], 2),
      ]),
      local,
    );

    assert.deepEqual(
      result.matched.map((entry) => [entry.localTrackId, entry.duplicate]),
      [
        ["a1", false],
        ["b2", false],
        ["a1", true],
      ],
    );
    assert.deepEqual(
      result.ambiguous.map((entry) => entry.sourcePosition),
      [0],
    );
  });

  it("keeps local tracks with an empty title out of the index", () => {
    const index = buildLocalTrackIndex([localTrack("a1", "   ", "Artist")]);
    assert.equal(index.byTitle.size, 0);
    assert.equal(
      resolveRemoteTrackCandidates({ title: "   ", artists: ["Artist"] }, index).kind,
      null,
    );
  });
});

void describe("normalizeMatchKey", () => {
  it("folds case, punctuation and provider annotations", () => {
    assert.equal(normalizeMatchKey("Song (Remastered 2011)"), "song");
    assert.equal(normalizeMatchKey("SONG"), "song");
    assert.equal(normalizeMatchKey("Song – Live"), normalizeMatchKey("Song - Live"));
    assert.equal(normalizeMatchKey("Song, Live"), normalizeMatchKey("Song Live"));
    assert.equal(normalizeMatchKey("  "), "");
  });
});
