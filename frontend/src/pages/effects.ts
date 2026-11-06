import type { Page } from "../router";
import { navigate } from "../router";

export const effectsPage: Page = {
  id: "effects",
  title: "Effects & Tools",
  mount(root) {
    root.innerHTML = `<section class="page"><header class="page-head"><div><p class="overline">Signal tools</p><h1 class="page-title">Effects & Tools</h1></div></header><p class="hint">Choose a browser-supported effect or the desktop-only VST3 toolchain. Profiles are applied per track when available and fall back to the global DSP.</p><div class="page-actions"><button class="btn" id="effects-eq" type="button">Equalizer & DSP</button><button class="btn" id="effects-plugins" type="button">Desktop VST3 plugins</button></div></section>`;
    root.querySelector<HTMLButtonElement>("#effects-eq")?.addEventListener("click", () => navigate("eq"));
    root.querySelector<HTMLButtonElement>("#effects-plugins")?.addEventListener("click", () => navigate("plugins"));
  },
};
