/** YTM-style web shell (Aurora Web).
 *
 * When the app runs in a plain browser (no Tauri), this shell renders
 * the YouTube Music layout one-to-one: sidebar + shelves + player bar
 * (desktop), bottom tabs + miniplayer (mobile). The pages mounted
 * inside are the SAME page modules the desktop shell uses — the router
 * doesn't care which shell hosts it.
 */

import { platform, invoke } from "./invoke";
import { app, loadStatus, nextTrack, playQueueIndex, prevTrack, setQueue, setToast } from "./state";
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
  { route: "decks", label: "Decks", icon: "more" },
  { route: "eq", label: "EQ", icon: "more" },
  { route: "settings", label: "Settings", icon: "more" },
];

const MOBILE_TABS: NavEntry[] = [
  { route: "home", label: "Home", icon: "home" },
  { route: "library", label: "Explore", icon: "explore" },
  { route: "queue", label: "Library", icon: "library" },
];

/** Render the YTM shell into #app. Returns the content root + nav
 *  elements the router needs. */
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
        ${NAV.map(
          (n) =>
            `<button class="ytm-nav-item" data-route="${n.route}" type="button">${icon(n.icon)}<span>${n.label}</span></button>`,
        ).join("\n        ")}
      </nav>

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
          <span class="ytm-player-artist" id="ytm-state">IDLE</span>
        </div>
      </footer>
    </div>
    <p class="toast" id="toast" role="status"></p>
  `;

  return {
    content: document.getElementById("ytm-content")!,
    sidebar: document.getElementById("ytm-sidebar")!,
    tabs: document.getElementById("ytm-tabs")!,
  };
}

// ---------------------------------------------------------------------------
// YTM player bar behavior (bound after the shell renders)
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
    // Fullscreen now-playing: swap the content for the player page.
    window.location.hash = "#/player";
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
}

/** Update the YTM bar from status (called from the status poll). */
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
}

/** Update the YTM progress bar (from the position poll). */
export function renderWebProgress(positionS: number): void {
  const seek = document.getElementById("ytm-seek") as HTMLInputElement | null;
  const cur = document.getElementById("ytm-cur");
  const dur = document.getElementById("ytm-dur");
  if (seek && app.durationS > 0) {
    seek.value = String(Math.min(1000, Math.round((positionS / app.durationS) * 1000)));
  }
  if (cur) cur.textContent = fmtTime(positionS);
  if (dur && app.durationS > 0) dur.textContent = fmtTime(app.durationS);
}

/** Register the web-only routes (home). Call before startRouter. */
export { homePage };
