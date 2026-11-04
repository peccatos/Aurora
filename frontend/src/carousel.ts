/** Horizontal card-row carousels (spec 4.3/4.4): scroll-snap rows with
 *  hover chevron arrows that page by ~80% of the visible width.
 *
 *  Idempotent by design: Home re-populates rows on every state change
 *  (renderHistory runs on each update), so makeCarousel can be called
 *  again on the same row — previous arrows and listeners are torn down
 *  first. Rows without a .ytm-shelf parent are left alone (no arrows
 *  without a positioning context). */

import { icon } from "./styles/ytm-icons";

/** Per-row teardown handles (AbortControllers) so a re-carousel on the
 *  same element drops its old scroll/resize listeners. */
const controllers = new WeakMap<HTMLElement, AbortController>();

export function makeCarousel(row: HTMLElement): void {
  const shelf = row.closest(".ytm-shelf");
  if (!shelf) return;

  row.style.scrollSnapType = "x mandatory";
  for (const child of Array.from(row.children)) {
    child.classList.add("ytm-snap");
  }

  // Idempotency: kill the previous incarnation (arrows + listeners).
  controllers.get(row)?.abort();
  shelf.querySelectorAll(".ytm-shelf-arrow").forEach((b) => b.remove());

  const ac = new AbortController();
  controllers.set(row, ac);

  const left = document.createElement("button");
  left.type = "button";
  left.className = "ytm-shelf-arrow left";
  left.title = "Scroll left";
  left.setAttribute("aria-label", "Scroll left");
  left.innerHTML = icon("chevronLeft", 20);

  const right = document.createElement("button");
  right.type = "button";
  right.className = "ytm-shelf-arrow right";
  right.title = "Scroll right";
  right.setAttribute("aria-label", "Scroll right");
  right.innerHTML = icon("chevronRight", 20);

  // Arrows sit at the scroll ends only (honest affordance: nothing to
  // page towards means no button). Re-checked on scroll AND resize —
  // a viewport change moves the ends.
  const update = (): void => {
    const max = row.scrollWidth - row.clientWidth;
    left.style.display = row.scrollLeft <= 2 ? "none" : "";
    right.style.display = row.scrollLeft >= max - 2 ? "none" : "";
  };

  left.addEventListener("click", () => {
    row.scrollBy({ left: -row.clientWidth * 0.8, behavior: "smooth" });
  });
  right.addEventListener("click", () => {
    row.scrollBy({ left: row.clientWidth * 0.8, behavior: "smooth" });
  });

  row.addEventListener("scroll", () => update(), { passive: true, signal: ac.signal });
  window.addEventListener("resize", () => update(), { signal: ac.signal });

  shelf.append(left, right);
  update();
}
