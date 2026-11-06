/** Aurora Web shell — YTM 1:1 layout (dark, ru-RU).
 *
 * Anatomy mirrors music.youtube.com exactly: persistent left guide
 * (240px, collapsible to 72px icon rail), 64px topbar with centered
 * search pill, routed content, 72px full-width bottom player bar with
 * a thin 4px progress strip on its top edge, 380px queue side panel.
 * The one deliberate deviation: Braun-orange accent instead of YTM red
 * (owner's veto on a literal Google look — one-file swap if reverted).
 */

import { platform, invoke } from "./invoke";
import {
  app,
  loadStatus,
  nextTrack,
  notify,
  playQueueIndex,
  prevTrack,
  setQueue,
  setRepeat,
  setShuffle,
  setToast,
  subscribe,
  type RepeatMode,
} from "./state";
import { getLike, setLike, type LikeValue } from "./likes-service";
import { unlockAudio, flushOutput } from "./audio-bridge";
import { icon, type IconName } from "./styles/ytm-icons";
import "./styles/ytm.css";
import { homePage } from "./pages/home";
import { listPlaylists, onPlaylistsChanged } from "./playlist-service";

export function isWebShell(): boolean {
  return platform === "web";
}

interface NavEntry {
  route: string;
  label: string;
  icon: IconName;
}

const NAV: NavEntry[] = [
  { route: "home", label: "Главная", icon: "home" },
  { route: "search", label: "Поиск", icon: "search" },
  { route: "history", label: "Библиотека", icon: "library" },
];

const LIBRARY_NAV: NavEntry[] = [
  { route: "playlists", label: "Плейлисты", icon: "queue" },
  { route: "eq", label: "Эквалайзер", icon: "eq" },
  { route: "effects", label: "Effects & Tools", icon: "eq" },
  { route: "settings", label: "Настройки", icon: "settings" },
];

// Decks are NATIVE-ONLY for now: the wasm bridge drains only the main
// engine's ring — deck engines have no consumer on the web, so
// deck_play would report "playing" with permanent silence (a dishonest
// status per ADR 0002). Hidden until the ring mixer lands.
const MOBILE_TABS: NavEntry[] = [
  { route: "home", label: "Главная", icon: "home" },
  { route: "search", label: "Поиск", icon: "search" },
  { route: "queue", label: "Очередь", icon: "queue" },
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
        <div class="ytm-guide-nav">
          <div class="ytm-guide-head">
            <button class="ytm-btn-icon" id="ytm-guide-toggle" type="button" title="Скрыть меню">${icon("more")}</button>
            <div class="ytm-logo">
              <svg viewBox="0 0 24 24" width="24" height="24" aria-hidden="true">
                <!-- Aurora mark: an off-center pulse (waveform seed), Braun-orange -->
                <circle cx="12" cy="12" r="11" fill="#e8500a" opacity="0.92"/>
                <path d="M4 12h2.2l1.4-4.4L10.5 17l2.2-9 1.6 5.6 1.1-1.6H20" fill="none" stroke="#030303" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"/>
              </svg>
              <span>Aurora</span>
            </div>
          </div>
        </div>
        <div class="ytm-guide-sections" id="ytm-guide-sections">
          <div class="ytm-guide-section" id="ytm-guide-main"></div>
          <div class="ytm-guide-divider"></div>
          <div class="ytm-guide-section" id="ytm-guide-playlists-wrap">
            <button class="ytm-guide-new" id="ytm-guide-new" type="button">${icon("plus")}<span>Новый</span></button>
          </div>
          <div class="ytm-guide-playlists" id="ytm-guide-playlists"></div>
          <div class="ytm-guide-divider"></div>
          <div class="ytm-guide-section" id="ytm-guide-library"></div>
        </div>
      </nav>

      <header class="ytm-topbar" id="ytm-topbar">
        <div class="ytm-topbar-left">
          <button class="ytm-btn-icon" id="ytm-guide-toggle-2" type="button" title="Меню" hidden>${icon("more")}</button>
        </div>
        <div class="ytm-topbar-center">
          <div class="ytm-search" id="ytm-topbar-search">
            ${icon("search", 20)}
            <input id="ytm-search-input" type="search" placeholder="Поиск треков, альбомов, исполнителей, подкастов" autocomplete="off" />
            <button class="ytm-btn-icon" id="ytm-search-clear" type="button" title="Очистить" hidden>${icon("close", 18)}</button>
          </div>
        </div>
        <div class="ytm-topbar-right">
          <button id="ytm-avatar" type="button" title="Меню">A</button>
        </div>
      </header>

      <main class="ytm-content" id="ytm-content"></main>

      <div class="ytm-miniplayer" id="ytm-mini">
        <div class="ytm-player-art" id="ytm-mini-art"></div>
        <div class="ytm-player-meta">
          <div class="ytm-player-title" id="ytm-mini-title">Aurora Web</div>
          <div class="ytm-player-artist" id="ytm-mini-artist">Перетащите музыку, чтобы начать</div>
        </div>
        <button class="ytm-btn-icon ytm-btn-play" id="ytm-mini-play" type="button">${icon("play")}</button>
      </div>

      <nav class="ytm-tabs" id="ytm-tabs">
        ${MOBILE_TABS.map(
          (t) =>
            `<button class="ytm-tab" data-route="${t.route}" type="button">${icon(t.icon)}<span>${t.label}</span></button>`,
        ).join("\n        ")}
      </nav>

      <aside class="ytm-drawer" id="ytm-drawer" aria-label="Очередь">
        <header class="ytm-drawer-head">
          <span class="ytm-drawer-title">Очередь</span>
          <span class="ytm-drawer-head-actions">
            <button id="ytm-drawer-clear" type="button" title="Очистить очередь">Очистить</button>
            <button class="ytm-btn-icon" id="ytm-drawer-close" type="button" title="Закрыть">${icon("close", 20)}</button>
          </span>
        </header>
        <div class="ytm-drawer-body" id="ytm-drawer-body"></div>
      </aside>

      <footer class="ytm-player">
        <div class="ytm-player-progress" id="ytm-player-progress">
          <span class="ytm-time" id="ytm-cur">0:00</span>
          <input id="ytm-seek" type="range" min="0" max="1000" step="1" value="0" />
          <span class="ytm-time" id="ytm-dur">0:00</span>
        </div>
        <div class="ytm-player-body">
          <div class="ytm-player-now" id="ytm-now">
            <div class="ytm-player-art" id="ytm-art"></div>
            <div class="ytm-player-meta">
              <div class="ytm-player-title" id="ytm-title">Aurora Web</div>
              <div class="ytm-player-artist" id="ytm-artist">Перетащите музыку, чтобы начать</div>
            </div>
            <div class="ytm-player-like">
              <button class="ytm-btn-icon" id="ytm-like" type="button" title="Нравится">${icon("like")}</button>
              <button class="ytm-btn-icon" id="ytm-dislike" type="button" title="Не нравится">${icon("dislike")}</button>
            </div>
          </div>
          <div class="ytm-player-center">
            <div class="ytm-transport">
              <button class="ytm-btn-icon" id="ytm-prev" type="button" title="Назад">${icon("prev")}</button>
              <button class="ytm-btn-icon ytm-btn-play" id="ytm-play" type="button" title="Приостановить">${icon("play")}</button>
              <button class="ytm-btn-icon" id="ytm-next" type="button" title="Далее">${icon("next")}</button>
            </div>
          </div>
          <div class="ytm-player-right">
            <span class="ytm-quality" id="ytm-quality" hidden></span>
            <span class="ytm-player-artist" id="ytm-state">IDLE</span>
            <button class="ytm-btn-icon" id="ytm-drawer-toggle" type="button" title="Очередь">${icon("queue")}</button>
            <button class="ytm-btn-icon" id="ytm-stop" type="button" title="Стоп">${icon("close")}</button>
            <div class="ytm-vol" id="ytm-vol">
              <input id="ytm-vol-range" type="range" min="0" max="100" step="1" value="100" aria-label="Громкость" />
              <button class="ytm-btn-icon" id="ytm-vol-btn" type="button" title="Громкость">${icon("volume")}</button>
            </div>
            <button class="ytm-btn-icon" id="ytm-shuffle" type="button" title="Перемешать">${icon("shuffle")}</button>
            <button class="ytm-btn-icon" id="ytm-repeat" type="button" title="Повтор отключен">${icon("repeat")}</button>
          </div>
        </div>
      </footer>
    </div>
    <p class="toast" id="toast" role="status"></p>
  `;

  // Static guide sections (nav rows).
  const mainSec = document.getElementById("ytm-guide-main");
  const libSec = document.getElementById("ytm-guide-library");
  if (mainSec) {
    for (const n of NAV) mainSec.appendChild(navRow(n));
  }
  if (libSec) {
    for (const n of LIBRARY_NAV) libSec.appendChild(navRow(n));
  }

  // Guide collapse (YTM hamburger): persisted.
  const guideToggle = document.getElementById("ytm-guide-toggle");
  guideToggle?.addEventListener("click", () => {
    const el = document.querySelector<HTMLElement>(".ytm-app");
    if (!el) return;
    const collapsed = el.classList.toggle("guide-collapsed");
    const alt = document.getElementById("ytm-guide-toggle-2");
    if (alt) alt.hidden = !collapsed;
    try {
      localStorage.setItem("aw:guide-collapsed", collapsed ? "1" : "0");
    } catch {
      /* private mode */
    }
  });
  document.getElementById("ytm-guide-toggle-2")?.addEventListener("click", () => {
    document.getElementById("ytm-guide-toggle")?.click();
  });
  let startCollapsed = false;
  try {
    startCollapsed = localStorage.getItem("aw:guide-collapsed") === "1";
  } catch {
    /* ignore */
  }
  if (startCollapsed) {
    document.querySelector<HTMLElement>(".ytm-app")?.classList.add("guide-collapsed");
    const alt = document.getElementById("ytm-guide-toggle-2");
    if (alt) alt.hidden = false;
  }

  // Tablet (spec 5.3): the collapsed-rail layout applies whenever the
  // viewport is narrow, regardless of the hamburger state (the CSS
  // shares the guide-collapsed rules through a combined selector).
  if (matchMedia("(max-width:1024px)").matches) {
    document.querySelector<HTMLElement>(".ytm-app")?.classList.add("tablet-narrow");
  }

  // Live playlists in the guide (YTM: user playlists under "Новый").
  void renderGuidePlaylists();
  onPlaylistsChanged(() => void renderGuidePlaylists());
  document.getElementById("ytm-guide-new")?.addEventListener("click", () => {
    // Same flow as the playlists page's "Новый": prompt + create, then
    // navigate to the fresh playlist.
    const name = window.prompt("Название плейлиста");
    if (name === null) return;
    void (async () => {
      const { createPlaylist } = await import("./playlist-service");
      const pl = await createPlaylist(name);
      setToast(`Создан «${pl.name}»`);
      window.location.hash = `#/playlist?id=${encodeURIComponent(pl.id)}`;
    })();
  });

  // Avatar (spec 5.4): kv profile-name initial, shared menu pattern.
  const avatar = document.getElementById("ytm-avatar");
  if (avatar) {
    void (async () => {
      try {
        const { kvGet } = await import("./library-db");
        const name = await kvGet<string>("aw:profile-name");
        if (name) avatar.textContent = name.trim().slice(0, 1).toUpperCase();
      } catch {
        /* default "A" stands */
      }
    })();
    avatar.addEventListener("click", () => showAvatarMenu(avatar));
  }

  // Queue drawer: Clear (whole queue) — bound here, the rows' ✕ live in
  // renderDrawer().
  document.getElementById("ytm-drawer-clear")?.addEventListener("click", () => {
    void (async () => {
      app.queueCurrent = null;
      await setQueue([]);
      notify();
    })();
  });

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
  // Enter: full search page with the query — live-forwarding stays as
  // the type-ahead behavior on Home.
  topSearch?.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && topSearch.value.trim()) {
      window.location.hash = `#/search?q=${encodeURIComponent(topSearch.value)}`;
    }
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

function navRow(n: NavEntry): HTMLButtonElement {
  const b = document.createElement("button");
  b.type = "button";
  b.className = "ytm-nav-item";
  b.dataset.route = n.route;
  b.innerHTML = icon(n.icon) + `<span>${n.label}</span>`;
  return b;
}

/** Guide playlists: live rows from playlist-service (YTM's guide list). */
async function renderGuidePlaylists(): Promise<void> {
  const box = document.getElementById("ytm-guide-playlists");
  if (!box) return;
  box.innerHTML = "";
  try {
    const lists = await listPlaylists();
    for (const p of lists) {
      const row = document.createElement("button");
      row.type = "button";
      row.className = "ytm-nav-item ytm-guide-playlist";
      row.dataset.playlist = p.id;
      const ic = document.createElement("span");
      ic.className = "ytm-guide-playlist-icon";
      ic.innerHTML = icon("queue");
      const label = document.createElement("span");
      label.textContent = p.name;
      row.append(ic, label);
      row.addEventListener("click", () => {
        window.location.hash = `#/playlist?id=${encodeURIComponent(p.id)}`;
      });
      box.appendChild(row);
    }
  } catch {
    /* playlists store unavailable — the section just stays empty */
  }
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
      setToast(`Не удалось воспроизвести: ${String(e)}`);
    }
  };

  play?.addEventListener("click", toggle);
  miniPlay?.addEventListener("click", (e) => {
    e.stopPropagation();
    void toggle();
  });
  next?.addEventListener("click", () => void nextTrack());
  prev?.addEventListener("click", () => void prevTrack());

  // Repeat: cycles off -> all -> one -> off (spec 3.1).
  document.getElementById("ytm-repeat")?.addEventListener("click", () => {
    const order: RepeatMode[] = ["off", "all", "one"];
    const nextMode = order[(order.indexOf(app.repeat) + 1) % order.length];
    void setRepeat(nextMode).then(() => renderTransportModes());
  });

  // Shuffle: toggles + reshuffles the queue tail (the actual queue
  // reorder lives in main.ts' setShuffle handler — state only persists).
  document.getElementById("ytm-shuffle")?.addEventListener("click", () => {
    void setShuffle(!app.shuffle).then(() => {
      renderTransportModes();
      void applyShuffle();
    });
  });

  /** Shuffle ON: current track first, Fisher-Yates the tail (mirrors
   *  the desktop shuffle handler). Shuffle OFF: no reorder (YTM keeps
   *  the shuffled order — an unshuffle is a lie about "original"). */
  async function applyShuffle(): Promise<void> {
    if (!app.shuffle || app.queueTracks.length < 2) return;
    const cur = app.queueCurrent ?? 0;
    const head = app.queueTracks[cur];
    if (!head) return;
    const rest = app.queueTracks.filter((_, i) => i !== cur);
    for (let i = rest.length - 1; i > 0; i--) {
      const j = Math.floor(Math.random() * (i + 1));
      [rest[i], rest[j]] = [rest[j], rest[i]];
    }
    app.queueTracks = [head, ...rest];
    app.queueCurrent = 0;
    await setQueue(app.queueTracks);
    notify();
    renderTransportModes();
  }

  // Likes (spec 3.2): thumbs toggle — clicking the active rating clears it.
  const likeBtn = document.getElementById("ytm-like");
  const dislikeBtn = document.getElementById("ytm-dislike");
  const currentTrackId = (): string | null => {
    const cur = app.queueTracks[app.queueCurrent ?? 0];
    return cur?.path.startsWith("web://") ? cur.path.slice(6) : null;
  };
  likeBtn?.addEventListener("click", async () => {
    const id = currentTrackId();
    if (!id) return;
    const cur = await getLike(id);
    await setLike(id, cur === "like" ? null : "like");
    renderLikes();
  });
  dislikeBtn?.addEventListener("click", async () => {
    const id = currentTrackId();
    if (!id) return;
    const cur = await getLike(id);
    await setLike(id, cur === "dislike" ? null : "dislike");
    renderLikes();
  });

  /** Active state for like/dislike on the current track. */
  async function renderLikes(): Promise<void> {
    const id = currentTrackId();
    const val: LikeValue | null = id ? await getLike(id) : null;
    likeBtn?.classList.toggle("active", val === "like");
    dislikeBtn?.classList.toggle("active", val === "dislike");
    likeBtn?.toggleAttribute("disabled", !id);
    dislikeBtn?.toggleAttribute("disabled", !id);
  }
  void renderLikes();
  subscribe(() => void renderLikes());
  // Persisted-mode restore (loadTransportModes notifies) updates the
  // transport immediately instead of waiting for the status poll.
  subscribe(renderTransportModes);
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
      if (app.dsp) {
        app.dsp.volume_db = db;
        await invoke("set_dsp", { config: structuredClone(app.dsp) });
        const { kvPut } = await import("./library-db");
        await kvPut("dsp", app.dsp);
      }
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
  if (miniArtist) miniArtist.textContent = a || "Перетащите музыку, чтобы начать";
  if (stateEl) stateEl.textContent = s?.state?.toUpperCase() ?? "IDLE";

  // Play/pause icon.
  const playing = s?.state === "playing";
  for (const id of ["ytm-play", "ytm-mini-play"]) {
    const btn = document.getElementById(id);
    if (btn) btn.innerHTML = playing ? icon("pause") : icon("play");
  }
  const playBtn = document.getElementById("ytm-play");
  if (playBtn) playBtn.title = playing ? "Приостановить" : "Воспроизвести";

  // Seek bar: enabled whenever a duration is known.
  const seek = document.getElementById("ytm-seek") as HTMLInputElement | null;
  if (seek) seek.disabled = !(app.durationS > 0);

  renderTransportModes();
  renderDrawer();
}

/** Repeat + shuffle button state (active class, titles, "1" badge). */
export function renderTransportModes(): void {
  const repeatBtn = document.getElementById("ytm-repeat");
  if (repeatBtn) {
    repeatBtn.classList.toggle("active", app.repeat !== "off");
    repeatBtn.title =
      app.repeat === "all" ? "Повторять все треки" : app.repeat === "one" ? "Повторять один трек" : "Повтор отключен";
    // "1" indicator (repeat-one): a tnum numeral badge inside the button.
    let badge = repeatBtn.querySelector<HTMLElement>(".ytm-repeat-one");
    if (app.repeat === "one") {
      if (!badge) {
        badge = document.createElement("span");
        badge.className = "ytm-repeat-one";
        badge.textContent = "1";
        repeatBtn.appendChild(badge);
      }
    } else {
      badge?.remove();
    }
  }
  const shuffleBtn = document.getElementById("ytm-shuffle");
  if (shuffleBtn) {
    shuffleBtn.classList.toggle("active", app.shuffle);
    shuffleBtn.title = app.shuffle ? "Перемешать: вкл" : "Перемешать";
  }
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

/** Queue drawer body: current + up next (click to jump, ✕ to remove). */
export function renderDrawer(): void {
  const body = document.getElementById("ytm-drawer-body");
  if (!body) return;
  body.innerHTML = "";
  const head = document.createElement("p");
  head.className = "ytm-drawer-section";
  head.textContent = app.queueCurrent != null ? "Сейчас играет" : "Очередь";
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
    // ✕ remove (spec 5.1): splice + index fix + push to the kernel.
    // The row button captures clicks, so the ✕ stops propagation.
    const remove = document.createElement("span");
    remove.className = "ytm-drawer-row-remove";
    remove.title = "Удалить из очереди";
    remove.textContent = "✕";
    remove.addEventListener("click", (e) => {
      e.stopPropagation();
      removeQueueRow(idx);
    });
    row.append(title, sub, remove);
    row.addEventListener("click", () => void playQueueIndex(idx));
    body.appendChild(row);
  });
  if (!app.queueTracks.length) {
    const empty = document.createElement("p");
    empty.className = "ytm-drawer-empty";
    empty.textContent = "Очередь пуста";
    body.appendChild(empty);
  }
}

/** Remove one queue row: splice, keep `current` pointing at the SAME
 *  track (shift down when the removed row sat before it, clamp when the
 *  current track itself was removed), push the list to the kernel. The
 *  kernel's set_queue preserves a `current` that still lands inside the
 *  new list, so mid-playback removals never break auto-advance. */
function removeQueueRow(idx: number): void {
  if (idx < 0 || idx >= app.queueTracks.length) return;
  void (async () => {
    const next = [...app.queueTracks];
    next.splice(idx, 1);
    const wasCurrent = idx === app.queueCurrent;
    const wasBefore = idx < (app.queueCurrent ?? -1);
    if (app.queueCurrent != null) {
      if (wasCurrent) {
        app.queueCurrent = next.length ? Math.min(idx, next.length - 1) : null;
      } else if (wasBefore) {
        app.queueCurrent -= 1;
      }
    }
    await setQueue(next);
    notify();
    if (wasCurrent && app.status?.state === "playing") {
      // The playing track was removed: hand the queue position to the
      // track that took its place (leaving `current` pointing at a
      // different track than the one sounding would lie in the bar).
      // Only while actually playing — a paused/idle queue removal must
      // never START playback.
      if (next.length && app.queueCurrent != null) {
        await playQueueIndex(app.queueCurrent);
      } else {
        try {
          await invoke("stop_playback");
        } catch {
          /* already idle */
        }
        app.queueCurrent = null;
        notify();
      }
    }
  })();
}

/** Avatar menu (spec 5.4): the shared .ytm-menu pattern — Настройки /
 *  Эквалайзер / История navigation plus an async Storage line from
 *  navigator.storage.estimate(). No sign-out or account switch: there
 *  is nothing to switch to (ADR 0002 honesty). */
function showAvatarMenu(anchor: HTMLElement): void {
  document.querySelector(".ytm-menu")?.remove();
  const menu = document.createElement("div");
  menu.className = "ytm-menu";

  const item = (label: string, onClick: () => void): HTMLButtonElement => {
    const b = document.createElement("button");
    b.type = "button";
    b.textContent = label;
    b.addEventListener("click", () => {
      onClick();
      menu.remove();
    });
    return b;
  };
  menu.appendChild(item("Настройки", () => (window.location.hash = "#/settings")));
  menu.appendChild(item("Эквалайзер", () => (window.location.hash = "#/eq")));
  menu.appendChild(item("История", () => (window.location.hash = "#/history")));

  // Storage: the browser's own estimate, rendered when it answers.
  const storage = document.createElement("p");
  storage.className = "ytm-menu-head";
  storage.textContent = "Хранилище: …";
  menu.appendChild(storage);
  if (navigator.storage?.estimate) {
    navigator.storage
      .estimate()
      .then((est) => {
        const mb = Math.round((est.usage ?? 0) / 1048576);
        storage.textContent = `Хранилище: ${mb} МБ`;
      })
      .catch(() => {
        storage.textContent = "Хранилище: недоступно";
      });
  } else {
    storage.textContent = "Хранилище: недоступно";
  }

  document.body.appendChild(menu);
  const rect = anchor.getBoundingClientRect();
  menu.style.right = `${Math.max(8, window.innerWidth - rect.right)}px`;
  menu.style.top = `${Math.min(rect.bottom + 6, window.innerHeight - 240)}px`;
  menu.style.left = "auto";

  const close = (e: Event) => {
    if (!menu.contains(e.target as Node)) {
      menu.remove();
      document.removeEventListener("click", close, true);
    }
  };
  // The avatar's own click is still propagating — defer arming by a tick
  // so the menu does not close immediately.
  setTimeout(() => document.addEventListener("click", close, true), 0);
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
