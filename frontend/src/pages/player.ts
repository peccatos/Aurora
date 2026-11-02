/** Now Playing page: big art, waveform seek, spectrum, A/B loop,
 *  honest signal-path report. */

import { invoke } from "@tauri-apps/api/core";
import type { Page } from "../router";
import { app, setToast } from "../state";
import { renderArtwork } from "../artwork";
import type { PlaybackStatus } from "../types";

const SIGNAL_FIELDS: Array<[string, (s: PlaybackStatus | null) => string]> = [
  ["state", (s) => s?.state ?? "idle"],
  ["sample rate", (s) => (s?.sample_rate ? `${(s.sample_rate / 1000).toFixed(1)} kHz` : "—")],
  ["channels", (s) => (s?.channels ? String(s.channels) : "—")],
  ["bit depth", (s) => (s?.bits_per_sample ? `${s.bits_per_sample}-bit` : "—")],
  ["transform", (s) => (s?.transformations.length ? s.transformations.join(", ") : "none")],
  ["integrity", (s) => s?.integrity ?? "unverified"],
  ["frames", (s) => (s?.frames_played ?? 0).toLocaleString()],
  ["underruns", (s) => (s?.underruns ?? 0).toLocaleString()],
];

const WAVE_BINS = 160;

/** Waveform cache per track path. */
const waveCache = new Map<string, number[]>();
let waveBars: number[] | null = null;
let loopRegion: { start: number; end: number } | null = null;

async function loadWave(path: string) {
  if (waveCache.has(path)) {
    waveBars = waveCache.get(path) ?? null;
    return;
  }
  try {
    const peaks = await invoke<number[]>("waveform", { path, bins: WAVE_BINS });
    waveCache.set(path, peaks);
    waveBars = peaks;
  } catch {
    waveBars = null;
  }
}

function fmtTime(s: number): string {
  if (!Number.isFinite(s) || s < 0) return "0:00";
  const m = Math.floor(s / 60);
  const sec = Math.floor(s % 60);
  return `${m}:${sec.toString().padStart(2, "0")}`;
}

function render(root: HTMLElement) {
  root.innerHTML = `
    <div class="np-page">
      <div class="np-art" id="np-art"><span class="art-glyph">♪</span></div>
      <div class="np-side">
        <p class="overline">Now playing</p>
        <h1 class="np-title" id="np-title">Nothing playing</h1>
        <p class="np-artist" id="np-artist">Open a file or pick from the library</p>

        <div class="wave-seek" id="wave-seek">
          <canvas id="wave-canvas" width="640" height="56"></canvas>
          <div class="wave-progress" id="wave-progress"></div>
          <div class="wave-loop" id="wave-loop" hidden></div>
        </div>
        <div class="wave-times">
          <span id="wave-cur">0:00</span>
          <div class="loop-controls">
            <button class="btn" id="loop-set-a" type="button">A</button>
            <button class="btn" id="loop-set-b" type="button">B</button>
            <button class="btn" id="loop-clear" type="button">Loop off</button>
          </div>
          <span id="wave-dur">0:00</span>
        </div>

        <div class="spectrum" id="spectrum"></div>

        <dl class="signal-grid" id="signal-grid"></dl>
        <p class="hint" id="np-toast"></p>
      </div>
    </div>
  `;

  // Seek by clicking/dragging on the waveform.
  const seekEl = root.querySelector<HTMLElement>("#wave-seek");
  seekEl?.addEventListener("pointerdown", (ev) => {
    const rect = seekEl.getBoundingClientRect();
    const frac = Math.min(1, Math.max(0, (ev.clientX - rect.left) / rect.width));
    const pos = frac * app.durationS;
    if (app.durationS > 0) {
      void invoke("seek", { positionS: pos });
    }
  });

  // A/B loop controls.
  root.querySelector<HTMLButtonElement>("#loop-set-a")?.addEventListener("click", () => {
    const pos = currentPosSec(root);
    loopRegion = { start: pos, end: loopRegion?.end ?? Math.min(app.durationS, pos + 30) };
    void syncLoop();
    setToast(`Loop A at ${fmtTime(pos)}`);
  });
  root.querySelector<HTMLButtonElement>("#loop-set-b")?.addEventListener("click", () => {
    if (!loopRegion) {
      setToast("Set A first");
      return;
    }
    loopRegion.end = currentPosSec(root);
    void syncLoop();
    setToast(`Loop ${fmtTime(loopRegion.start)} → ${fmtTime(loopRegion.end)}`);
  });
  root.querySelector<HTMLButtonElement>("#loop-clear")?.addEventListener("click", () => {
    loopRegion = null;
    void invoke("set_loop", { loopStart: null, loopEnd: null });
    update(root);
    setToast("Loop off");
  });

  // Spectrum bars skeleton.
  const spec = root.querySelector<HTMLElement>("#spectrum");
  if (spec) {
    for (let i = 0; i < 16; i++) {
      const bar = document.createElement("span");
      bar.className = "spec-bar";
      spec.appendChild(bar);
    }
  }

  update(root);
}

async function syncLoop() {
  if (!loopRegion) return;
  await invoke("set_loop", {
    loopStart: loopRegion.start,
    loopEnd: loopRegion.end,
  });
}

function currentPosSec(root: HTMLElement): number {
  const cur = root.querySelector<HTMLElement>("#wave-cur")?.textContent ?? "0:00";
  const [m, s] = cur.split(":").map(Number);
  return (m || 0) * 60 + (s || 0);
}

function drawWave(root: HTMLElement) {
  const canvas = root.querySelector<HTMLCanvasElement>("#wave-canvas");
  const prog = root.querySelector<HTMLElement>("#wave-progress");
  const loopEl = root.querySelector<HTMLElement>("#wave-loop");
  const curEl = root.querySelector<HTMLElement>("#wave-cur");
  const durEl = root.querySelector<HTMLElement>("#wave-dur");
  if (!canvas) return;
  const ctx = canvas.getContext("2d");
  if (!ctx) return;

  const w = canvas.width;
  const h = canvas.height;
  ctx.clearRect(0, 0, w, h);

  const bars = waveBars ?? Array<number>(WAVE_BINS).fill(0.15);
  const n = bars.length;
  const barW = w / n;
  const posFrac =
    app.durationS > 0 ? Math.min(1, posFromBar() / app.durationS) : 0;

  for (let i = 0; i < n; i++) {
    const v = bars[i];
    const bh = Math.max(2, v * (h - 6));
    const x = i * barW;
    const y = (h - bh) / 2;
    // Played part: accent; upcoming: quiet grey.
    ctx.fillStyle = i / n <= posFrac ? "#e8500a" : "#4a4a47";
    ctx.fillRect(x + barW * 0.15, y, Math.max(1, barW * 0.7), bh);
  }

  if (prog) prog.style.width = `${posFrac * 100}%`;

  // Loop region overlay.
  if (loopEl) {
    if (loopRegion && app.durationS > 0) {
      loopEl.hidden = false;
      loopEl.style.left = `${(loopRegion.start / app.durationS) * 100}%`;
      loopEl.style.width = `${((loopRegion.end - loopRegion.start) / app.durationS) * 100}%`;
    } else {
      loopEl.hidden = true;
    }
  }

  if (durEl && app.durationS > 0) durEl.textContent = fmtTime(app.durationS);
  void curEl;
}

function posFromBar(): number {
  const bar = document.getElementById("seek") as HTMLInputElement | null;
  if (bar && app.durationS > 0) {
    return (Number.parseFloat(bar.value) / 100) * app.durationS;
  }
  return 0;
}

function drawSpectrum(root: HTMLElement) {
  const spec = root.querySelector<HTMLElement>("#spectrum");
  if (!spec) return;
  const bars = spec.querySelectorAll<HTMLElement>(".spec-bar");
  if (!bars.length) return;
  void invoke<number[]>("playback_spectrum")
    .then((bands) => {
      bars.forEach((el, i) => {
        const v = bands[i] ?? 0;
        el.style.setProperty("--v", v.toFixed(3));
      });
    })
    .catch(() => {});
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

  // Load the waveform for this track (once, cached).
  const wavePath = current?.path ?? app.selectedPath;
  if (wavePath && waveCache.size === 0 || (wavePath && !waveCache.has(wavePath))) {
    if (wavePath) void loadWave(wavePath);
  }

  drawWave(root);
  drawSpectrum(root);

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

/** Continuous animation: waveform progress + spectrum while visible. */
export function animatePlayer() {
  const root = document.getElementById("page-root");
  if (!root) return;
  const curEl = root.querySelector<HTMLElement>("#wave-cur");
  const seekBar = document.getElementById("seek") as HTMLInputElement | null;
  if (curEl && seekBar && app.durationS > 0) {
    curEl.textContent = fmtTime((Number.parseFloat(seekBar.value) / 100) * app.durationS);
  }
  drawWave(root);
  drawSpectrum(root);
}

export const playerPage: Page = {
  id: "player",
  title: "Player",
  mount: render,
  update,
};
