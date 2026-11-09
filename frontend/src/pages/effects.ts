import type { Page } from "../router";
import { navigate } from "../router";
import { app, setToast } from "../state";
import { invoke, platform } from "../invoke";
import { kvPut } from "../library-db";
import { resetDspProfile, saveDspProfile } from "../dsp-profiles";
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
    drawState();

    root.querySelector<HTMLButtonElement>("#tool-reset")?.addEventListener("click", () => {
      d.tools = defaultTools();
      d.crossfeed = defaultCrossfeed();
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
