/** REW (Room EQ Wizard) "Export filter as text" parser — DOM-free mirror
 * of the Rust parser in crates/aurora-dsp/src/room_correction.rs.
 * Both must parse the same files identically (see tests in both trees).
 *
 * Validation is the contract: files violating the caps are rejected, not
 * clamped — a silently clamped correction is a lie about the room. */

export const MAX_FILTERS = 32;
export const FC_MIN_HZ = 10;
export const FC_MAX_HZ = 20000;
export const GAIN_MIN_DB = -20;
export const GAIN_MAX_DB = 20;
export const Q_MIN = 0.1;
export const Q_MAX = 12;

export type CorrectionFilterKind = "pk" | "ls" | "hs";

export interface CorrectionFilter {
  kind: CorrectionFilterKind;
  fc_hz: number;
  gain_db: number;
  q: number;
}

export class RewParseError extends Error {}

const parseNumber = (token: string): number | null => {
  const v = Number(token);
  return Number.isFinite(v) ? v : null;
};

/** Parse REW export text into correction filters (snake_case fields —
 * the exact shape DspConfig.room_correction.filters expects on the wire). */
export function parseRewFilters(text: string): CorrectionFilter[] {
  const filters: CorrectionFilter[] = [];
  const lines = text.split(/\r?\n/);
  for (let lineNo = 0; lineNo < lines.length; lineNo++) {
    const line = lines[lineNo]!.trim();
    if (!line || line.startsWith("#")) continue;
    if (line.startsWith("Preamp:")) continue;
    if (!line.startsWith("Filter")) continue;
    const tokens = line.slice("Filter".length).trim().split(/\s+/);
    if (tokens.length < 9) {
      throw new RewParseError(`line ${lineNo + 1}: not a REW filter export line`);
    }
    const state = tokens[1]!;
    const type = tokens[2]!;
    let kind: CorrectionFilterKind;
    if (type === "PK") kind = "pk";
    else if (type === "LS") kind = "ls";
    else if (type === "HS") kind = "hs";
    else {
      throw new RewParseError(
        `line ${lineNo + 1}: unsupported filter type '${type}' (only PK/LS/HS)`,
      );
    }
    let fc: number | null = null;
    let gain: number | null = null;
    let q: number | null = null;
    for (let i = 3; i + 1 < tokens.length; ) {
      if (tokens[i] === "Fc") { fc = parseNumber(tokens[i + 1]!); i += 2; }
      else if (tokens[i] === "Gain") { gain = parseNumber(tokens[i + 1]!); i += 2; }
      else if (tokens[i] === "Q") { q = parseNumber(tokens[i + 1]!); i += 2; }
      else i += 1;
    }
    // OFF filters are skipped entirely (not validated — REW keeps
    // disabled experiments in the file).
    if (state !== "ON") continue;
    if (fc === null) throw new RewParseError(`line ${lineNo + 1}: missing Fc`);
    if (gain === null) throw new RewParseError(`line ${lineNo + 1}: missing Gain`);
    if (q === null) throw new RewParseError(`line ${lineNo + 1}: missing Q`);
    if (fc < FC_MIN_HZ || fc > FC_MAX_HZ) {
      throw new RewParseError(`line ${lineNo + 1}: Fc ${fc} Hz outside 10..20000 Hz`);
    }
    if (gain < GAIN_MIN_DB || gain > GAIN_MAX_DB) {
      throw new RewParseError(`line ${lineNo + 1}: Gain ${gain} dB outside -20..+20 dB`);
    }
    if (q < Q_MIN || q > Q_MAX) {
      throw new RewParseError(`line ${lineNo + 1}: Q ${q} outside 0.1..12`);
    }
    if (filters.length >= MAX_FILTERS) {
      throw new RewParseError(`more than ${MAX_FILTERS} enabled filters`);
    }
    filters.push({ kind, fc_hz: fc, gain_db: gain, q });
  }
  if (filters.length === 0) {
    throw new RewParseError("no enabled filters found (all OFF or an empty file)");
  }
  return filters;
}

/** One-line preview for the UI: "PK 100 Hz −3.5 dB Q2". */
export function formatCorrectionFilter(f: CorrectionFilter): string {
  return `${f.kind.toUpperCase()} ${f.fc_hz} Hz ${f.gain_db > 0 ? "+" : ""}${f.gain_db.toFixed(1)} dB Q${f.q.toFixed(2)}`;
}
