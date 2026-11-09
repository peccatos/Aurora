import type { Page } from "../router";
import { navigate } from "../router";
import { app, setToast } from "../state";
import { invoke, platform, setConvolutionIr } from "../invoke";
import { kvPut } from "../library-db";
import { resetDspProfile, saveDspProfile } from "../dsp-profiles";
import { formatCorrectionFilter, parseRewFilters, RewParseError } from "../rew-parser";
import type { CorrectionFilter } from "../rew-parser";
import type { DspConfig } from "../types";

type ToolKey = "balance" | "stereo_width" | "soft_clip";

const defaultCrossfeed = () => ({ enabled: false, cutoff_hz: 700, feed_db: -6.5 });

function ensureCrossfeed(d: DspConfig) {
  d.crossfeed ??= defaultCrossfeed();
  d.crossfeed.enabled = Boolean(d.crossfeed.enabled);
  d.crossfeed.cutoff_hz = Number.isFinite(d.crossfeed.cutoff_hz)
    ? Math.max(20, Math.min(5000, d.crossfeed.cutoff_hz))
    : 700;
  d.crossfeed.feed_db = Number.isFinite(d.crossfeed.feed_db)
    ? Math.max(-24, Math.min(0, d.crossfeed.feed_db))
    : -6.5;
  return d;
}

const defaultConvolution = () => ({
  enabled: false,
  ir_id: 0,
  wet: 0.3,
  dry: 0.8,
  predelay_ms: 0,
  ir_gain_db: 0,
});

function ensureConvolution(d: DspConfig) {
  d.convolution ??= defaultConvolution();
  d.convolution.enabled = Boolean(d.convolution.enabled);
  d.convolution.ir_id = Number.isFinite(d.convolution.ir_id)
    ? Math.max(0, Math.trunc(d.convolution.ir_id))
    : 0;
  const clamp01 = (v: number, fb: number) =>
    Number.isFinite(v) ? Math.max(0, Math.min(1, v)) : fb;
  d.convolution.wet = clamp01(d.convolution.wet, 0.3);
  d.convolution.dry = clamp01(d.convolution.dry, 0.8);
  d.convolution.predelay_ms = Number.isFinite(d.convolution.predelay_ms)
    ? Math.max(0, Math.min(250, d.convolution.predelay_ms))
    : 0;
  d.convolution.ir_gain_db = Number.isFinite(d.convolution.ir_gain_db)
    ? Math.max(-24, Math.min(12, d.convolution.ir_gain_db))
    : 0;
  return d;
}

function convolutionActive(d: DspConfig): boolean {
  ensureConvolution(d);
  return d.convolution?.enabled === true && d.convolution.ir_id > 0;
}

const defaultRoomCorrection = () => ({ enabled: false, filters: [] as CorrectionFilter[] });

function ensureRoomCorrection(d: DspConfig) {
  d.room_correction ??= defaultRoomCorrection();
  d.room_correction.enabled = Boolean(d.room_correction.enabled);
  d.room_correction.filters = Array.isArray(d.room_correction.filters)
    ? d.room_correction.filters
    : [];
  return d;
}

function roomCorrectionActive(d: DspConfig): boolean {
  ensureRoomCorrection(d);
  return d.room_correction?.enabled === true && d.room_correction.filters.length > 0;
}

const defaultTools = () => ({
  balance: 0,
  phase_invert_left: false,
  stereo_width: 1,
  soft_clip: 0,
});

function ensureTools(d: DspConfig) {
  d.tools ??= defaultTools();
  d.tools.balance = Number.isFinite(d.tools.balance) ? Math.max(-1, Math.min(1, d.tools.balance)) : 0;
  d.tools.stereo_width = Number.isFinite(d.tools.stereo_width) ? Math.max(0, Math.min(2, d.tools.stereo_width)) : 1;
  d.tools.soft_clip = Number.isFinite(d.tools.soft_clip) ? Math.max(0, Math.min(1, d.tools.soft_clip)) : 0;
  d.tools.phase_invert_left = Boolean(d.tools.phase_invert_left);
}

function toolsActive(d: DspConfig): boolean {
  ensureTools(d);
  return Math.abs(d.tools.balance) > 1e-6 || d.tools.phase_invert_left ||
    Math.abs(d.tools.stereo_width - 1) > 1e-6 || d.tools.soft_clip > 1e-6;
}

function crossfeedActive(d: DspConfig): boolean {
  ensureCrossfeed(d);
  return d.crossfeed?.enabled === true;
}

export const effectsPage: Page = {
  id: "effects",
  title: "Effects & Tools",
  mount(root) {
    if (!app.dsp) return;
    const d = app.dsp;
    ensureTools(d);
    ensureCrossfeed(d);
    const cv = ensureConvolution(d).convolution!;
    const rc = ensureRoomCorrection(d).room_correction!;
    const cf = ensureCrossfeed(d).crossfeed!;
    const currentTrack = app.selectedPath;
    root.innerHTML = `
      <section class="page">
        <header class="page-head"><div><p class="overline">Orchard-inspired portable tools</p><h1 class="page-title">Effects & Tools</h1></div></header>
        <p class="hint">These four built-ins run in Aurora's shared Rust DSP path on browser and desktop. Native VST3 plugins remain desktop-only and separate.</p>
        <div class="eq-card">
          <header class="card-head"><div><p class="overline">Global DSP fallback</p><h2>Portable tools</h2></div><strong id="tools-state"></strong></header>
          <div class="ctl-grid">
            <label class="ctl">Balance <input id="tool-balance" type="range" min="-1" max="1" step="0.01"><span id="tool-balance-v"></span></label>
            <label class="ctl">Stereo width <input id="tool-width" type="range" min="0" max="2" step="0.01"><span id="tool-width-v"></span></label>
            <label class="ctl">Soft clip <input id="tool-clip" type="range" min="0" max="1" step="0.01"><span id="tool-clip-v"></span></label>
          </div>
          <label class="ctl"><input id="tool-phase" type="checkbox"> Invert left phase</label>
          <div class="page-actions"><button class="btn" id="tool-reset" type="button">Reset global tools</button></div>
        </div>
        <div class="eq-card">
          <header class="card-head"><div><p class="overline">Headphone stage · bs2b</p><h2>Crossfeed (наушники)</h2></div><strong id="crossfeed-state"></strong></header>
          <label class="ctl"><input id="crossfeed-enabled" type="checkbox"> Enable crossfeed</label>
          <div class="ctl-grid">
            <label class="ctl">Cutoff <input id="crossfeed-cutoff" type="range" min="200" max="1000" step="10"><span id="crossfeed-cutoff-v"></span></label>
            <label class="ctl">Feed <input id="crossfeed-feed" type="range" min="-12" max="0" step="0.5"><span id="crossfeed-feed-v"></span></label>
          </div>
          <div class="page-actions">
            <button class="btn" id="crossfeed-bs2b" type="button">bs2b default (700 Hz / -6.5 dB)</button>
          </div>
          <p class="hint">Bleeds an attenuated, lowpassed copy of each channel into the opposite ear — speaker-like imaging on headphones. Stereo only; reported as a signal transformation when active.</p>
        </div>
        <div class="eq-card">
          <header class="card-head"><div><p class="overline">Convolution reverb · partitioned FIR</p><h2>Convolution Reverb</h2></div><strong id="conv-state"></strong></header>
          <label class="ctl"><input id="conv-enabled" type="checkbox"> Enable convolution reverb</label>
          <div class="ctl-grid">
            <label class="ctl">IR preset
              <select id="conv-ir">
                <option value="0">None (bypass)</option>
                <option value="1">Small room (~0.25 s)</option>
                <option value="2">Hall (~1.5 s)</option>
                <option value="3">Plate (~1.2 s)</option>
                <option value="100">User IR</option>
              </select>
            </label>
            <label class="ctl">IR gain <input id="conv-irgain" type="range" min="-24" max="12" step="0.5"><span id="conv-irgain-v"></span></label>
          </div>
          <div class="ctl-grid">
            <label class="ctl">Wet <input id="conv-wet" type="range" min="0" max="1" step="0.01"><span id="conv-wet-v"></span></label>
            <label class="ctl">Dry <input id="conv-dry" type="range" min="0" max="1" step="0.01"><span id="conv-dry-v"></span></label>
            <label class="ctl">Predelay <input id="conv-predelay" type="range" min="0" max="250" step="1"><span id="conv-predelay-v"></span></label>
          </div>
          <label class="ctl">Load user IR <input id="conv-ir-file" type="file" accept="audio/*,.wav,.flac,.mp3,.ogg,.opus"></label>
          <p class="hint" id="conv-ir-note">Built-in presets are synthetic (generated in Rust, no licensed assets). A user IR is decoded to f32, capped at 10 s / 5 MB, and mono is upmixed. Web only for now.</p>
        </div>
        <div class="eq-card">
          <header class="card-head"><div><p class="overline">REW parametric import</p><h2>Room correction (REW)</h2></div><strong id="rc-state"></strong></header>
          <label class="ctl"><input id="rc-enabled" type="checkbox"> Enable room correction</label>
          <label class="ctl">Load REW filter export <input id="rc-file" type="file" accept=".txt,text/plain"></label>
          <div id="rc-filters" class="ctl" style="display:grid;gap:2px"></div>
          <p class="hint" id="rc-note">Export "Filter as text" from REW (Room EQ Wizard). Filters apply as a biquad cascade after convolution; PK/LS/HS only, max 32 filters, bounds-checked (out-of-range files are rejected, not clamped).</p>
        </div>
        <div class="eq-card">
          <p class="overline">Per-track profile</p>
          <p class="hint" id="current-track-label"></p>
          <div class="page-actions">
            <button class="btn" id="tool-save-track" type="button" ${currentTrack ? "" : "disabled"}>Save current tools to track</button>
            <button class="btn" id="tool-reset-track" type="button" ${currentTrack ? "" : "disabled"}>Remove track profile</button>
          </div>
        </div>
        <div class="page-actions"><button class="btn" id="effects-eq" type="button">Equalizer & DSP</button><button class="btn" id="effects-plugins" type="button">Desktop VST3 plugins</button></div>
        <p class="hint">Platform: ${platform === "web" ? "WebAssembly portable DSP" : "Desktop portable DSP"}. Orchard Studio reference is retained under MIT in THIRD_PARTY_NOTICES.md.</p>
      </section>`;

    const trackLabel = root.querySelector<HTMLElement>("#current-track-label");
    if (trackLabel) trackLabel.textContent = currentTrack
      ? `Current track: ${currentTrack}`
      : "No track selected; global fallback is active.";
    const drawState = () => {
      const state = root.querySelector<HTMLElement>("#tools-state");
      if (state) state.textContent = toolsActive(d) ? "ACTIVE · transformed" : "BYPASSED · bit-exact";
      const cfState = root.querySelector<HTMLElement>("#crossfeed-state");
      if (cfState) cfState.textContent = crossfeedActive(d) ? "ACTIVE · transformed" : "BYPASSED · bit-exact";
      const cvState = root.querySelector<HTMLElement>("#conv-state");
      if (cvState) cvState.textContent = convolutionActive(d) ? "ACTIVE · transformed" : "BYPASSED · bit-exact";
      const rcState = root.querySelector<HTMLElement>("#rc-state");
      if (rcState) rcState.textContent = roomCorrectionActive(d) ? "ACTIVE · transformed" : "BYPASSED · bit-exact";
    };
    // Serialize IPC writes so a slower slider update cannot finish after a
    // newer one and leave the kernel with stale settings.
    let saveChain: Promise<void> = Promise.resolve();
    const save = () => {
      const snapshot = structuredClone(d);
      saveChain = saveChain.then(async () => {
        await invoke("set_dsp", { config: snapshot });
        if (platform === "web") await kvPut("dsp", snapshot);
      }).catch((error: unknown) => setToast(`Save failed: ${String(error)}`));
      drawState();
    };
    const bind = (id: string, outId: string, key: ToolKey, fmt: (v: number) => string) => {
      const el = root.querySelector<HTMLInputElement>(id);
      const out = root.querySelector<HTMLElement>(outId);
      if (!el || !out) return;
      const draw = () => { const v = d.tools[key]; el.value = String(v); out.textContent = fmt(v); };
      draw();
      el.addEventListener("input", () => { d.tools[key] = Number(el.value); draw(); save(); });
    };
    bind("#tool-balance", "#tool-balance-v", "balance", (v) => v.toFixed(2));
    bind("#tool-width", "#tool-width-v", "stereo_width", (v) => v.toFixed(2));
    bind("#tool-clip", "#tool-clip-v", "soft_clip", (v) => `${Math.round(v * 100)}%`);
    const phase = root.querySelector<HTMLInputElement>("#tool-phase");
    if (phase) { phase.checked = d.tools.phase_invert_left; phase.addEventListener("change", () => { d.tools.phase_invert_left = phase.checked; save(); }); }

    const cfEnabled = root.querySelector<HTMLInputElement>("#crossfeed-enabled");
    if (cfEnabled) {
      cfEnabled.checked = cf.enabled;
      cfEnabled.addEventListener("change", () => { cf.enabled = cfEnabled.checked; save(); });
    }
    const cfCutoff = root.querySelector<HTMLInputElement>("#crossfeed-cutoff");
    const cfCutoffV = root.querySelector<HTMLElement>("#crossfeed-cutoff-v");
    if (cfCutoff && cfCutoffV) {
      cfCutoff.value = String(cf.cutoff_hz);
      cfCutoffV.textContent = `${Math.round(cf.cutoff_hz)} Hz`;
      cfCutoff.addEventListener("input", () => {
        cf.cutoff_hz = Number(cfCutoff.value);
        cfCutoffV.textContent = `${Math.round(cf.cutoff_hz)} Hz`;
        save();
      });
    }
    const cfFeed = root.querySelector<HTMLInputElement>("#crossfeed-feed");
    const cfFeedV = root.querySelector<HTMLElement>("#crossfeed-feed-v");
    if (cfFeed && cfFeedV) {
      cfFeed.value = String(cf.feed_db);
      cfFeedV.textContent = `${cf.feed_db.toFixed(1)} dB`;
      cfFeed.addEventListener("input", () => {
        cf.feed_db = Number(cfFeed.value);
        cfFeedV.textContent = `${cf.feed_db.toFixed(1)} dB`;
        save();
      });
    }
    root.querySelector<HTMLButtonElement>("#crossfeed-bs2b")?.addEventListener("click", () => {
      cf.cutoff_hz = 700;
      cf.feed_db = -6.5;
      if (cfCutoff) cfCutoff.value = "700";
      if (cfCutoffV) cfCutoffV.textContent = "700 Hz";
      if (cfFeed) cfFeed.value = "-6.5";
      if (cfFeedV) cfFeedV.textContent = "-6.5 dB";
      save();
    });
    // --- Convolution reverb card ---
    const irNote = root.querySelector<HTMLElement>("#conv-ir-note");
    const cvEnabled = root.querySelector<HTMLInputElement>("#conv-enabled");
    if (cvEnabled) {
      cvEnabled.checked = cv.enabled;
      cvEnabled.addEventListener("change", () => { cv.enabled = cvEnabled.checked; save(); });
    }
    const cvIr = root.querySelector<HTMLSelectElement>("#conv-ir");
    if (cvIr) {
      // Map any user id back to the "User IR" option.
      const value = cv.ir_id === 0 ? "0" : cv.ir_id >= 100 ? "100" : String(cv.ir_id);
      cvIr.value = value;
      cvIr.addEventListener("change", () => {
        const v = Number(cvIr.value);
        cv.ir_id = v === 100 ? Math.max(cv.ir_id, 100) : v;
        save();
      });
    }
    const bindCv = (
      id: string,
      outId: string,
      key: "wet" | "dry" | "predelay_ms" | "ir_gain_db",
      fmt: (v: number) => string,
    ) => {
      const el = root.querySelector<HTMLInputElement>(id);
      const out = root.querySelector<HTMLElement>(outId);
      if (!el || !out) return;
      el.value = String(cv[key]);
      out.textContent = fmt(cv[key]);
      el.addEventListener("input", () => {
        cv[key] = Number(el.value);
        out.textContent = fmt(cv[key]);
        save();
      });
    };
    bindCv("#conv-wet", "#conv-wet-v", "wet", (v) => `${Math.round(v * 100)}%`);
    bindCv("#conv-dry", "#conv-dry-v", "dry", (v) => `${Math.round(v * 100)}%`);
    bindCv("#conv-predelay", "#conv-predelay-v", "predelay_ms", (v) => `${Math.round(v)} ms`);
    bindCv("#conv-irgain", "#conv-irgain-v", "ir_gain_db", (v) => `${v.toFixed(1)} dB`);
    const irFile = root.querySelector<HTMLInputElement>("#conv-ir-file");
    if (irFile) {
      irFile.addEventListener("change", async () => {
        const file = irFile.files?.[0];
        if (!file) return;
        const irNote = root.querySelector<HTMLElement>("#conv-ir-note");
        if (irNote) irNote.textContent = "Decoding IR…";
        try {
          if (file.size > 5 * 1024 * 1024) throw new Error("IR file exceeds the 5 MB cap");
          const buf = await file.arrayBuffer();
          // Decode via the browser's AudioContext (any container the
          // browser knows), then hand raw f32 channels to the wasm IR
          // store — the same shell-decode/kernel-process split as LUFS.
          const ctx = new AudioContext();
          try {
            const audio = await ctx.decodeAudioData(buf);
            if (audio.sampleRate < 8000 || audio.sampleRate > 768000) {
              throw new Error("implausible IR sample rate");
            }
            if (audio.length > Math.ceil(audio.sampleRate * 10)) {
              throw new Error("IR longer than the 10 s cap");
            }
            const left = audio.getChannelData(0);
            const right = audio.numberOfChannels > 1 ? audio.getChannelData(1) : new Float32Array(0);
            const id = 100;
            const ok = await setConvolutionIr(id, left, right, audio.sampleRate);
            if (!ok) throw new Error("the wasm kernel rejected the IR (non-finite samples?)");
            cv.ir_id = id;
            if (cvIr) cvIr.value = "100";
            if (irNote) {
              irNote.textContent = `User IR loaded: ${(audio.length / audio.sampleRate).toFixed(2)} s @ ${audio.sampleRate} Hz, ${audio.numberOfChannels} ch (mono upmixed in Rust).`;
            }
            save();
          } finally {
            void ctx.close();
          }
        } catch (e: unknown) {
          if (irNote) irNote.textContent = `IR load failed: ${String(e)}`;
        }
        irFile.value = "";
      });
    }
    // --- Room correction (REW) card ---
    const rcFilters = root.querySelector<HTMLElement>("#rc-filters");
    const drawFilters = () => {
      if (!rcFilters) return;
      rcFilters.innerHTML = "";
      for (const f of rc.filters) {
        const line = document.createElement("p");
        line.className = "hint";
        line.textContent = formatCorrectionFilter(f);
        rcFilters.appendChild(line);
      }
    };
    drawFilters();
    const rcEnabled = root.querySelector<HTMLInputElement>("#rc-enabled");
    if (rcEnabled) {
      rcEnabled.checked = rc.enabled;
      rcEnabled.addEventListener("change", () => { rc.enabled = rcEnabled.checked; save(); });
    }
    const rcFile = root.querySelector<HTMLInputElement>("#rc-file");
    if (rcFile) {
      rcFile.addEventListener("change", async () => {
        const file = rcFile.files?.[0];
        if (!file) return;
        const note = root.querySelector<HTMLElement>("#rc-note");
        try {
          if (file.size > 1024 * 1024) throw new Error("file exceeds the 1 MB cap");
          const text = await file.text();
          // Frontend parse for validation + preview; the kernel applies the
          // same filters from DspConfig (mirrored parser, tested identical).
          const filters: CorrectionFilter[] = parseRewFilters(text);
          rc.filters = filters;
          rc.enabled = true;
          if (rcEnabled) rcEnabled.checked = true;
          drawFilters();
          if (note) note.textContent = `REW import ok: ${filters.length} filters loaded.`;
          save();
        } catch (e: unknown) {
          const msg = e instanceof RewParseError ? e.message : String(e);
          if (note) note.textContent = `REW import failed: ${msg}`;
          setToast(`Room correction import failed: ${msg}`);
        }
        rcFile.value = "";
      });
    }
    drawState();

    root.querySelector<HTMLButtonElement>("#tool-reset")?.addEventListener("click", () => {
      d.tools = defaultTools();
      d.crossfeed = defaultCrossfeed();
      d.convolution = defaultConvolution();
      d.room_correction = defaultRoomCorrection();
      void save();
      effectsPage.mount(root);
    });
    root.querySelector<HTMLButtonElement>("#tool-save-track")?.addEventListener("click", () => {
      if (!currentTrack) return;
      void saveDspProfile(currentTrack, d).then(() => setToast("Saved tools to the current track profile.")).catch((e: unknown) => setToast(String(e)));
    });
    root.querySelector<HTMLButtonElement>("#tool-reset-track")?.addEventListener("click", () => {
      if (!currentTrack) return;
      void resetDspProfile(currentTrack).then(() => setToast("Track profile removed; global fallback remains.")).catch((e: unknown) => setToast(String(e)));
    });
    root.querySelector<HTMLButtonElement>("#effects-eq")?.addEventListener("click", () => navigate("eq"));
    root.querySelector<HTMLButtonElement>("#effects-plugins")?.addEventListener("click", () => navigate("plugins"));
  },
};
