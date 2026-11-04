/** Aurora — app shell.
 *  Swiss-minimal / Material-grid layout: left rail navigation, routed
 *  pages, one global player bar. Pages live in ./pages, state in ./state,
 *  routes in ./router (hash-based: #/player, #/queue, …). */

import { invoke, platform } from "./invoke";
import { onKernelEvent } from "./invoke";
const open = (...a: Parameters<typeof import("@tauri-apps/plugin-dialog")["open"]>) =>
  import("@tauri-apps/plugin-dialog").then((m) => m.open(...a));
import "./styles/base.css";
import "./styles/components.css";
import "./styles/player.css";
import "./styles/pages.css";

import { navigate, refreshActive, registerPage, startRouter, activeRoute } from "./router";
import { app, loadStatus, nextTrack, playQueueIndex, prevTrack, setQueue, setToast, subscribe } from "./state";
import { registerDrop } from "./drop";
import { loadDsp } from "./pages/eq";
import { playerPage, animatePlayer } from "./pages/player";
import { queuePage } from "./pages/queue";
import { libraryPage } from "./pages/library";
import { eqPage } from "./pages/eq";
import { pluginsPage } from "./pages/plugins";
import { settingsPage } from "./pages/settings";
import { decksPage } from "./pages/decks";
import { playlistsPage, playlistDetailPage } from "./pages/playlists";
import {
  isWebShell,
  renderWebShell,
  bindWebPlayerBar,
  renderWebBar,
  renderWebQuality,
  renderWebProgress,
  homePage,
} from "./web-shell";
import type { PlaybackStatus, QueueDto } from "./types";

registerPage(playerPage);
registerPage(queuePage);
registerPage(libraryPage);
registerPage(eqPage);
registerPage(pluginsPage);
registerPage(settingsPage);
// Decks page is native-only on the web shell (deck engines have no
// ring consumer there — deck_play would be silent "playing"). The
// page itself stays registered for the desktop shell.
if (!isWebShell()) registerPage(decksPage);
if (isWebShell()) {
  registerPage(homePage);
  registerPage(playlistsPage);
  registerPage(playlistDetailPage);
}

const shell = document.querySelector<HTMLDivElement>("#app");
if (!shell) throw new Error("Application root #app was not found");

// ---------------------------------------------------------------------------
// Web shell (YTM one-to-one): different chrome, same pages/router.
// ---------------------------------------------------------------------------
if (isWebShell()) {
  const { content, sidebar, tabs } = renderWebShell(shell);
  bindWebPlayerBar();

  // Default route: Home.
  if (!window.location.hash) window.location.hash = "#/home";
  startRouter(content, sidebar);

  const clickNav = (ev: Event) => {
    const btn = (ev.target as HTMLElement).closest<HTMLElement>("[data-route]");
    if (btn?.dataset.route) navigate(btn.dataset.route);
  };
  sidebar.addEventListener("click", clickNav);
  tabs.addEventListener("click", clickNav);

  // MediaSession (W6): lock-screen / hardware media keys — metadata +
  // transport handlers. Guarded: Safari/older browsers lack it.
  if ("mediaSession" in navigator) {
    const ms = navigator.mediaSession;
    ms.setActionHandler("play", () => void resumeOrPlay());
    ms.setActionHandler("pause", () => void invoke("pause_playback").then(() => loadStatus()));
    ms.setActionHandler("previoustrack", () => void prevTrack());
    ms.setActionHandler("nexttrack", () =>
      void nextTrack().then((q) => {
        if (!q && app.queueTracks.length === 1) void playQueueIndex(0);
      }),
    );
  }

  async function resumeOrPlay(): Promise<void> {
    try {
      if (app.status?.state === "paused") {
        await invoke("resume_playback");
      } else if (app.queueTracks.length && app.status?.state !== "playing") {
        await playQueueIndex(app.queueCurrent ?? 0);
      }
      await loadStatus();
    } catch (e) {
      setToast(String(e));
    }
  }

  // Update MediaSession metadata when the current track changes.
  const updateMediaMetadata = () => {
    if (!("mediaSession" in navigator)) return;
    const cur = app.queueCurrent != null ? app.queueTracks[app.queueCurrent] : null;
    if (!cur) return;
    try {
      navigator.mediaSession.metadata = new MediaMetadata({
        title: cur.title ?? "Unknown",
        artist: cur.artist ?? "",
        album: "",
      });
    } catch {
      /* unsupported metadata shape */
    }
  };
  subscribe(updateMediaMetadata);

  // Keyboard shortcuts (W6): space play/pause, arrows seek/volume, /
  // focuses search.
  document.addEventListener("keydown", (e) => {
    const inField = (e.target as HTMLElement)?.matches?.("input, select, textarea");
    if (inField) return;
    if (e.code === "Space") {
      e.preventDefault();
      void (async () => {
        try {
          if (app.status?.state === "playing") {
            app.status = await invoke("pause_playback");
          } else {
            await resumeOrPlay();
          }
          await loadStatus();
        } catch {
          /* ignore */
        }
      })();
    } else if (e.key === "ArrowRight") {
      void invoke("seek", { positionS: Math.min(app.durationS, (app.status ? 0 : 0) + posFromSeek() + 5) });
    } else if (e.key === "ArrowLeft") {
      void invoke("seek", { positionS: Math.max(0, posFromSeek() - 5) });
    } else if (e.key === "n") {
      void nextTrack();
    } else if (e.key === "p") {
      void prevTrack();
    } else if (e.key === "/") {
      e.preventDefault();
      const inp = document.querySelector<HTMLInputElement>("#ytm-search-input");
      inp?.focus();
    }
  });

  function posFromSeek(): number {
    const bar = document.getElementById("ytm-seek") as HTMLInputElement | null;
    if (bar && app.durationS > 0) {
      return (Number.parseFloat(bar.value) / 1000) * app.durationS;
    }
    return 0;
  }

  // Web polls: status + progress through the YTM bar.
  setInterval(async () => {
    try {
      await loadStatus();
      renderWebBar();
      renderWebQuality();
      refreshActive();
    } catch {
      /* kernel busy */
    }
  }, 500);

  setInterval(async () => {
    try {
      const pos = await invoke<number>("playback_position");
      renderWebProgress(pos);
      // End-of-track threshold: the engine's position counts bytes HANDED
      // to the bridge, which schedules ~AHEAD_MS ahead of the audible
      // clock — cutting at duration-0.3 would clip the audible tail.
      // Wait for the scheduled audio to have sounded.
      const AHEAD_S = 0.25; // bridge schedule window + scheduling slack
      if (app.durationS > 0 && pos >= app.durationS - 0.3 + AHEAD_S) {
        const q = await nextTrack();
        if (!q && app.queueTracks.length === 1) {
          // Single-track queue replay: use the WRAPPER (restores
          // queueCurrent/selectedPath + refreshes status), not the raw
          // invoke — the raw call left the UI showing defaults while
          // the track actually replayed.
          await playQueueIndex(0);
        }
      }
    } catch {
      /* not playing */
    }
  }, 400);

  void loadStatus().then(() => {
    renderWebBar();
    renderWebQuality();
  });

  // Player page animation (web shell too — the desktop branch had its
  // own ticker; without this the fullscreen player's waveform/lyrics
  // sync never ticked on the web).
  setInterval(() => {
    if (activeRoute() === "player") animatePlayer();
  }, 120);
} else {
  mainDesktop();
}

// ---------------------------------------------------------------------------
// Desktop shell (the original layout) — runs only under Tauri.
// ---------------------------------------------------------------------------

function mainDesktop(): void {

shell!.innerHTML = `
  <div class="shell">
    <nav class="rail" id="rail">
      <div class="rail-brand">
        <span class="rail-mark">A</span>
        <span class="rail-name">Aurora</span>
      </div>
      <button class="nav-item" data-route="player" type="button">Player</button>
      <button class="nav-item" data-route="queue" type="button">Queue</button>
      <button class="nav-item" data-route="library" type="button">Library</button>
      <button class="nav-item" data-route="decks" type="button">Decks</button>
      <button class="nav-item" data-route="eq" type="button">Equalizer</button>
      <button class="nav-item" data-route="plugins" type="button">Plugins</button>
      <button class="nav-item" data-route="settings" type="button">Settings</button>
      <div class="rail-foot">
        <span class="rail-meta" id="rail-count"></span>
        <span class="rail-ver">Aurora 0.1.0</span>
      </div>
    </nav>

    <main class="content" id="page-root"></main>
  </div>

  <footer class="playerbar">
    <div class="pb-now">
      <span class="pb-title" id="pb-title">Aurora</span>
      <span class="pb-artist" id="pb-artist">Local audio</span>
    </div>
    <div class="pb-center">
      <div class="pb-controls">
        <button id="btn-shuffle" class="pb-btn" type="button" title="Shuffle">⇄</button>
        <button id="btn-prev" class="pb-btn" type="button" disabled title="Previous">⏮</button>
        <button id="btn-play" class="pb-btn pb-main" type="button" disabled title="Play / Pause">▶</button>
        <button id="btn-stop" class="pb-btn" type="button" disabled title="Stop">⏹</button>
        <button id="btn-next" class="pb-btn" type="button" disabled title="Next">⏭</button>
      </div>
      <div class="seek-row">
        <span id="seek-cur">0:00</span>
        <input id="seek" type="range" min="0" max="100" step="0.1" value="0" disabled />
        <span id="seek-dur">0:00</span>
      </div>
    </div>
    <div class="pb-right">
      <span class="pb-state" id="st-state-mini">IDLE</span>
      <span class="pb-int" id="st-integrity-mini">UNVERIFIED</span>
    </div>
  </footer>
  <p class="toast" id="toast" role="status"></p>
`;

// ---------------------------------------------------------------------------
// Router + rail
// ---------------------------------------------------------------------------

const rail = document.getElementById("rail")!;
const pageRoot = document.getElementById("page-root")!;
startRouter(pageRoot, rail);

rail.addEventListener("click", (ev) => {
  const btn = (ev.target as HTMLElement).closest<HTMLElement>(".nav-item");
  if (!btn?.dataset.route) return;
  navigate(btn.dataset.route);
});

// ---------------------------------------------------------------------------
// Player bar transport
// ---------------------------------------------------------------------------

let shuffleMode = false;

function fmtTime(s: number): string {
  if (!Number.isFinite(s) || s < 0) return "0:00";
  const m = Math.floor(s / 60);
  const sec = Math.floor(s % 60);
  return `${m}:${sec.toString().padStart(2, "0")}`;
}

async function playFromBar() {
  if (!app.selectedPath) return;
  try {
    if (app.status?.state === "playing") {
      app.status = await invoke<PlaybackStatus>("pause_playback");
      setToast("Paused.");
    } else if (app.status?.state === "paused") {
      app.status = await invoke<PlaybackStatus>("resume_playback");
      setToast("Resumed.");
    } else if (app.queueTracks.length > 1) {
      const q = await invoke<QueueDto>("play_queue_index", { index: app.queueCurrent ?? 0 });
      app.queueCurrent = q.current;
    } else {
      const current = app.queueTracks[app.queueCurrent ?? 0];
      app.status = await invoke<PlaybackStatus>("play_track", {
        request: {
          path: app.selectedPath,
          device: app.device || null,
          replaygain_track_db: current?.replaygain_track_db ?? null,
          replaygain_album_db: current?.replaygain_album_db ?? null,
        },
      });
      setToast("Playing.");
    }
  } catch (e) {
    setToast(`Play failed: ${String(e)}`);
  }
}

document.getElementById("btn-play")?.addEventListener("click", playFromBar);

document.getElementById("btn-stop")?.addEventListener("click", async () => {
  try {
    app.status = await invoke<PlaybackStatus>("stop_playback");
    setToast("Stopped.");
  } catch (e) {
    setToast(`Stop failed: ${String(e)}`);
  }
});

document.getElementById("btn-next")?.addEventListener("click", async () => {
  const q = await nextTrack();
  if (q) {
    setToast(`${q.display[q.current ?? 0] ?? ""} (${(q.current ?? 0) + 1}/${q.tracks.length})`);
  } else if (app.queueTracks.length === 1) {
    await invoke("play_queue_index", { index: 0 });
    setToast("Replay.");
  } else {
    setToast("End of queue");
  }
});

document.getElementById("btn-prev")?.addEventListener("click", async () => {
  const q = await prevTrack();
  setToast(`${q.display[q.current ?? 0] ?? ""} (${(q.current ?? 0) + 1}/${q.tracks.length})`);
});

document.getElementById("btn-shuffle")?.addEventListener("click", async () => {
  shuffleMode = !shuffleMode;
  if (shuffleMode && app.queueTracks.length > 1) {
    const cur = app.queueCurrent ?? 0;
    const head = app.queueTracks[cur];
    const rest = app.queueTracks.filter((_, i) => i !== cur);
    for (let i = rest.length - 1; i > 0; i--) {
      const j = Math.floor(Math.random() * (i + 1));
      [rest[i], rest[j]] = [rest[j], rest[i]];
    }
    app.queueTracks = [head, ...rest];
    app.queueCurrent = 0;
    await setQueue(app.queueTracks);
    setToast("Shuffled queue");
  } else {
    setToast("Shuffle off");
  }
});

// Seek: click-drag sends the position; polling updates the bar.
let seeking = false;
const seekBar = document.getElementById("seek") as HTMLInputElement;
seekBar.addEventListener("pointerdown", () => (seeking = true));
seekBar.addEventListener("change", async () => {
  const pos = (Number.parseFloat(seekBar.value) / 100) * app.durationS;
  try {
    await invoke("seek", { positionS: pos });
    const cur = document.getElementById("seek-cur");
    if (cur) cur.textContent = fmtTime(pos);
  } catch (e) {
    setToast(`Seek failed: ${String(e)}`);
  } finally {
    seeking = false;
  }
});
seekBar.addEventListener("pointerup", () => {
  setTimeout(() => (seeking = false), 250);
});

// ---------------------------------------------------------------------------
// Status polling + bar rendering
// ---------------------------------------------------------------------------

let lastState: string | null = null;

function renderBar() {
  const s = app.status;
  const current = app.queueCurrent != null ? app.queueTracks[app.queueCurrent] : null;
  const titleEl = document.getElementById("pb-title");
  const artistEl = document.getElementById("pb-artist");
  if (titleEl && current) {
    const stem = current.path.split(/[\\/]/).pop() ?? current.path;
    titleEl.textContent = current.title ?? stem;
    if (artistEl) artistEl.textContent = current.artist ?? "";
  } else if (titleEl && app.selectedPath) {
    titleEl.textContent = app.selectedPath.split(/[\\/]/).pop() ?? "";
    if (artistEl) artistEl.textContent = "";
  }

  const stateMini = document.getElementById("st-state-mini");
  if (stateMini && s) stateMini.textContent = s.state.toUpperCase();
  const intMini = document.getElementById("st-integrity-mini");
  if (intMini && s) {
    intMini.textContent = s.integrity.toUpperCase();
    intMini.className = "pb-int " + (s.integrity === "verified" ? "ok" : "");
  }

  const playBtn = document.querySelector<HTMLButtonElement>(".pb-main");
  if (playBtn) {
    playBtn.textContent =
      s?.state === "playing" ? "⏸" : s?.state === "paused" ? "⏵" : "▶";
  }
  const btnPlay = document.getElementById("btn-play") as HTMLButtonElement;
  if (btnPlay) btnPlay.disabled = !app.selectedPath;
  const btnStop = document.getElementById("btn-stop") as HTMLButtonElement;
  if (btnStop) btnStop.disabled = s?.state === "idle";
  const btnPrev = document.getElementById("btn-prev") as HTMLButtonElement;
  const btnNext = document.getElementById("btn-next") as HTMLButtonElement;
  if (btnPrev) btnPrev.disabled = app.queueTracks.length === 0;
  if (btnNext) btnNext.disabled = app.queueTracks.length === 0;

  if (s?.fault && s.state !== lastState) {
    setToast(`⚠ ${s.fault} — use Recover in Settings.`);
  }
  lastState = s?.state ?? null;
}

// Track-change reset for the seek bar.
let seenTrack: string | null = null;
function trackChangeReset(s: PlaybackStatus) {
  const key = s.state + (app.queueCurrent ?? "-");
  if (s.state === "playing" && seenTrack !== key) {
    seenTrack = key;
    app.durationS = s.duration_s ?? 0;
    seekBar.value = "0";
    const cur = document.getElementById("seek-cur");
    if (cur) cur.textContent = "0:00";
  } else if (s.duration_s && s.duration_s > 0) {
    app.durationS = s.duration_s;
  }
}

// Live status poll.
async function pollStatus() {
  try {
    await loadStatus();
    if (app.status) trackChangeReset(app.status);
    renderBar();
    refreshActive();
  } catch {
    /* backend busy */
  }
}

// Position poll: seek bar + auto-advance.
async function pollPosition() {
  try {
    const pos = await invoke<number>("playback_position");
    if (!seeking && app.durationS > 0) {
      seekBar.disabled = false;
      seekBar.value = String(Math.min(100, (pos / app.durationS) * 100));
    }
    const cur = document.getElementById("seek-cur");
    if (cur) cur.textContent = fmtTime(pos);
    const dur = document.getElementById("seek-dur");
    if (dur && app.durationS > 0) dur.textContent = fmtTime(app.durationS);
    if (app.durationS > 0 && pos >= app.durationS - 0.3) {
      const q = await nextTrack();
      if (!q && app.queueTracks.length === 1) {
        await invoke("play_queue_index", { index: 0 });
      } else if (!q) {
        setToast("End of queue");
      }
    }
  } catch {
    /* not playing */
  }
}

setInterval(pollStatus, 500);
setInterval(pollPosition, 400);

// Player page animation: waveform progress + spectrum (only while the
// player route is active).
setInterval(() => {
  if (activeRoute() === "player") animatePlayer();
}, 120);

// State changes re-render the active page.
subscribe(() => refreshActive());

// Media keys (keyboard/headphones) -> transport.
void onKernelEvent("media-next", () => {
  void nextTrack().then((q) => {
    if (!q && app.queueTracks.length === 1) void invoke("play_queue_index", { index: 0 });
  });
});
void onKernelEvent("media-prev", () => {
  void prevTrack();
});

// Library watcher (kernel event): a rescan added/removed tracks —
// refresh the rail count and re-render whatever page is showing.
void onKernelEvent("library-changed", (payload) => {
  const { added, pruned } = payload as { added: number; pruned: number };
  setToast(
    added > 0
      ? `Library: ${added} new track${added === 1 ? "" : "s"}`
      : pruned > 0
        ? `Library: ${pruned} removed`
        : "Library updated",
  );
  refreshActive();
  void updateRailCount();
});

// External file drops (Explorer -> window).
void registerDrop();

// Keyboard: Space play/pause, arrows seek, Ctrl+K quick search.
document.addEventListener("keydown", (e) => {
  const inField = (e.target as HTMLElement)?.matches?.("input, select, textarea");
  if (e.code === "Space" && !inField) {
    e.preventDefault();
    playFromBar();
  } else if (e.key === "ArrowRight" && !inField) {
    void invoke("seek", { positionS: Math.min(app.durationS, currentPos() + 5) });
  } else if (e.key === "ArrowLeft" && !inField) {
    void invoke("seek", { positionS: Math.max(0, currentPos() - 5) });
  } else if (e.ctrlKey && e.key.toLowerCase() === "k") {
    e.preventDefault();
    navigate("library");
    setTimeout(() => document.querySelector<HTMLInputElement>("#lib-search")?.focus(), 60);
  }
});

function currentPos(): number {
  const bar = document.getElementById("seek") as HTMLInputElement | null;
  if (bar && app.durationS > 0) {
    return (Number.parseFloat(bar.value) / 100) * app.durationS;
  }
  return 0;
}

// ---------------------------------------------------------------------------
// Boot
// ---------------------------------------------------------------------------

void loadDsp();
void pollStatus();

// Keep the library count in the rail fresh.
async function updateRailCount(): Promise<void> {
  try {
    const st = await invoke<{ open: boolean; track_count: number }>("library_status");
    const el = document.getElementById("rail-count");
    if (el) el.textContent = st.open ? `${st.track_count} tracks` : "";
  } catch {
    /* not ready */
  }
}
setInterval(() => void updateRailCount(), 5000);
}
