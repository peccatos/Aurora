import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { artistRouteName, normalizeArtist, tracksForArtist } from "../src/artist-model.ts";

describe("artist route helpers", () => {
  it("normalizes case and whitespace without merging punctuation", () => {
    assert.equal(normalizeArtist("  The   National "), "the national");
    assert.notEqual(normalizeArtist("AC/DC"), normalizeArtist("AC DC"));
  });

  it("matches local tracks case-insensitively and ignores missing artists", () => {
    const tracks = [{ artist: " Björk " }, { artist: "Other" }, { artist: null }];
    assert.deepEqual(tracksForArtist(tracks, "björk"), [tracks[0]]);
    assert.deepEqual(tracksForArtist(tracks, ""), []);
  });

  it("reads and safely rejects artist route query values", () => {
    assert.equal(artistRouteName("#/artist?name=Mass%20Appeal"), "Mass Appeal");
    assert.equal(artistRouteName("#/artist?name="), null);
    assert.equal(artistRouteName("#/artist"), null);
  });
});
