import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { formatCorrectionFilter, parseRewFilters, RewParseError } from "../src/rew-parser.ts";

const REW_FILE = [
  "Room EQ V5.20  Room 1 Correction.txt",
  "Filter  1: ON  PK  Fc  25.2 Hz  Gain  0.5 dB  Q  2.00",
  "Filter  2: ON  LS  Fc  100 Hz  Gain -3.5 dB  Q  0.70",
  "Filter  3: OFF PK  Fc  3000 Hz  Gain  8.0 dB  Q  4.00",
  "Filter  4: ON  HS  Fc  8000 Hz  Gain -2.0 dB  Q  0.50",
  "Preamp: -3.0 dB",
].join("\n");

describe("REW parser", () => {
  it("parses a full REW file, skipping OFF filters and Preamp", () => {
    const filters = parseRewFilters(REW_FILE);
    assert.equal(filters.length, 3);
    assert.deepEqual(filters[0], { kind: "pk", fc_hz: 25.2, gain_db: 0.5, q: 2 });
    assert.deepEqual(filters[1], { kind: "ls", fc_hz: 100, gain_db: -3.5, q: 0.7 });
    assert.deepEqual(filters[2], { kind: "hs", fc_hz: 8000, gain_db: -2, q: 0.5 });
  });

  it("rejects an all-OFF file", () => {
    assert.throws(
      () => parseRewFilters("Filter 1: OFF PK Fc 100 Hz Gain 1.0 dB Q 2.0"),
      /no enabled filters/,
    );
  });

  it("rejects malformed lines", () => {
    assert.throws(
      () => parseRewFilters("Filter 1: ON PK Fc Hz Gain 1.0 dB Q 2.0"),
      RewParseError,
    );
    assert.throws(
      () => parseRewFilters("Filter 1: ON BPZ Fc 100 Hz Gain 1.0 dB Q 2.0"),
      /unsupported filter type/,
    );
  });

  it("rejects out-of-bounds values instead of clamping", () => {
    const bad = [
      "Filter 1: ON PK Fc 5 Hz Gain 1.0 dB Q 2.0",
      "Filter 1: ON PK Fc 25000 Hz Gain 1.0 dB Q 2.0",
      "Filter 1: ON PK Fc 100 Hz Gain 25.0 dB Q 2.0",
      "Filter 1: ON PK Fc 100 Hz Gain -25.0 dB Q 2.0",
      "Filter 1: ON PK Fc 100 Hz Gain 1.0 dB Q 0.05",
      "Filter 1: ON PK Fc 100 Hz Gain 1.0 dB Q 14.0",
    ];
    for (const line of bad) {
      assert.throws(() => parseRewFilters(line), RewParseError, line);
    }
  });

  it("caps at 32 enabled filters, rejecting the 33rd", () => {
    const lines = Array.from({ length: 33 }, (_, i) =>
      `Filter ${i}: ON PK Fc ${100 + i} Hz Gain 1.0 dB Q 2.0`,
    );
    assert.throws(() => parseRewFilters(lines.join("\n")), /more than 32/);
    assert.equal(parseRewFilters(lines.slice(0, 32).join("\n")).length, 32);
  });

  it("ignores non-filter header lines", () => {
    const filters = parseRewFilters(
      "some header\nNotes: measured 2026-11-10\n" + REW_FILE,
    );
    assert.equal(filters.length, 3);
  });

  it("formats a preview line", () => {
    assert.equal(
      formatCorrectionFilter({ kind: "pk", fc_hz: 100, gain_db: -3.5, q: 2 }),
      "PK 100 Hz -3.5 dB Q2.00",
    );
  });
});
