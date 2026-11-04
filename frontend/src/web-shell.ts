/** Aurora Web shell — YTM-faithful layout (design direction winner).
 *
 * Anatomy (YTM): left guide rail + collapsible sidebar, top bar with
 * search pill, routed content, bottom player bar (72px) with volume +
 * quality badge, right queue drawer (340px), mobile tabs + miniplayer.
 * The one deliberate deviation: Braun-orange accent instead of YTM red
 * (owner's veto on a literal Google look — one-file swap if reverted).
 */

import { platform, invoke } from "./invoke";
import {
  app,
  loadStatus,
  nextTrack,
  playQueueIndex,
  prevTrack,
  setToast,
  subscribe,
} from "./state";
import { unlockAudio, flushOutput } from "./audio-bridge";
import { icon, type IconName } from "./styles/ytm-icons";
import "./styles/ytm.css";
import { homePage } from "./pages/home";

export function isWebShell(): boolean {
  return platform === "web";
}

interface NavEntry {
  route: string;
  label: string;
  icon: IconName;
}

const NAV: NavEntry[] = [
  { route: "home", label: "Home", icon: "home" },
  { route: "library", label: "Explore", icon: "explore" },
  { route: "queue", label: "Queue", icon: "queue" },
];

const LIBRARY_NAV: NavEntry[] = [
  { route: "playlists", label: "Playlists", icon: "queue" },
  { route: "eq", label: "Equalizer", icon: "more" },
  { route: "settings", label: "Settings", icon: "more" },
];

// Decks are NATIVE-ONLY for now: the wasm bridge drains only the main
// engine's ring — deck engines have no consumer on the web, so
// deck_play would report "playing" with permanent silence (a dishonest
// status per ADR 0002). Hidden until the ring mixer lands.
const MOBILE_TABS: NavEntry[] = [
  { route: "home", label: "Home", icon: "home" },
  { route: "library", label: "Explore", icon: "explore" },
  { route: "queue", label: "Queue", icon: "queue" },
];

/** Render the shell into #app. Returns the content root + nav elements. */
export function renderWebShell(root: HTMLElement): {
  content: HTMLElement;
  sidebar: HTMLElement;
  tabs: HTMLElement;
} {
  root.innerHTML = `
    <div class="ytm-app">
      <nav class="ytm-sidebar" id="ytm-sidebar">
        <div class="ytm-logo">
          <svg viewBox="0 0 24 24" width="26" height="26" aria-hidden="true">
            <!-- Aurora mark: an off-center pulse (waveform seed), Braun-orange -->
            <circle cx="12" cy="12" r="11" fill="#e8500a" opacity="0.92"/>
            <path d="M4 12h2.2l1.4-4.4L10.5 17l2.2-9 1.6 5.6 1.1-1.6H20" fill="none" stroke="#101014" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"/>
          </svg>
          <span>Aurora</span>
        </div>
        <p class="ytm-nav-section">Main</p>
        ${NAV.map(
          (n) =>
            `<button class="ytm-nav-item" data-route="${n.route}" type="button">${icon(n.icon)}<span>${n.label}</span></button>`,
        ).join("\n        ")}
        <p class="ytm-nav-section">Library</p>
        ${LIBRARY_NAV.map(
          (n) =>
            `<button class="ytm-nav-item" data-route="${n.route}" type="button">${icon(n.icon)}<span>${n.label}</span></button>`,
        ).join("\n        ")}
      </nav>

      <header class="ytm-topbar" id="ytm-topbar">
        <button class="ytm-btn-icon" id="ytm-guide-toggle" type="button" title="Toggle menu">${icon("more")}</button>
        <div class="ytm-search" id="ytm-topbar-search">
          ${icon("search", 20)}
          <input id="ytm-search-input" type="search" placeholder="Search songs, artists" autocomplete="off" />
          <button class="ytm-btn-icon" id="ytm-search-clear" type="button" title="Clear" hidden>${icon("close", 18)}</button>
        </div>
      </header>

      <main class="ytm-content" id="ytm-content"></main>

      <div class="ytm-miniplayer" id="ytm-mini">
        <div class="ytm-player-art" id="ytm-mini-art"></div>
        <div class="ytm-player-meta">
          <div class="ytm-player-title" id="ytm-mini-title">Aurora Web</div>
          <div class="ytm-player-artist" id="ytm-mini-artist">Drop music to begin</div>
        </div>
        <button class="ytm-btn-icon ytm-btn-play" id="ytm-mini-play" type="button">${icon("play")}</button>
      </div>

      <nav class="ytm-tabs" id="ytm-tabs">
        ${MOBILE_TABS.map(
          (t) =>
            `<button class="ytm-tab" data-route="${t.route}" type="button">${icon(t.icon)}<span>${t.label}</span></button>`,
        ).join("\n        ")}
      </nav>

      <aside class="ytm-drawer" id="ytm-drawer" aria-label="Queue">
        <header class="ytm-drawer-head">
          <span class="ytm-drawer-title">Queue</span>
          <button class="ytm-btn-icon" id="ytm-drawer-close" type="button" title="Close">${icon("close", 20)}</button>
        </header>
        <div class="ytm-drawer-body" id="ytm-drawer-body"></div>
      </aside>

      <footer class="ytm-player">
        <div class="ytm-player-now" id="ytm-now">
          <div class="ytm-player-art" id="ytm-art"></div>
          <div class="ytm-player-meta">
            <div class="ytm-player-title" id="ytm-title">Aurora Web</div>
            <div class="ytm-player-artist" id="ytm-artist">Drop music to begin</div>
          </div>
        </div>
        <div class="ytm-player-center">
          <div class="ytm-transport">
            <button class="ytm-btn-icon" id="ytm-prev" type="button" title="Previous">${icon("prev")}</button>
            <button class="ytm-btn-icon ytm-btn-play" id="ytm-play" type="button" title="Play / Pause">${icon("play")}</button>
            <button class="ytm-btn-icon" id="ytm-next" type="button" title="Next">${icon("next")}</button>
            <button class="ytm-btn-icon" id="ytm-stop" type="button" title="Stop">${icon("close")}</button>
          </div>
          <div class="ytm-progress">
            <span class="ytm-time" id="ytm-cur">0:00</span>
            <input id="ytm-seek" type="range" min="0" max="1000" step="1" value="0" />
            <span class="ytm-time" id="ytm-dur">0:00</span>
          </div>
        </div>
        <div class="ytm-player-right">
          <button class="ytm-btn-icon" id="ytm-drawer-toggle" type="button" title="Queue">${icon("queue")}</button>
          <span class="ytm-quality" id="ytm-quality" hidden></span>
          <div class="ytm-vol" id="ytm-vol">
            <button class="ytm-btn-icon" id="ytm-vol-btn" type="button" title="Volume">${icon("volume")}</button>
            <input id="ytm-vol-range" type="range" min="0" max="100" step="1" value="100" aria-label="Volume" />
          </div>
          <span class="ytm-player-artist" id="ytm-state">IDLE</span>
        </div>
      </footer>
    </div>
    <p class="toast" id="toast" role="status"></p>
  `;

  // Guide collapse (YTM hamburger): persisted.
  const guideToggle = document.getElementById("ytm-guide-toggle");
  guideToggle?.addEventListener("click", () => {
    const el = document.querySelector<HTMLElement>(".ytm-app");
    if (!el) return;
    const collapsed = el.classList.toggle("guide-collapsed");
    try {
      localStorage.setItem("aw:guide-collapsed", collapsed ? "1" : "0");
    } catch {
      /* private mode */
    }
  });
  try {
    if (localStorage.getItem("aw:guide-collapsed") === "1") {
      document.querySelector<HTMLElement>(".ytm-app")?.classList.add("guide-collapsed");
    }
  } catch {
    /* ignore */
  }

  // Topbar search: routes to home and drives its search input.
  const topSearch = document.getElementById("ytm-search-input") as HTMLInputElement | null;
  const forward = (value: string) => {
    const homeInput = document.querySelector<HTMLInputElement>("#home-search-input");
    if (homeInput) homeInput.value = value;
    homeInput?.dispatchEvent(new Event("input", { bubbles: true }));
  };
  topSearch?.addEventListener("input", () => {
    const clear = document.getElementById("ytm-search-clear");
    if (clear) clear.hidden = topSearch.value.length === 0;
    if (window.location.hash !== "#/home") window.location.hash = "#/home";
    forward(topSearch.value);
  });
  document.getElementById("ytm-search-clear")?.addEventListener("click", () => {
    if (topSearch) topSearch.value = "";
    const clear = document.getElementById("ytm-search-clear");
    if (clear) clear.hidden = true;
    forward("");
  });

  // Drawer open/close.
  const drawer = document.getElementById("ytm-drawer");
  document.getElementById("ytm-drawer-toggle")?.addEventListener("click", () => {
    drawer?.classList.toggle("open");
  });
  document.getElementById("ytm-drawer-close")?.addEventListener("click", () => {
    drawer?.classList.remove("open");
  });

  return {
    content: document.getElementById("ytm-content")!,
    sidebar: document.getElementById("ytm-sidebar")!,
    tabs: document.getElementById("ytm-tabs")!,
  };
}

// ---------------------------------------------------------------------------
// Player bar behavior (bound after the shell renders)
// ---------------------------------------------------------------------------

function fmtTime(s: number): string {
  if (!Number.isFinite(s) || s < 0) return "0:00";
  const m = Math.floor(s / 60);
  const sec = Math.floor(s % 60);
  return `${m}:${sec.toString().padStart(2, "0")}`;
}

export function bindWebPlayerBar(): void {
  const play = document.getElementById("ytm-play");
  const miniPlay = document.getElementById("ytm-mini-play");
  const next = document.getElementById("ytm-next");
  const prev = document.getElementById("ytm-prev");
  const seek = document.getElementById("ytm-seek") as HTMLInputElement | null;
  const now = document.getElementById("ytm-now");

  const toggle = async () => {
    unlockAudio(); // user gesture: unblock the browser's autoplay policy
    try {
      if (app.status?.state === "playing") {
        app.status = await invoke("pause_playback");
      } else if (app.status?.state === "paused") {
        app.status = await invoke("resume_playback");
      } else if (app.queueTracks.length) {
        await playQueueIndex(app.queueCurrent ?? 0);
      }
      await loadStatus();
    } catch (e) {
      setToast(`Play failed: ${String(e)}`);
    }
  };

  play?.addEventListener("click", toggle);
  miniPlay?.addEventListener("click", (e) => {
    e.stopPropagation();
    void toggle();
  });
  next?.addEventListener("click", () => void nextTrack());
  prev?.addEventListener("click", () => void prevTrack());
  document.getElementById("ytm-stop")?.addEventListener("click", async () => {
    try {
      app.status = await invoke("stop_playback");
      await loadStatus();
    } catch (e) {
      setToast(String(e));
    }
  });
  now?.addEventListener("click", () => {
    window.location.hash = "#/player";
  });
  // Mobile miniplayer tap -> fullscreen (same route; CSS handles the
  // mobile presentation).
  document.getElementById("ytm-mini")?.addEventListener("click", (e) => {
    if ((e.target as HTMLElement).closest("button")) return; // transport btn
    window.location.hash = "#/player";
  });

  // Volume: through the kernel's DSP (volume_db) — the transformation
  // stays visible in the Signal path (a JS GainNode would amplify
  // invisibly, lying in the ADR 0002 report). Persisted in kv.
  const volRange = document.getElementById("ytm-vol-range") as HTMLInputElement | null;
  const volBtn = document.getElementById("ytm-vol-btn");
  let lastVol = 100;
  void (async () => {
    try {
      const { kvGet } = await import("./library-db");
      const saved = await kvGet<number>("aw:volume");
      if (typeof saved === "number" && volRange) {
        volRange.value = String(saved);
        lastVol = saved;
        await applyVolume(saved);
      }
    } catch {
      /* defaults */
    }
  })();
  async function applyVolume(pct: number): Promise<void> {
    // 0..100 -> dB: 100% = 0 dB (unity), 50% ~ -14 dB, 0% = -80 dB.
    const db = pct <= 0 ? -80 : 60 * Math.log10(pct / 100);
    try {
      await invoke("set_dsp", { config: { ...(app.dsp ?? {}), volume_db: db } });
    } catch {
      /* kernel busy — retried on the next change */
    }
  }
  volRange?.addEventListener("input", () => {
    const pct = Number.parseFloat(volRange.value);
    void applyVolume(pct);
  });
  volRange?.addEventListener("change", () => {
    const pct = Number.parseFloat(volRange.value);
    lastVol = pct;
    void import("./library-db").then(({ kvPut }) => kvPut("aw:volume", pct)).catch(() => undefined);
  });
  volBtn?.addEventListener("click", () => {
    if (!volRange) return;
    const muted = Number.parseFloat(volRange.value) === 0;
    volRange.value = muted ? String(lastVol || 100) : "0";
    void applyVolume(muted ? lastVol || 100 : 0);
  });

  let seeking = false;
  seek?.addEventListener("pointerdown", () => (seeking = true));
  seek?.addEventListener("change", async () => {
    const pos = (Number.parseFloat(seek.value) / 1000) * app.durationS;
    try {
      flushOutput(); // drop browser-scheduled stale audio first
      await invoke("seek", { positionS: pos });
    } catch (e) {
      setToast(String(e));
    } finally {
      seeking = false;
    }
  });

  // Esc closes the drawer (and the fullscreen player back).
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      document.getElementById("ytm-drawer")?.classList.remove("open");
    }
  });
}

/** Update the player bar from status (status poll). */
export function renderWebBar(): void {
  const s = app.status;
  const current = app.queueCurrent != null ? app.queueTracks[app.queueCurrent] : null;

  const title = document.getElementById("ytm-title");
  const artist = document.getElementById("ytm-artist");
  const miniTitle = document.getElementById("ytm-mini-title");
  const miniArtist = document.getElementById("ytm-mini-artist");
  const stateEl = document.getElementById("ytm-state");

  const t = current?.title ?? (current ? (current.path.split(/[\\/]/).pop() ?? "") : "Aurora Web");
  const a = current?.artist ?? "";
  if (title) title.textContent = t;
  if (artist) artist.textContent = a;
  if (miniTitle) miniTitle.textContent = t;
  if (miniArtist) miniArtist.textContent = a || "Drop music to begin";
  if (stateEl) stateEl.textContent = s?.state?.toUpperCase() ?? "IDLE";

  // Play/pause icon.
  const playing = s?.state === "playing";
  for (const id of ["ytm-play", "ytm-mini-play"]) {
    const btn = document.getElementById(id);
    if (btn) btn.innerHTML = playing ? icon("pause") : icon("play");
  }

  // Seek bar: enabled whenever a duration is known.
  const seek = document.getElementById("ytm-seek") as HTMLInputElement | null;
  if (seek) seek.disabled = !(app.durationS > 0);

  renderDrawer();
}

/** Quality badge: honest format facts from the kernel's negotiated
 *  output (ADR 0002 in the UI — facts, not marketing). */
export function renderWebQuality(): void {
  const el = document.getElementById("ytm-quality");
  const s = app.status;
  if (!el) return;
  if (s?.sample_rate) {
    const bits = s.bits_per_sample ?? 32;
    el.hidden = false;
    el.textContent = `${(s.sample_rate / 1000).toFixed(1)}k/${bits}`;
  } else {
    el.hidden = true;
  }
}

/** Queue drawer body: current + up next (click to jump). */
export function renderDrawer(): void {
  const body = document.getElementById("ytm-drawer-body");
  if (!body) return;
  body.innerHTML = "";
  const head = document.createElement("p");
  head.className = "ytm-drawer-section";
  head.textContent = app.queueCurrent != null ? "Now playing" : "Queue";
  body.appendChild(head);

  app.queueTracks.forEach((t, idx) => {
    const row = document.createElement("button");
    row.type = "button";
    row.className = "ytm-drawer-row" + (idx === app.queueCurrent ? " current" : "");
    const title = document.createElement("span");
    title.textContent = t.title ?? t.path;
    const sub = document.createElement("span");
    sub.className = "sub";
    sub.textContent = t.artist ?? "";
    row.append(title, sub);
    row.addEventListener("click", () => void playQueueIndex(idx));
    body.appendChild(row);
  });
  if (!app.queueTracks.length) {
    const empty = document.createElement("p");
    empty.className = "ytm-drawer-empty";
    empty.textContent = "Queue is empty";
    body.appendChild(empty);
  }
}

/** Update the progress bar (position poll). */
export function renderWebProgress(positionS: number): void {
  const seek = document.getElementById("ytm-seek") as HTMLInputElement | null;
  const cur = document.getElementById("ytm-cur");
  const dur = document.getElementById("ytm-dur");
  if (seek && app.durationS > 0) {
    seek.value = String(Math.min(1000, Math.round((positionS / app.durationS) * 1000)));
  }
  if (cur) cur.textContent = fmtTime(positionS);
  if (dur && app.durationS > 0) dur.textContent = fmtTime(app.durationS);
  // Miniplayer progress hairline.
  const mini = document.getElementById("ytm-mini");
  if (mini && app.durationS > 0) {
    mini.style.setProperty("--mini-progress", `${Math.min(100, (positionS / app.durationS) * 100)}%`);
  }
}

/** Register the web-only routes (home). Call before startRouter. */
export { homePage };
