/** Now Playing page: big art, transport meta, honest signal-path report. */

import type { Page } from "../router";
import { app, setToast } from "../state";
import { renderArtwork } from "../artwork";

const SIGNAL_FIELDS: Array<[string, (s: typeof app.status) => string]> = [
  ["state", (s) => s?.state ?? "idle"],
  ["sample rate", (s) => (s?.sample_rate ? `${(s.sample_rate / 1000).toFixed(1)} kHz` : "—")],
  ["channels", (s) => (s?.channels ? String(s.channels) : "—")],
  ["bit depth", (s) => (s?.bits_per_sample ? `${s.bits_per_sample}-bit` : "—")],
  ["transform", (s) => (s?.transformations.length ? s.transformations.join(", ") : "none")],
  ["integrity", (s) => s?.integrity ?? "unverified"],
  ["frames", (s) => (s?.frames_played ?? 0).toLocaleString()],
  ["underruns", (s) => (s?.underruns ?? 0).toLocaleString()],
];

function render(root: HTMLElement) {
  root.innerHTML = `
    <div class="np-page">
      <div class="np-art" id="np-art"><span class="art-glyph">♪</span></div>
      <div class="np-side">
        <p class="overline">Now playing</p>
        <h1 class="np-title" id="np-title">Nothing playing</h1>
        <p class="np-artist" id="np-artist">Open a file or pick from the library</p>
        <dl class="signal-grid" id="signal-grid"></dl>
        <p class="hint" id="np-toast"></p>
      </div>
    </div>
  `;
  update(root);
}

function update(root: HTMLElement) {
  const s = app.status;
  const current = app.queueCurrent != null ? app.queueTracks[app.queueCurrent] : null;

  const titleEl = root.querySelector<HTMLElement>("#np-title");
  const artistEl = root.querySelector<HTMLElement>("#np-artist");
  const toastEl = root.querySelector<HTMLElement>("#np-toast");

  if (current) {
    const stem = current.path.split(/[\\/]/).pop() ?? current.path;
    if (titleEl) titleEl.textContent = current.title ?? stem;
    if (artistEl) artistEl.textContent = current.artist ?? "—";
  } else if (app.selectedPath) {
    const stem = app.selectedPath.split(/[\\/]/).pop() ?? app.selectedPath;
    if (titleEl) titleEl.textContent = stem;
    if (artistEl) artistEl.textContent = "";
  }

  const art = root.querySelector<HTMLElement>("#np-art");
  if (art) void renderArtwork(art, current?.path ?? app.selectedPath);

  const grid = root.querySelector<HTMLElement>("#signal-grid");
  if (grid) {
    grid.innerHTML = "";
    for (const [label, get] of SIGNAL_FIELDS) {
      const cell = document.createElement("div");
      const dt = document.createElement("dt");
      dt.textContent = label;
      const dd = document.createElement("dd");
      dd.textContent = get(s);
      if (label === "integrity" && s?.integrity === "verified") {
        dd.classList.add("ok");
      }
      cell.append(dt, dd);
      grid.appendChild(cell);
    }
  }
  if (toastEl) toastEl.textContent = app.toast;
}

export function setNowPlayingToast(text: string) {
  setToast(text);
  const el = document.querySelector<HTMLElement>("#np-toast");
  if (el) el.textContent = text;
}

export const playerPage: Page = {
  id: "player",
  title: "Player",
  mount: render,
  update,
};
