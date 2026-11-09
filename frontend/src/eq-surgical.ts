/** Surgical EQ mode constants and pure helpers (no DOM imports so the
 *  node test runner can load this module directly). The Rust side
 *  accepts any band count; the cap is a UI decision. */
export const SURGICAL_MAX_BANDS = 31;
export const SURGICAL_Q_DEFAULT = 4.0;

/** Adding a band is refused at the cap; removal keeps at least one. */
export function nextBandCount(current: number, add: boolean): number | null {
  if (add) return current < SURGICAL_MAX_BANDS ? current + 1 : null;
  return current > 1 ? current - 1 : null;
}

/** Clamp a surgical Q to the 0.1-12 UI range. */
export function clampSurgicalQ(q: number): number {
  return Number.isFinite(q) ? Math.max(0.1, Math.min(12, q)) : SURGICAL_Q_DEFAULT;
}
