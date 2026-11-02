/** Plugins page: VST3 scan, attach, chain, editor, generic params, presets. */

import { invoke } from "@tauri-apps/api/core";
import type { Page } from "../router";
import { setToast } from "../state";
import type { VstParamDto, VstPresetDto, VstPluginDto, VstSlot } from "../types";

let found: VstPluginDto[] = [];
let slots: VstSlot[] = [];
let paramsFor: { path: string; class_id: string } | null = null;
let paramsList: VstParamDto[] = [];
/** Bypassed slot indices (A/B compare). */
let bypassedSlots = new Set<number>();
/** One-shot chain restore flag (library auto-open may race the mount). */
let restored = false;

function note(text: string, isError = false) {
  const el = document.getElementById("vst-note");
  if (el) {
    el.textContent = text;
    el.classList.toggle("err", isError);
  }
}

async function applyChain() {
  try {
    await invoke("set_vst_chain", { slots });
    // Persist so the chain survives restarts (best effort: the library
    // may not be open).
    await invoke("save_vst_chain_setting", { slots: slots.length ? slots : null }).catch(
      () => {},
    );
    renderChain(document.getElementById("page-root")!);
  } catch (e) {
    note(`chain failed: ${String(e)}`, true);
  }
}

function renderChain(root: HTMLElement) {
  update(root);
}

function render(root: HTMLElement) {
  root.innerHTML = `
    <section class="page">
      <header class="page-head">
        <div>
          <p class="overline">VST3 · out-of-process chain</p>
          <h1 class="page-title">Plugins</h1>
        </div>
        <div class="page-actions">
          <button class="btn" id="vst-scan" type="button">Scan…</button>
          <button class="btn" id="vst-clear" type="button">Chain off</button>
        </div>
      </header>

      <ul id="vst-list" class="vst-list"></ul>
      <p class="hint" id="vst-note"></p>
      <p class="hint">Plugins run in isolated helper processes — a crashing plugin never takes the player down. Active plugins disable bit-perfect (honest report).</p>
    </section>
  `;

  root.querySelector<HTMLButtonElement>("#vst-scan")?.addEventListener("click", async () => {
    const btn = root.querySelector<HTMLButtonElement>("#vst-scan");
    try {
      if (btn) {
        btn.disabled = true;
        btn.textContent = "Scanning…";
      }
      note("scanning VST3 folders (crash-safe out-of-process probe)…");
      found = await invoke<VstPluginDto[]>("scan_vst_plugins", { extraFolder: null });
      if (!found.length) {
        note("no VST3 plugins found — install one into %LOCALAPPDATA%\\VST3");
        return;
      }
      note(`${found.length} plugin(s) found.`);
    } catch (e) {
      note(`scan failed: ${String(e)}`, true);
    } finally {
      if (btn) {
        btn.disabled = false;
        btn.textContent = "Scan…";
      }
      update(root);
    }
  });

  root.querySelector<HTMLButtonElement>("#vst-clear")?.addEventListener("click", async () => {
    slots = [];
    paramsFor = null;
    paramsList = [];
    bypassedSlots.clear();
    await applyChain();
    note("chain off");
  });

  // Restore the persisted chain (library settings) once at mount.
  if (!restored) {
    restored = true;
    void (async () => {
      try {
        const saved = await invoke<VstSlot[] | null>("load_vst_chain_setting");
        if (saved?.length) {
          slots = saved;
          await invoke("set_vst_chain", { slots });
          note(`Chain restored: ${slots.length} plugin(s)`);
          update(root);
        }
      } catch {
        /* library may not be open yet */
      }
    })();
  }
  // Refresh bypass state from the engine.
  void invoke<number[]>("vst_bypassed_slots")
    .then((list) => {
      bypassedSlots = new Set(list);
    })
    .catch(() => {});

  update(root);
}

function update(root: HTMLElement) {
  const ul = root.querySelector<HTMLUListElement>("#vst-list");
  if (!ul) return;
  ul.innerHTML = "";

  // Found plugins: attach row.
  for (const p of found) {
    const attached = slots.some((s) => s.path === p.path && s.class_id === p.class_id);
    const li = document.createElement("li");
    li.className = "track-row";
    const name = document.createElement("span");
    name.className = "track-name";
    const stem = p.path.split(/[\\/]/).pop() ?? p.path;
    name.textContent = `${p.name} · ${p.vendor} ${p.version} — ${stem}`;
    name.title = p.path;
    const add = document.createElement("button");
    add.type = "button";
    add.className = "btn";
    add.textContent = attached ? "✓ In chain" : "Attach";
    add.disabled = attached;
    add.addEventListener("click", async () => {
      add.disabled = true;
      add.textContent = "Testing…";
      note(`loading ${p.name} in an isolated process…`);
      try {
        const verdict = await invoke<string>("test_vst_plugin", {
          path: p.path,
          classId: p.class_id,
        });
        slots.push({ path: p.path, class_id: p.class_id });
        await applyChain();
        note(verdict);
      } catch (e) {
        const msg = typeof e === "object" && e && "message" in e
          ? String((e as { message: unknown }).message)
          : String(e);
        note(`${p.name}: ${msg}`, true);
      } finally {
        update(root);
      }
    });
    li.append(name, add);
    ul.appendChild(li);
  }

  // Attached: editor / params / presets rows.
  for (const p of found) {
    const attached = slots.some((s) => s.path === p.path && s.class_id === p.class_id);
    if (!attached) continue;

    const li = document.createElement("li");
    li.className = "track-row sub";
    const label = document.createElement("span");
    label.className = "track-name";
    label.textContent = p.name;
    li.appendChild(label);

    const open = document.createElement("button");
    open.type = "button";
    open.className = "btn";
    open.textContent = "Editor";
    open.title = "Open the plugin's own GUI window";
    open.addEventListener("click", async () => {
      try {
        open.disabled = true;
        open.textContent = "Opening…";
        await invoke("open_vst_editor", { path: p.path, classId: p.class_id });
        note(`${p.name} editor open — tweak knobs there; the chain hears it.`);
      } catch (e) {
        const msg = typeof e === "object" && e && "message" in e
          ? String((e as { message: unknown }).message)
          : String(e);
        note(`${p.name} editor: ${msg}`, true);
      } finally {
        open.disabled = false;
        open.textContent = "Editor";
      }
    });
    li.appendChild(open);

    const btnP = document.createElement("button");
    btnP.type = "button";
    btnP.className = "btn";
    btnP.textContent =
      paramsFor && paramsFor.path === p.path && paramsFor.class_id === p.class_id
        ? "Hide"
        : "Params";
    btnP.addEventListener("click", async () => {
      const same =
        paramsFor && paramsFor.path === p.path && paramsFor.class_id === p.class_id;
      if (same) {
        paramsFor = null;
        paramsList = [];
        update(root);
        return;
      }
      try {
        btnP.disabled = true;
        btnP.textContent = "…";
        paramsList = await invoke<VstParamDto[]>("list_vst_params", {
          path: p.path,
          classId: p.class_id,
        });
        paramsFor = { path: p.path, class_id: p.class_id };
      } catch (e) {
        note(`${p.name} params: ${String(e)}`, true);
      } finally {
        update(root);
      }
    });
    li.appendChild(btnP);

    const saveBtn = document.createElement("button");
    saveBtn.type = "button";
    saveBtn.className = "btn";
    saveBtn.textContent = "Save preset";
    saveBtn.addEventListener("click", async () => {
      const nm = prompt(`Preset name for ${p.name}`);
      if (!nm) return;
      try {
        saveBtn.disabled = true;
        const file = await invoke<string>("save_vst_preset", {
          path: p.path,
          classId: p.class_id,
          name: nm,
        });
        note(`Preset saved: ${file}`);
      } catch (e) {
        note(`Preset save failed: ${String(e)}`, true);
      } finally {
        saveBtn.disabled = false;
      }
    });
    li.appendChild(saveBtn);

    ul.appendChild(li);
  }

  // Generic params panel + presets.
  if (paramsFor) {
    const wrap = document.createElement("li");
    wrap.className = "param-panel";
    const { path, class_id } = paramsFor;

    void invoke<VstPresetDto[]>("list_vst_presets", { path })
      .then((presets) => {
        if (presets.length) {
          const head = document.createElement("p");
          head.className = "overline";
          head.textContent = "Presets";
          wrap.appendChild(head);
          const row = document.createElement("div");
          row.className = "preset-row";
          for (const preset of presets) {
            const b = document.createElement("button");
            b.type = "button";
            b.className = "btn";
            b.textContent = preset.name;
            b.addEventListener("click", async () => {
              try {
                b.disabled = true;
                await invoke("load_vst_preset", {
                  path,
                  classId: class_id,
                  presetPath: preset.path,
                });
                paramsList = await invoke<VstParamDto[]>("list_vst_params", {
                  path,
                  classId: class_id,
                });
                update(root);
                note(`Preset loaded: ${preset.name}`);
              } catch (e) {
                note(`Preset load failed: ${String(e)}`, true);
              } finally {
                b.disabled = false;
              }
            });
            row.appendChild(b);
          }
          wrap.appendChild(row);
        }
        appendSliders(wrap, root);
      })
      .catch(() => appendSliders(wrap, root));

    ul.appendChild(wrap);
  }

  // Active chain.
  slots.forEach((slot, idx) => {
    const li = document.createElement("li");
    li.className = "track-row current";
    const name = document.createElement("span");
    name.className = "track-name";
    const stem = slot.path.split(/[\\/]/).pop() ?? slot.path;
    name.textContent = `CHAIN ${idx + 1}: ${stem}`;
    const del = document.createElement("button");
    del.type = "button";
    del.className = "row-del";
    del.textContent = "×";
    del.title = "Remove from chain";
    del.addEventListener("click", async () => {
      slots.splice(idx, 1);
      await applyChain();
    });

    // Bypass (A/B): instant compare with/without this slot.
    const bypassBtn = document.createElement("button");
    bypassBtn.type = "button";
    bypassBtn.className = "btn";
    bypassBtn.textContent = bypassedSlots.has(idx) ? "Bypassed" : "Bypass";
    bypassBtn.title = "Route audio around this slot (A/B compare) — the plugin stays live";
    bypassBtn.addEventListener("click", async () => {
      const next = !bypassedSlots.has(idx);
      try {
        await invoke("set_vst_slot_bypass", { slot: idx, bypassed: next });
        if (next) bypassedSlots.add(idx);
        else bypassedSlots.delete(idx);
        update(root);
        note(next ? `Slot ${idx + 1} bypassed` : `Slot ${idx + 1} active`);
      } catch (e) {
        note(`bypass failed: ${String(e)}`, true);
      }
    });
    li.append(name, bypassBtn, del);
    ul.appendChild(li);
  });

  note(
    slots.length
      ? `${slots.length} plugin${slots.length > 1 ? "s" : ""} in chain — active while a track plays`
      : found.length
        ? "found plugins above; attach one to enable"
        : "chain off — press Scan…",
  );
}

function appendSliders(wrap: HTMLElement, root: HTMLElement) {
  if (!paramsFor || !paramsList.length) return;
  const { path, class_id } = paramsFor;
  const head = document.createElement("p");
  head.className = "overline";
  head.textContent = "Parameters";
  wrap.appendChild(head);
  const grid = document.createElement("div");
  grid.className = "param-grid";
  for (const prm of paramsList.slice(0, 24)) {
    const cell = document.createElement("label");
    cell.className = "ctl";
    const title = document.createElement("span");
    title.className = "param-title";
    title.textContent = prm.title;
    title.title = prm.title;
    const slider = document.createElement("input");
    slider.type = "range";
    slider.min = "0";
    slider.max = "1";
    slider.step = "0.001";
    slider.value = String(prm.value);
    const out = document.createElement("b");
    out.textContent = prm.value.toFixed(3);
    let timer: ReturnType<typeof setTimeout> | null = null;
    slider.addEventListener("input", () => {
      const v = Number.parseFloat(slider.value);
      out.textContent = v.toFixed(3);
      if (timer) clearTimeout(timer);
      timer = setTimeout(() => {
        void invoke("set_vst_param", {
          path,
          classId: class_id,
          id: prm.id,
          value: v,
        }).catch((e) => note(`param set failed: ${String(e)}`, true));
      }, 60);
    });
    cell.append(title, slider, out);
    grid.appendChild(cell);
  }
  wrap.appendChild(grid);
  if (paramsList.length > 24) {
    const more = document.createElement("p");
    more.className = "hint";
    more.textContent = `+${paramsList.length - 24} more parameters (use the plugin editor)`;
    wrap.appendChild(more);
  }
  void root;
}

export const pluginsPage: Page = {
  id: "plugins",
  title: "Plugins",
  mount: render,
  update,
};
