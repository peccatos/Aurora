import type { Page } from "../router";
import { navigate } from "../router";
import { app, setToast } from "../state";
import { invoke, platform } from "../invoke";
import { kvPut } from "../library-db";
import { resetDspProfile, saveDspProfile } from "../dsp-profiles";
import type { DspConfig } from "../types";

type ToolKey = "balance" | "stereo_width" | "soft_clip";

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

export const effectsPage: Page = {
  id: "effects",
  title: "Effects & Tools",
  mount(root) {
    if (!app.dsp) return;
    const d = app.dsp;
    ensureTools(d);
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
    drawState();

    root.querySelector<HTMLButtonElement>("#tool-reset")?.addEventListener("click", () => {
      d.tools = defaultTools();
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
