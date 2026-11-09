import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { SURGICAL_MAX_BANDS, SURGICAL_Q_DEFAULT, clampSurgicalQ, nextBandCount } from "../src/eq-surgical.ts";

describe("EQ surgical mode", () => {
  it("caps band count at 31 and never drops below one", () => {
    assert.equal(nextBandCount(10, true), 11);
    assert.equal(nextBandCount(SURGICAL_MAX_BANDS, true), null, "must refuse to exceed 31 bands");
    assert.equal(nextBandCount(1, false), null, "must keep at least one band");
    assert.equal(nextBandCount(5, false), 4);
  });

  it("clamps Q into the surgical 0.1-12 range", () => {
    assert.equal(clampSurgicalQ(0), 0.1);
    assert.equal(clampSurgicalQ(40), 12);
    assert.ok(Math.abs(clampSurgicalQ(Number.NaN) - SURGICAL_Q_DEFAULT) < 1e-9);
    assert.ok(Math.abs(clampSurgicalQ(4) - 4) < 1e-9);
  });
});
