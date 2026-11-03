/** Decks page: Pioneer DDJ-1000-style dual-deck layout.
 *  Each deck = an independent engine stream (backend Deck struct):
 *  load/play/seek, channel fader, 8 hot cues, tempo, jog waveform.
 *  Center: crossfader (equal-power). */

import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import type { Page } from "../router";
import { setToast } from "../state";
import type { TrackDto } from "../types";

interface DeckDto {
  index: number;
  name: string;
  loaded: {
    path: string;
    title: string | null;
    artist: string | null;
    bpm: number | null;
  } | null;
  state: string;
  position_s: number;
  duration_s: number | null;
  channel_fader: number;
  gain_db: number;
  cues: (number | null)[];
  speed: number;
  bpm: number | null;
}

/** Waveform per deck path (cache, 160 bins). */
const waveCache = new Map<string, number[]>();
const WAVE_BINS = 160;

async function getWave(path: string): Promise<number[]> {
  const hit = waveCache.get(path);
  if (hit) return hit;
  try {
    const peaks = await invoke<number[]>("waveform", { path, bins: WAVE_BINS });
    waveCache.set(path, peaks);
    return peaks;
  } catch {
    return Array<number>(WAVE_BINS).fill(0.12);
  }
}

function fmtTime(s: number): string {
  if (!Number.isFinite(s) || s < 0) return "0:00";
  const m = Math.floor(s / 60);
  const sec = Math.floor(s % 60);
  return `${m}:${sec.toString().padStart(2, "0")}`;
}

function stem(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}

let deckPoll: ReturnType<typeof setInterval> | null = null;

function render(root: HTMLElement) {
  root.innerHTML = `
    <section class="page decks-page">
      <header class="page-head">
        <div>
          <p class="overline">Performance</p>
          <h1 class="page-title">Decks</h1>
        </div>
        <div class="page-actions">
          <span class="meta" id="decks-hint">Load a track on each deck, then mix with the crossfader</span>
        </div>
      </header>

      <div class="decks-grid">
        <div class="deck" id="deck-0"></div>
        <div class="deck deck-b" id="deck-1"></div>
      </div>

      <div class="mixer">
        <div class="mixer-col crossfader-col">
          <p class="overline">Crossfader</p>
          <input id="crossfader" type="range" min="0" max="1" step="0.01" value="0" />
          <div class="cf-labels"><span>A</span><span>B</span></div>
        </div>
      </div>
    </section>
  `;

  const cf = root.querySelector<HTMLInputElement>("#crossfader");
  cf?.addEventListener("input", () => {
    const x = Number.parseFloat(cf.value);
    void invoke<[DeckDto, DeckDto]>("set_crossfader", { value: x });
  });

  renderDeck(root, 0);
  renderDeck(root, 1);
  void refresh(root);

  // Live poll: positions + waveform redraw while the page is mounted.
  if (deckPoll) clearInterval(deckPoll);
  deckPoll = setInterval(() => {
    if (!document.getElementById("deck-0")) {
      if (deckPoll) clearInterval(deckPoll);
      deckPoll = null;
      return;
    }
    void refresh(root, true);
  }, 200);
}

function renderDeck(root: HTMLElement, idx: number) {
  const el = root.querySelector<HTMLElement>(`#deck-${idx}`);
  if (!el) return;
  const name = idx === 0 ? "A" : "B";
  el.innerHTML = `
    <header class="deck-head">
      <span class="deck-letter">${name}</span>
      <div class="deck-track">
        <span class="deck-title" id="dk-${idx}-title">No track loaded</span>
        <span class="deck-artist" id="dk-${idx}-artist"></span>
      </div>
      <span class="deck-bpm" id="dk-${idx}-bpm" title="Measured tempo of the loaded track">--</span>
      <div class="deck-transport">
        <button class="btn deck-load-btn" id="dk-${idx}-load" type="button">Load</button>
        <button class="btn deck-play-btn" id="dk-${idx}-play" type="button">▶</button>
        <button class="btn" id="dk-${idx}-stop" type="button">■</button>
      </div>
    </header>

    <div class="jog-wave" id="dk-${idx}-wave">
      <canvas id="dk-${idx}-canvas" width="560" height="72"></canvas>
      <div class="wave-progress" id="dk-${idx}-prog"></div>
      <div class="cue-markers" id="dk-${idx}-cues"></div>
    </div>
    <div class="deck-times">
      <span id="dk-${idx}-cur">0:00</span>
      <span id="dk-${idx}-dur">0:00</span>
    </div>

    <div class="deck-controls">
      <label class="ctl deck-fader-ctl">
        <span class="overline">Level</span>
        <input id="dk-${idx}-fader" type="range" min="0" max="1" step="0.01" value="1"
          style="writing-mode: vertical-lr; direction: rtl; height: 90px; width: 30px;" />
      </label>

      <div class="deck-right">
        <label class="ctl tempo-ctl">
          <span class="overline">Tempo</span>
          <input id="dk-${idx}-tempo" type="range" min="0.5" max="1.5" step="0.005" value="1" />
          <b id="dk-${idx}-tempo-v">1.00x</b>
          <b class="bpm-live" id="dk-${idx}-bpm-live" hidden></b>
        </label>
        <div class="cue-grid" id="dk-${idx}-cuegrid"></div>
      </div>
    </div>
  `;

  // Transport.
  el.querySelector<HTMLButtonElement>(`#dk-${idx}-load`)?.addEventListener("click", async () => {
    const picked = await open({
      multiple: false,
      filters: [
        {
          name: "Audio",
          extensions: ["flac", "wav", "wave", "mp3", "m4a", "alac", "aac", "ogg", "oga", "opus", "aif", "aiff", "wv"],
        },
      ],
    });
    const path = typeof picked === "string" ? picked : null;
    if (!path) return;
    try {
      await invoke("deck_load", {
        deck: idx,
        path,
        title: null,
        artist: null,
      });
      setToast(`Deck ${name}: ${stem(path)} loaded`);
      void refresh(root);
    } catch (e) {
      setToast(`Deck ${name} load failed: ${String(e)}`);
    }
  });

  el.querySelector<HTMLButtonElement>(`#dk-${idx}-play`)?.addEventListener("click", async () => {
    await invoke("deck_play", { deck: idx });
    void refresh(root);
  });
  el.querySelector<HTMLButtonElement>(`#dk-${idx}-stop`)?.addEventListener("click", async () => {
    await invoke("deck_stop", { deck: idx });
    void refresh(root);
  });

  // Jog: click on the waveform = seek.
  el.querySelector<HTMLElement>(`#dk-${idx}-wave`)?.addEventListener("pointerdown", (ev) => {
    const rect = (ev.currentTarget as HTMLElement).getBoundingClientRect();
    const frac = Math.min(1, Math.max(0, (ev.clientX - rect.left) / rect.width));
    void refresh(root, true).then((decks) => {
      const d = decks[idx];
      if (d?.duration_s) {
        void invoke("deck_seek", { deck: idx, positionS: frac * d.duration_s });
      }
    });
  });

  // Fader.
  el.querySelector<HTMLInputElement>(`#dk-${idx}-fader`)?.addEventListener("input", (ev) => {
    const v = Number.parseFloat((ev.target as HTMLInputElement).value);
    void invoke("deck_fader", { deck: idx, value: v });
  });

  // Tempo.
  const tempoEl = el.querySelector<HTMLInputElement>(`#dk-${idx}-tempo`);
  const tempoV = el.querySelector<HTMLElement>(`#dk-${idx}-tempo-v`);
  tempoEl?.addEventListener("input", () => {
    const v = Number.parseFloat(tempoEl.value);
    if (tempoV) tempoV.textContent = `${v.toFixed(2)}x`;
    // Live effective BPM (bpm × speed) without waiting for the poll.
    const bpmLive = el.querySelector<HTMLElement>(`#dk-${idx}-bpm-live`);
    const bpmEl = el.querySelector<HTMLElement>(`#dk-${idx}-bpm`);
    const bpmText = bpmEl?.textContent ?? "";
    const bpm = Number.parseFloat(bpmText);
    if (bpmLive && Number.isFinite(bpm)) {
      bpmLive.hidden = false;
      bpmLive.textContent = `${(bpm * v).toFixed(1)} BPM`;
    }
    void invoke("deck_tempo", { deck: idx, speed: v });
  });

  // Cue grid: 8 slots; click = jump, shift-click = set at current pos.
  const cueGrid = el.querySelector<HTMLElement>(`#dk-${idx}-cuegrid`);
  if (cueGrid) {
    for (let s = 0; s < 8; s++) {
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = "cue-btn";
      btn.textContent = String(s + 1);
      btn.id = `dk-${idx}-cue-${s}`;
      btn.title = `Cue ${s + 1}: click = jump, shift+click = set here`;
      btn.addEventListener("click", async (ev) => {
        if (ev.shiftKey) {
          const decks = await refresh(root, true);
          const pos = decks[idx]?.position_s ?? 0;
          await invoke("deck_set_cue", { deck: idx, slot: s, position: pos });
          setToast(`Deck ${name} cue ${s + 1} set at ${fmtTime(pos)}`);
          void refresh(root);
        } else {
          await invoke("deck_jump_cue", { deck: idx, slot: s });
          void refresh(root);
        }
      });
      cueGrid.appendChild(btn);
    }
  }
}

/** Fetch status and redraw everything. Returns the decks for callers. */
async function refresh(root: HTMLElement, quiet = false): Promise<DeckDto[]> {
  let decks: DeckDto[] = [];
  try {
    decks = await invoke<[DeckDto, DeckDto]>("deck_status");
  } catch {
    return decks;
  }
  for (const d of decks) {
    const idx = d.index;
    const titleEl = root.querySelector<HTMLElement>(`#dk-${idx}-title`);
    const artistEl = root.querySelector<HTMLElement>(`#dk-${idx}-artist`);
    const curEl = root.querySelector<HTMLElement>(`#dk-${idx}-cur`);
    const durEl = root.querySelector<HTMLElement>(`#dk-${idx}-dur`);
    const playBtn = root.querySelector<HTMLButtonElement>(`#dk-${idx}-play`);
    const prog = root.querySelector<HTMLElement>(`#dk-${idx}-prog`);

    if (titleEl && d.loaded) titleEl.textContent = d.loaded.title ?? stem(d.loaded.path);
    if (artistEl && d.loaded) artistEl.textContent = d.loaded.artist ?? "";

    // Measured BPM: the number is the file's own tempo; the live value
    // next to the tempo slider shows bpm × speed (what you actually hear).
    const bpmEl = root.querySelector<HTMLElement>(`#dk-${idx}-bpm`);
    const bpmLive = root.querySelector<HTMLElement>(`#dk-${idx}-bpm-live`);
    const bpm = d.loaded?.bpm ?? null;
    if (bpmEl) bpmEl.textContent = bpm != null ? `${bpm.toFixed(1)}` : "--";
    if (bpmLive) {
      if (bpm != null && Math.abs(d.speed - 1) > 0.004) {
        bpmLive.hidden = false;
        bpmLive.textContent = `${(bpm * d.speed).toFixed(1)} BPM`;
      } else if (bpm != null) {
        bpmLive.hidden = false;
        bpmLive.textContent = `${bpm.toFixed(1)} BPM`;
      } else {
        bpmLive.hidden = true;
      }
    }

    if (playBtn) {
      playBtn.textContent = d.state === "playing" ? "⏸" : "▶";
      playBtn.classList.toggle("playing", d.state === "playing");
    }
    if (curEl) curEl.textContent = fmtTime(d.position_s);
    if (durEl) durEl.textContent = fmtTime(d.duration_s ?? 0);

    // Waveform + cue markers.
    if (d.loaded) {
      void drawDeckWave(root, idx, d);
      void drawCues(root, idx, d);
    }
    if (prog && d.duration_s) {
      prog.style.width = `${Math.min(100, (d.position_s / d.duration_s) * 100)}%`;
    }
  }
  if (!quiet) void 0;
  return decks;
}

async function drawDeckWave(root: HTMLElement, idx: number, d: DeckDto) {
  const canvas = root.querySelector<HTMLCanvasElement>(`#dk-${idx}-canvas`);
  if (!canvas || !d.loaded) return;
  const bars = await getWave(d.loaded.path);
  const ctx = canvas.getContext("2d");
  if (!ctx) return;
  const w = canvas.width;
  const h = canvas.height;
  ctx.clearRect(0, 0, w, h);
  const n = bars.length;
  const barW = w / n;
  const frac = d.duration_s ? Math.min(1, d.position_s / d.duration_s) : 0;
  for (let i = 0; i < n; i++) {
    const v = bars[i];
    const bh = Math.max(2, v * (h - 8));
    const x = i * barW;
    const y = (h - bh) / 2;
    ctx.fillStyle = i / n <= frac ? "#e8500a" : "#4a4a47";
    ctx.fillRect(x + barW * 0.15, y, Math.max(1, barW * 0.7), bh);
  }
}

function drawCues(root: HTMLElement, idx: number, d: DeckDto) {
  const holder = root.querySelector<HTMLElement>(`#dk-${idx}-cues`);
  if (!holder || !d.duration_s) return;
  holder.innerHTML = "";
  d.cues.forEach((pos, slot) => {
    if (pos == null) return;
    const mark = document.createElement("span");
    mark.className = "cue-mark";
    mark.style.left = `${(pos / d.duration_s!) * 100}%`;
    mark.title = `Cue ${slot + 1} @ ${fmtTime(pos)}`;
    holder.appendChild(mark);
    // Button state.
    const btn = root.querySelector<HTMLButtonElement>(`#dk-${idx}-cue-${slot}`);
    if (btn) btn.classList.add("set");
  });
}

function update(root: HTMLElement) {
  void refresh(root);
}

/** Load a library track onto a deck (from the library page context). */
export async function loadTrackToDeck(idx: number, t: TrackDto) {
  try {
    await invoke("deck_load", {
      deck: idx,
      path: t.path,
      title: t.title,
      artist: t.artist,
    });
    setToast(`Deck ${idx === 0 ? "A" : "B"}: ${t.display} loaded`);
  } catch (e) {
    setToast(`Deck load failed: ${String(e)}`);
  }
}

export const decksPage: Page = {
  id: "decks",
  title: "Decks",
  mount: render,
  update,
};
