/** EQ page: 10-band parametric EQ, preamp/volume, reverb, speed. */

import { invoke, platform } from "../invoke";
import type { Page } from "../router";
import { app, setToast } from "../state";
import type { DspConfig, EqBand, ReverbParams } from "../types";
import { kvGet, kvPut } from "../library-db";

const BAND_FREQS = [31, 63, 125, 250, 500, 1000, 2000, 4000, 8000, 16000];

const EQ_PRESETS: Record<string, number[]> = {
  flat: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
  bass: [6, 5, 3.5, 1.5, 0, 0, 0, 0, 0, 0],
  vocal: [-2, -1, 0, 2, 4, 4.5, 3.5, 1.5, 0, -1],
  rock: [4.5, 3.5, 1.5, -1, -2, 1, 3, 4.5, 4, 3.5],
  jazz: [3, 2, 1, 1.5, -1, -1, 0, 1.5, 2.5, 3],
  electronic: [5, 4, 1, 0, -1.5, -1, 1.5, 3, 4.5, 5],
  classical: [3.5, 2.5, 1, 0, 0, 0, 0, 1, 2, 3],
  night: [-3, -1.5, 0, 1.5, 3, 3.5, 3, 2, 1, 0],
};

function defaultDsp(): DspConfig {
  return {
    eq: {
      bands: BAND_FREQS.map((f) => ({
        kind: "peaking" as const,
        freq_hz: f,
        q: 1.0,
        gain_db: 0,
        enabled: true,
      })),
      preamp_db: 0,
    },
    volume_db: 0,
    replaygain: "off",
    reverb: { room_size: 0.5, damping: 0.3, wet: 0, width: 1, input_lowpass: 0 },
    speed: 1,
  };
}

async function saveDsp() {
  if (!app.dsp) return;
  try {
    await invoke("set_dsp", { config: app.dsp });
    // Persist (web shell): the EQ must survive page reloads. Best-effort —
    // a storage failure never breaks playback.
    if (platform === "web") {
      void kvPut("dsp", app.dsp).catch(() => undefined);
    }
  } catch (e) {
    setToast(`Save failed: ${String(e)}`);
  }
}

function noteText(): string {
  const d = app.dsp;
  if (!d) return "";
  const active =
    d.eq.bands.some((b) => b.gain_db !== 0) ||
    d.eq.preamp_db !== 0 ||
    d.volume_db !== 0 ||
    d.reverb.wet > 0 ||
    Math.abs(d.speed - 1) > 0.01;
  return active
    ? "DSP active — transformations reported in the signal path."
    : "Flat EQ = bit-exact passthrough.";
}

function ctl(
  root: HTMLElement,
  id: string,
  label: string,
  min: number,
  max: number,
  step: number,
  value: number,
  fmt: (v: number) => string,
  onInput: (v: number) => void,
) {
  const el = root.querySelector<HTMLInputElement>(id);
  const out = root.querySelector<HTMLElement>(`${id}-v`);
  if (!el) return;
  el.value = String(value);
  if (out) out.textContent = fmt(value);
  el.addEventListener("input", () => {
    const v = Number.parseFloat(el.value);
    if (out) out.textContent = fmt(v);
    onInput(v);
    saveDsp();
    const note = root.querySelector<HTMLElement>("#eq-note");
    if (note) note.textContent = noteText();
  });
}

function render(root: HTMLElement) {
  if (!app.dsp) app.dsp = defaultDsp();

  root.innerHTML = `
    <section class="page">
      <header class="page-head">
        <div>
          <p class="overline">Parametric · 10 band</p>
          <h1 class="page-title">Equalizer</h1>
        </div>
        <div class="page-actions">
          <select id="eq-preset" aria-label="Preset">
            ${Object.keys(EQ_PRESETS)
              .map(
                (k) =>
                  `<option value="${k}">${k[0].toUpperCase() + k.slice(1)}</option>`,
              )
              .join("")}
          </select>
          <button class="btn" id="eq-flat" type="button">Flat</button>
        </div>
      </header>

      <div class="eq-card">
        <div id="eq-bands" class="eq-bands"></div>
        <div class="eq-extras">
          <label class="ctl">Preamp <input id="eq-preamp" type="range" min="-12" max="12" step="0.5" /> <span id="eq-preamp-v"></span></label>
          <label class="ctl">Volume <input id="eq-volume" type="range" min="-40" max="12" step="0.5" /> <span id="eq-volume-v"></span></label>
        </div>
        <p class="hint" id="eq-note"></p>
      </div>

      <div class="eq-card">
        <header class="card-head">
          <p class="overline">Reverb · Freeverb</p>
          <button class="btn" id="reverb-off" type="button">Off</button>
        </header>
        <div class="ctl-grid">
          <label class="ctl">Wet <input id="rv-wet" type="range" min="0" max="1" step="0.01" /><span id="rv-wet-v"></span></label>
          <label class="ctl">Room <input id="rv-room" type="range" min="0" max="1" step="0.01" /><span id="rv-room-v"></span></label>
          <label class="ctl">Damp <input id="rv-damp" type="range" min="0" max="1" step="0.01" /><span id="rv-damp-v"></span></label>
          <label class="ctl">Width <input id="rv-width" type="range" min="0" max="1" step="0.01" /><span id="rv-width-v"></span></label>
        </div>
      </div>

      <div class="eq-card">
        <header class="card-head">
          <p class="overline">Speed</p>
          <div class="page-actions">
            <button class="btn" id="fx-slowed" type="button">Slowed</button>
            <button class="btn" id="fx-speedup" type="button">Speed up</button>
          </div>
        </header>
        <div class="ctl-grid">
          <label class="ctl">Speed <input id="fx-speed" type="range" min="0.5" max="2" step="0.05" /><span id="fx-speed-v"></span></label>
        </div>
      </div>
    </section>
  `;

  update(root);

  // Sliders.
  const d = app.dsp!;
  const bind = (
    id: string,
    label: string,
    min: number,
    max: number,
    step: number,
    value: number,
    fmt: (v: number) => string,
    onInput: (v: number) => void,
  ) => ctl(root, `#${id}`, label, min, max, step, value, fmt, onInput);

  bind("eq-preamp", "Preamp", -12, 12, 0.5, d.eq.preamp_db, (v) => `${v.toFixed(1)} dB`, (v) => (d.eq.preamp_db = v));
  bind("eq-volume", "Volume", -40, 12, 0.5, d.volume_db, (v) => `${v.toFixed(1)} dB`, (v) => (d.volume_db = v));
  bind("rv-wet", "Wet", 0, 1, 0.01, d.reverb.wet, (v) => `${Math.round(v * 100)}%`, (v) => (d.reverb.wet = v));
  bind("rv-room", "Room", 0, 1, 0.01, d.reverb.room_size, (v) => v.toFixed(2), (v) => (d.reverb.room_size = v));
  bind("rv-damp", "Damp", 0, 1, 0.01, d.reverb.damping, (v) => v.toFixed(2), (v) => (d.reverb.damping = v));
  bind("rv-width", "Width", 0, 1, 0.01, d.reverb.width, (v) => v.toFixed(2), (v) => (d.reverb.width = v));
  bind("fx-speed", "Speed", 0.5, 2, 0.05, d.speed, (v) => `${v.toFixed(2)}x`, (v) => (d.speed = v));

  const setSpeed = (v: number) => {
    d.speed = v;
    const el = root.querySelector<HTMLInputElement>("#fx-speed");
    const out = root.querySelector<HTMLElement>("#fx-speed-v");
    if (el) el.value = String(v);
    if (out) out.textContent = `${v.toFixed(2)}x`;
    void saveDsp();
    const note = root.querySelector<HTMLElement>("#eq-note");
    if (note) note.textContent = noteText();
  };
  root.querySelector<HTMLButtonElement>("#fx-slowed")?.addEventListener("click", () => setSpeed(0.8));
  root.querySelector<HTMLButtonElement>("#fx-speedup")?.addEventListener("click", () => setSpeed(1.25));

  root.querySelector<HTMLButtonElement>("#reverb-off")?.addEventListener("click", () => {
    d.reverb.wet = 0;
    const el = root.querySelector<HTMLInputElement>("#rv-wet");
    const out = root.querySelector<HTMLElement>("#rv-wet-v");
    if (el) el.value = "0";
    if (out) out.textContent = "0%";
    void saveDsp();
    const note = root.querySelector<HTMLElement>("#eq-note");
    if (note) note.textContent = noteText();
  });

  root.querySelector<HTMLButtonElement>("#eq-flat")?.addEventListener("click", () => {
    d.eq.bands.forEach((b) => (b.gain_db = 0));
    d.eq.preamp_db = 0;
    update(root);
    void saveDsp();
  });

  root.querySelector<HTMLSelectElement>("#eq-preset")?.addEventListener("change", (e) => {
    const name = (e.target as HTMLSelectElement).value;
    const gains = EQ_PRESETS[name];
    if (!gains) return;
    d.eq.bands.forEach((b, i) => (b.gain_db = gains[i] ?? 0));
    d.eq.preamp_db = name === "night" ? -2 : 0;
    update(root);
    void saveDsp();
  });

  const note = root.querySelector<HTMLElement>("#eq-note");
  if (note) note.textContent = noteText();
}

function update(root: HTMLElement) {
  const d = app.dsp;
  if (!d) return;
  const wrap = root.querySelector<HTMLElement>("#eq-bands");
  if (!wrap) return;
  wrap.innerHTML = "";
  d.eq.bands.forEach((band, idx) => {
    const cell = document.createElement("div");
    cell.className = "eq-band";
    const label = document.createElement("span");
    label.textContent = band.freq_hz >= 1000 ? `${band.freq_hz / 1000}k` : `${band.freq_hz}`;
    const slider = document.createElement("input");
    slider.type = "range";
    slider.min = "-12";
    slider.max = "12";
    slider.step = "0.5";
    slider.value = String(band.gain_db);
    const value = document.createElement("b");
    value.textContent = `${band.gain_db > 0 ? "+" : ""}${band.gain_db.toFixed(1)}`;
    slider.addEventListener("input", () => {
      const v = Number.parseFloat(slider.value);
      d.eq.bands[idx].gain_db = v;
      value.textContent = `${v > 0 ? "+" : ""}${v.toFixed(1)}`;
      void saveDsp();
      const note = root.querySelector<HTMLElement>("#eq-note");
      if (note) note.textContent = noteText();
    });
    cell.append(label, slider, value);
    wrap.appendChild(cell);
  });
}

/** Load the backend's DSP config once at app start. Web: a persisted
 *  config (IndexedDB) is pushed INTO the kernel, so reloads restore the
 *  user's EQ. */
export async function loadDsp() {
  try {
    app.dsp = await invoke<DspConfig>("get_dsp");
  } catch {
    app.dsp = defaultDsp();
  }
  if (platform === "web") {
    try {
      const saved = await kvGet<DspConfig>("dsp");
      if (saved) {
        app.dsp = saved;
        try {
          await invoke("set_dsp", { config: saved });
        } catch {
          /* kernel busy — defaults remain */
        }
      }
    } catch {
      /* storage unavailable — defaults remain */
    }
  }
}

export const eqPage: Page = {
  id: "eq",
  title: "Equalizer",
  mount: render,
  update,
};
