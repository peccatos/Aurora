/** YTM Home page: shelves of cards (your music, quick mix).
 *  Web-shell-only route; the desktop shell keeps its own player page. */

import { invoke, platform, registerWebFile, registerWebBytes, analyzeLufs } from "../invoke";
import { app, playQueueIndex, setQueue, setToast } from "../state";
import { unlockAudio } from "../audio-bridge";
import { searchTracks, addPreviewToQueue, type SearchHit } from "../search-service";
import { icon } from "../styles/ytm-icons";
import type { Page } from "../router";
import {
  getAllTracks,
  putTrack,
  deleteTrack,
  kvGet,
  kvPut,
  putArtwork,
  type StoredTrack,
} from "../library-db";
import { trackArtworkUrl } from "../artwork-store";
import { readTrackTags } from "../tag-reader";
import { recordPlay, recentPlays } from "../history-service";
import { listPlaylists, onPlaylistsChanged } from "../playlist-service";
import { makeCarousel } from "../carousel";

/** Fetch one stored track by id (byte-loading helper). */
async function getStoredTrack(id: string): Promise<StoredTrack | null> {
  const all = await getAllTracks();
  return all.find((t) => t.id === id) ?? null;
}

interface WebTrack {
  id: string;
  name: string;
  artist: string | null;
  album: string | null;
  durationMs: number | null;
  /** In-memory file handle (page lifetime) — null for tracks restored
   *  from IndexedDB (their bytes live in the store). */
  file: File | null;
  /** File extension (no dot) — needed when bytes come from the store. */
  extension: string;
  /** R128 integrated loudness (LUFS) measured at ingest (W8): feeds the
   *  kernel's ReplayGain stage for track-normalized loudness. */
  lufs: number | null;
  /** The web:// path registered with the kernel (after first play). */
  kernelPath: string | null;
}

const webTracks = new Map<string, WebTrack>();

export function registerWebTrack(
  t: Omit<WebTrack, "kernelPath" | "extension" | "lufs"> & { extension?: string; lufs?: number | null },
) {
  webTracks.set(t.id, {
    ...t,
    lufs: t.lufs ?? null,
    extension: t.extension ?? (t.file ? (t.file.name.split(".").pop() ?? "").toLowerCase() : ""),
    kernelPath: null,
  });
}

export function webTrackCount(): number {
  return webTracks.size;
}

/** Snapshot of the session's web tracks (search page reads the library
 *  without touching the Map itself). */
export function getWebTracks(): WebTrack[] {
  return [...webTracks.values()];
}

/** Lazy byte loading for restored tracks: only fetched from IndexedDB
 *  when the track is actually played (a big library costs nothing
 *  until used). */
const storedBytesCache = new Map<string, ArrayBuffer | null>();

async function loadStoredBytes(id: string): Promise<ArrayBuffer | null> {
  if (storedBytesCache.has(id)) return storedBytesCache.get(id) ?? null;
  try {
    const t = await getStoredTrack(id);
    storedBytesCache.set(id, t?.bytes ?? null);
    return t?.bytes ?? null;
  } catch {
    storedBytesCache.set(id, null);
    return null;
  }
}

/** Spec 4.1/4.2: active Home filter chip (survives re-renders — the
 *  page re-mounts on every state push, so local state must live at
 *  module level, not inside render()). */
let activeChip: "all" | "songs" | "playlists" | "recent" = "all";

/** Which shelves each chip keeps visible. "all" = current behavior. */
function chipKeeps(chip: typeof activeChip, shelf: "history" | "playlists" | "tracks"): boolean {
  if (chip === "all") return true;
  if (chip === "recent") return shelf === "history";
  if (chip === "playlists") return shelf === "playlists";
  return shelf === "tracks"; // songs
}

/** The search filter shared by draw() and update() — update() must
 *  predict how many rows the CURRENT query yields, or it re-renders
 *  the whole page on every 500ms status poll while a filter is active. */
function filterTracks(query: string): WebTrack[] {
  return [...webTracks.values()].filter(
    (t) =>
      !query ||
      t.name.toLowerCase().includes(query) ||
      (t.artist ?? "").toLowerCase().includes(query),
  );
}

/** Render the "Your playlists" shelf row contents (spec 4.1): playlist
 *  cards — name + track count, click navigates to the playlist page.
 *  Called by draw() on mount and by the playlistsChanged listener on
 *  any mutation from any route (create via ⋯ menu, rename, reorder…). */
async function renderPlaylistsShelf(row: HTMLElement): Promise<void> {
  const lists = await listPlaylists();
  row.innerHTML = "";
  for (const pl of lists) {
    // DOM API: playlist names are user input (XSS law).
    const card = document.createElement("div");
    card.className = "ytm-card";
    card.title = pl.name;
    const art = document.createElement("div");
    art.className = "ytm-card-art ytm-card-art--playlist";
    art.innerHTML = `${icon("queue", 48)}<button class="ytm-card-play" type="button" title="Play">${icon("play")}</button>`;
    const ct = document.createElement("div");
    ct.className = "ytm-card-title";
    ct.textContent = pl.name;
    const cs = document.createElement("div");
    cs.className = "ytm-card-sub";
    cs.textContent = `${pl.trackIds.length} track${pl.trackIds.length === 1 ? "" : "s"}`;
    card.append(art, ct, cs);
    card.addEventListener("click", () => {
      // The playlist page reads the id from the hash query at mount.
      window.location.hash = `#/playlist?id=${encodeURIComponent(pl.id)}`;
    });
    card.querySelector<HTMLButtonElement>(".ytm-card-play")!.addEventListener("click", (e) => {
      e.stopPropagation();
      void playWebTracksByIds(pl.id, 0);
    });
    row.appendChild(card);
  }
  makeCarousel(row);
}

/** Spec 4.1: keep the shelf DOM alive across playlist mutations so the
 *  listener re-renders ONLY the row (not the whole page). Registered
 *  ONCE at module level (module state survives re-renders); the
 *  homepage module never unmounts in this shell's lifetime. */
let shelfRowEl: HTMLElement | null = null;
onPlaylistsChanged(() => {
  const row = shelfRowEl;
  if (row?.isConnected) void renderPlaylistsShelf(row);
});

/** Render the "Continue listening" shelf into the given root
 *  (draw() and update() share this — one card renderer, no drift). */
async function renderHistory(root: HTMLElement): Promise<void> {
  const entries = await recentPlays(8);
  const container = root.querySelector("#ytm-history");
  if (!container) return; // re-render raced us
  container.innerHTML = "";
  for (const e of entries) {
    const track = webTracks.get(e.id);
    const card = document.createElement("div");
    card.className = "ytm-card";
    const art = document.createElement("div");
    art.className = "ytm-card-art";
    art.innerHTML = `${icon("library", 48)}<button class="ytm-card-play" type="button" title="Play">${icon("play")}</button>`;
    const ct = document.createElement("div");
    ct.className = "ytm-card-title";
    ct.textContent = e.title;
    const cs = document.createElement("div");
    cs.className = "ytm-card-sub";
    cs.textContent = e.artist ?? "Unknown artist";
    card.append(art, ct, cs);
    // Spec 4.3 "continue listening": resume-position bar under the
    // artwork. Only when the entry carries a position AND the track's
    // duration is known — no bar (rather than a fake 0%) otherwise.
    const durationMs = track?.durationMs ?? null;
    if (e.positionS != null && e.positionS > 0 && durationMs && durationMs > 0) {
      const pct = Math.min(100, (e.positionS * 1000) / durationMs) * 100;
      if (pct > 0 && pct < 100) {
        const bar = document.createElement("div");
        bar.className = "ytm-card-progress";
        const fill = document.createElement("i");
        fill.style.setProperty("--p", `${pct.toFixed(1)}%`);
        bar.appendChild(fill);
        card.appendChild(bar);
      }
    }
    card.addEventListener("click", () => {
      if (track) void playWebTracks([track], 0);
    });
    container.appendChild(card);
  }
  // Carousel after populate (arrows need real card widths).
  makeCarousel(container as HTMLElement);
}

/** Third-party URL trust gate: only absolute https URLs pass. A
 *  catalog response must never inject javascript: or data: URIs into
 *  style attributes. */
function safeHttpsUrl(u: string | null | undefined): u is string {
  if (!u) return false;
  try {
    const parsed = new URL(u);
    return parsed.protocol === "https:";
  } catch {
    return false;
  }
}

function fmtDur(ms: number | null): string {
  if (!ms) return "";
  const s = Math.round(ms / 1000);
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

/** Play a set of web tracks through the kernel queue. Files register by
 *  File handle; IndexedDB-restored tracks register their stored bytes
 *  under their persistent id (queue paths survive reloads). */
async function playWebTracks(tracks: WebTrack[], index = 0): Promise<void> {
  if (!tracks.length) return;
  unlockAudio(); // the click that got here is the user gesture
  // History first (YTM "continue listening" shelf): the track that
  // STARTS playing is the one logged, even if the user skips later.
  const start = tracks[index];
  if (start) {
    void recordPlay({ id: start.id, title: start.name, artist: start.artist });
  }
  await doPlayWebTracks(tracks, index);
}

/** ReplayGain target equivalent: the LUFS level normalized playback
 *  aims at (-14 LUFS is the streaming-platform convention). */
const NORMALIZE_TARGET_LUFS = -14;

/** The track context menu (YTM ⋯): add to playlist (submenu of
 *  existing playlists + create), play next. Anchored to the ⋯ button,
 *  clamped to the viewport, closes on outside click/Esc. */
function showTrackMenu(
  anchor: HTMLElement,
  trackId: string,
  trackName: string,
  all: WebTrack[],
  index: number,
): void {
  document.querySelector(".ytm-menu")?.remove();
  const menu = document.createElement("div");
  menu.className = "ytm-menu";

  const playNext = document.createElement("button");
  playNext.type = "button";
  playNext.textContent = "Play next";
  playNext.addEventListener("click", () => {
    // The current queue plus this track right after the playing one.
    void (async () => {
      const q = [...app.queueTracks];
      const t = all[index];
      if (!t) return;
      const at = (app.queueCurrent ?? -1) + 1;
      const path = t.kernelPath ?? (await registerWebBytes(t.id, new Uint8Array((await loadStoredBytes(t.id)) ?? []), t.extension));
      const rg = t.lufs != null ? NORMALIZE_TARGET_LUFS - t.lufs : null;
      q.splice(at, 0, { path, title: t.name, artist: t.artist, replaygain_track_db: rg, replaygain_album_db: null });
      await setQueue(q);
      setToast("Playing next");
    })();
    menu.remove();
  });
  menu.appendChild(playNext);

  const addHeader = document.createElement("p");
  addHeader.className = "ytm-menu-head";
  addHeader.textContent = "Add to playlist";
  menu.appendChild(addHeader);

  void (async () => {
    const { listPlaylists, addToPlaylist, createPlaylist } = await import("../playlist-service");
    const lists = await listPlaylists();
    for (const pl of lists) {
      const b = document.createElement("button");
      b.type = "button";
      b.textContent = pl.name;
      b.addEventListener("click", async () => {
        await addToPlaylist(pl.id, [trackId]);
        setToast(`Added to "${pl.name}"`);
        menu.remove();
      });
      menu.appendChild(b);
    }
    const create = document.createElement("button");
    create.type = "button";
    create.className = "ytm-menu-create";
    create.textContent = "＋ New playlist";
    create.addEventListener("click", async () => {
      const name = window.prompt(`Playlist name (for "${trackName}")`);
      if (name === null) return;
      const pl = await createPlaylist(name);
      await addToPlaylist(pl.id, [trackId]);
      setToast(`Added to "${pl.name}"`);
      menu.remove();
    });
    menu.appendChild(create);
  })();

  document.body.appendChild(menu);
  const rect = anchor.getBoundingClientRect();
  menu.style.left = `${Math.min(rect.left, window.innerWidth - 260)}px`;
  menu.style.top = `${Math.min(rect.bottom + 4, window.innerHeight - 300)}px`;

  const close = (e: Event) => {
    if (!menu.contains(e.target as Node)) {
      menu.remove();
      document.removeEventListener("click", close, true);
    }
  };
  document.addEventListener("click", close, true);
}

async function doPlayWebTracks(tracks: WebTrack[], index = 0): Promise<void> {
  const queue = [];
  for (const t of tracks) {
    t.kernelPath = t.file
      ? await registerWebFile(t.file)
      : await registerWebBytes(t.id, new Uint8Array((await loadStoredBytes(t.id)) ?? []), t.extension);
    // W8: measured LUFS feeds the ReplayGain track gain when the DSP
    // mode is on; the kernel treats it exactly like a file tag.
    const rg = t.lufs != null ? NORMALIZE_TARGET_LUFS - t.lufs : null;
    queue.push({
      path: t.kernelPath,
      title: t.name,
      artist: t.artist,
      replaygain_track_db: rg,
      replaygain_album_db: null as number | null,
      bpm: null as number | null,
    });
  }
  await setQueue(queue);
  await playQueueIndex(index);
}

/** Play a PLAYLIST by id: resolve stored tracks -> queue (lazy bytes,
 *  persistent ids, LUFS normalization — the same pipeline as Home). */
export async function playWebTracksByIds(playlistId: string, startIndex = 0): Promise<void> {
  const { getPlaylist } = await import("../playlist-service");
  const pl = await getPlaylist(playlistId);
  if (!pl || !pl.trackIds.length) return;
  const all = await getAllTracks();
  const tracks: WebTrack[] = [];
  for (const id of pl.trackIds) {
    const stored = all.find((t) => t.id === id);
    if (stored) {
      tracks.push({
        id: stored.id,
        name: stored.name,
        artist: stored.artist,
        album: stored.album,
        durationMs: stored.durationMs,
        file: null,
        extension: stored.extension,
        lufs: stored.lufs ?? null,
        kernelPath: null,
      });
    }
  }
  await playWebTracks(tracks, startIndex);
}

async function render(root: HTMLElement) {
  root.innerHTML = `
    <div class="ytm-search">
      ${icon("search", 22)}
      <input id="home-search-input" type="search" placeholder="Search" />
    </div>
    <div class="ytm-chips" id="home-chips"></div>
    <div id="ytm-shelves"></div>
    <p class="hint" id="ytm-empty"></p>
  `;

  const shelves = root.querySelector<HTMLElement>("#ytm-shelves")!;
  const empty = root.querySelector<HTMLElement>("#ytm-empty")!;
  const chips = root.querySelector<HTMLElement>("#home-chips")!;
  const search = root.querySelector<HTMLInputElement>("#home-search-input");

  function draw(query: string): void {
    // Chips row (spec 4.1): All / Songs / Playlists / Recent — the
    // active chip survives re-draws (module-level activeChip).
    const CHIP_DEFS: Array<{ id: typeof activeChip; label: string }> = [
      { id: "all", label: "All" },
      { id: "songs", label: "Songs" },
      { id: "playlists", label: "Playlists" },
      { id: "recent", label: "Recent" },
    ];
    chips.innerHTML = "";
    for (const def of CHIP_DEFS) {
      const b = document.createElement("button");
      b.type = "button";
      b.className = "ytm-chip" + (def.id === activeChip ? " active" : "");
      b.textContent = def.label;
      b.addEventListener("click", () => {
        if (activeChip === def.id) return;
        activeChip = def.id;
        draw(search?.value.toLowerCase() ?? "");
      });
      chips.appendChild(b);
    }

    const keepHistory = chipKeeps(activeChip, "history");
    const keepPlaylists = chipKeeps(activeChip, "playlists");
    const keepTracks = chipKeeps(activeChip, "tracks");

    const all = filterTracks(query);
    shelves.innerHTML = "";
    shelfRowEl = null;
    // Empty library or nothing matching on a songs-only view: the old
    // early-return (onboarding hint / "nothing matches"), unchanged.
    if (!all.length && (!keepPlaylists || !keepHistory)) {
      empty.textContent = webTracks.size
        ? "Nothing matches the search."
        : "No music yet — drop audio files anywhere on this page, or pick some below.";
      empty.insertAdjacentHTML(
        "beforeend",
        `<label class="ytm-chip" style="margin-top:12px;cursor:pointer">＋ Add music<input id="ytm-add-input" type="file" accept="audio/*" multiple hidden /></label>`,
      );
      const input = root.querySelector<HTMLInputElement>("#ytm-add-input");
      input?.addEventListener("change", () => void ingest(input.files));
      return;
    }
    if (!all.length) {
      // Filtered chip (Playlists/Recent) without songs: keep the shelf
      // visible, honest message instead of the "add music" drop hint.
      empty.textContent = webTracks.size ? "Nothing matches the search." : "";
    } else {
      empty.textContent = "";
    }

    // Shelf 0: "Continue listening" (local play history, graft from the
    // design judges). Rendered async — the shelf appears when history
    // resolves; empty history leaves nothing (honest).
    if (keepHistory) {
      void renderHistory(shelves);
      shelves.insertAdjacentHTML(
        "afterbegin",
        `<div class="ytm-shelf">
           <div class="ytm-shelf-head"><span class="ytm-shelf-title">Continue listening</span></div>
           <div class="ytm-shelf-row" id="ytm-history"></div>
         </div>`,
      );
    }

    // Shelf 0.5: "Your playlists" (spec 4.1) — between history and
    // "Your music". Async render; the row element is kept at module
    // level so the onPlaylistsChanged listener can refresh ONLY this
    // shelf when playlists mutate from any route.
    if (keepPlaylists) {
      shelves.insertAdjacentHTML(
        "beforeend",
        `<div class="ytm-shelf" id="ytm-playlists-shelf">
           <div class="ytm-shelf-head"><span class="ytm-shelf-title">Your playlists</span></div>
           <div class="ytm-shelf-row" id="ytm-playlists"></div>
         </div>`,
      );
      shelfRowEl = shelves.querySelector<HTMLElement>("#ytm-playlists");
      if (shelfRowEl) void renderPlaylistsShelf(shelfRowEl);
    }

    if (keepTracks && all.length) {
      drawTracklist(all);
    }
    if (keepTracks && all.length && activeChip === "all") {
      drawQuickMix(all);
    }
  }

  function drawTracklist(all: WebTrack[]): void {
    // Shelf 1: all tracks as YTM song rows. Header (spec 4.6): track
    // count + shuffle-all button (btn small, shuffle icon).
    shelves.insertAdjacentHTML(
      "beforeend",
      `<div class="ytm-shelf">
         <div class="ytm-shelf-head"><span class="ytm-shelf-title">Your music</span><span class="ytm-shelf-more" id="ytm-music-count" style="cursor:default;text-transform:none;letter-spacing:normal"></span></div>
         <div class="ytm-tracklist" id="ytm-list"></div>
       </div>`,
    );
    const headCount = shelves.querySelector<HTMLElement>("#ytm-music-count")!;
    headCount.textContent = `${all.length} track${all.length === 1 ? "" : "s"}`;
    const shuffleAll = document.createElement("button");
    shuffleAll.type = "button";
    // Inline styles: this task owns home.ts only — CSS files are other
    // agents' territory (ytm.css has no .btn small variant).
    shuffleAll.style.cssText =
      "display:inline-flex;align-items:center;gap:6px;margin-left:auto;padding:4px 10px;border:1px solid var(--ytm-divider);border-radius:999px;background:transparent;color:var(--ytm-text-2);font:inherit;font-size:13px;font-weight:500;cursor:pointer";
    shuffleAll.title = "Shuffle all tracks";
    shuffleAll.setAttribute("aria-label", "Shuffle all tracks");
    shuffleAll.innerHTML = `${icon("shuffle", 16)}<span>Shuffle</span>`;
    shuffleAll.addEventListener("click", () => {
      void playWebTracks([...webTracks.values()].sort(() => Math.random() - 0.5), 0);
    });
    headCount.after(shuffleAll);
    const list = shelves.querySelector<HTMLElement>("#ytm-list")!;
    all.forEach((t, i) => {
      // DOM API, not innerHTML: file names are user input — a name like
      // '"><img src=x onerror=…>' must never execute (XSS).
      const row = document.createElement("div");
      row.className = "ytm-track-row";
      const idx = document.createElement("span");
      idx.className = "ytm-track-index";
      idx.textContent = String(i + 1);
      const thumb = document.createElement("div");
      thumb.className = "ytm-track-thumb";
      thumb.innerHTML = icon("library", 18); // trusted constant
      // Embedded artwork (async): replace the placeholder icon only if
      // this row is still on screen when the URL resolves (a re-draw
      // may have replaced the list meanwhile).
      void trackArtworkUrl(t.id).then((url) => {
        if (!url || !thumb.isConnected) return;
        thumb.style.backgroundImage = `url("${url}")`;
        thumb.style.backgroundSize = "cover";
        thumb.style.backgroundPosition = "center";
        thumb.innerHTML = "";
      });
      const main = document.createElement("div");
      main.className = "ytm-track-main";
      const title = document.createElement("div");
      title.className = "ytm-track-title";
      title.textContent = t.name;
      const meta = document.createElement("div");
      meta.className = "ytm-track-meta";
      meta.textContent = t.artist ?? "Unknown artist";
      const dur = document.createElement("span");
      dur.className = "ytm-track-dur";
      dur.textContent = fmtDur(t.durationMs);
      main.append(title, meta);
      row.append(idx, thumb, main, dur);
      // Context menu (YTM ⋯): add to playlist / play next.
      const more = document.createElement("button");
      more.type = "button";
      more.className = "ytm-track-more";
      more.title = "More";
      more.innerHTML = icon("more", 18);
      more.addEventListener("click", (e) => {
        e.stopPropagation();
        showTrackMenu(more, t.id, t.name, all, i);
      });
      row.appendChild(more);
      row.addEventListener("click", () => void playWebTracks(all, i));
      list.appendChild(row);
    });
  }

  function drawQuickMix(all: WebTrack[]): void {
    // Shelf 2: quick mix (shuffled).
    shelves.insertAdjacentHTML(
      "beforeend",
      `<div class="ytm-shelf">
         <div class="ytm-shelf-head"><span class="ytm-shelf-title">Quick mix</span></div>
         <div class="ytm-shelf-row" id="ytm-mix"></div>
       </div>`,
    );
    const mix = shelves.querySelector<HTMLElement>("#ytm-mix")!;
    const shuffled = [...all].sort(() => Math.random() - 0.5).slice(0, 8);
    shuffled.forEach((t) => {
      // DOM API: user file names must not pass through innerHTML (XSS).
      const card = document.createElement("div");
      card.className = "ytm-card";
      const art = document.createElement("div");
      art.className = "ytm-card-art";
      art.innerHTML = `${icon("library", 48)}<button class="ytm-card-play" type="button" title="Play">${icon("play")}</button>`;
      const ct = document.createElement("div");
      ct.className = "ytm-card-title";
      ct.textContent = t.name;
      const cs = document.createElement("div");
      cs.className = "ytm-card-sub";
      cs.textContent = t.artist ?? "Unknown artist";
      card.append(art, ct, cs);
      const playShuffled = () => {
        const shuffledAll = [...all].sort(() => Math.random() - 0.5);
        const startIdx = Math.max(0, shuffledAll.findIndex((x) => x.id === t.id));
        void playWebTracks(shuffledAll, startIdx);
      };
      card.querySelector<HTMLButtonElement>(".ytm-card-play")!.addEventListener("click", (e) => {
        e.stopPropagation();
        playShuffled();
      });
      card.addEventListener("click", playShuffled);
      mix.appendChild(card);
    });
    makeCarousel(mix);
  }

  search?.addEventListener("input", () => {
    draw(search.value.toLowerCase());
    void runExternalSearch(search.value);
  });

  /** External catalog search (iTunes + MusicBrainz): live results under
   *  the local list; each hit plays its 30s preview or is metadata-only. */
  let searchTimer: ReturnType<typeof setTimeout> | null = null;
  let searchSeq = 0;
  async function runExternalSearch(query: string): Promise<void> {
    if (searchTimer) clearTimeout(searchTimer);
    if (query.trim().length < 3) {
      hideExternal();
      return;
    }
    searchTimer = setTimeout(async () => {
      const seq = ++searchSeq;
      try {
        const hits = await searchTracks(query);
        if (seq !== searchSeq) return; // stale response
        drawExternal(hits);
      } catch {
        hideExternal(); // offline / API blocked: local results only
      }
    }, 400);
  }

  function hideExternal(): void {
    shelves.querySelector("#ytm-external")?.remove();
  }

  function drawExternal(hits: SearchHit[]): void {
    if (!hits.length) {
      hideExternal();
      return;
    }
    let ext = shelves.querySelector<HTMLElement>("#ytm-external");
    if (!ext) {
      ext = document.createElement("div");
      ext.className = "ytm-shelf";
      ext.id = "ytm-external";
      ext.innerHTML = `<div class="ytm-shelf-head"><span class="ytm-shelf-title">Found online</span><span class="ytm-shelf-more">30-second previews</span></div><div class="ytm-shelf-row" id="ytm-ext-row"></div>`;
      shelves.prepend(ext);
    }
    const row = ext.querySelector<HTMLElement>("#ytm-ext-row")!;
    row.innerHTML = "";
    for (const hit of hits.slice(0, 12)) {
      // THIRD-PARTY data (catalog APIs): never innerHTML. Text goes
      // through textContent; URLs are validated https-only.
      const card = document.createElement("div");
      card.className = "ytm-card";
      const art = document.createElement("div");
      art.className = "ytm-card-art";
      if (safeHttpsUrl(hit.artworkUrl)) {
        art.style.backgroundImage = `url("${hit.artworkUrl}")`;
        art.style.backgroundSize = "cover";
      } else {
        art.textContent = "";
        art.innerHTML = icon("search", 48);
      }
      if (hit.previewUrl) {
        const playBtn = document.createElement("button");
        playBtn.className = "ytm-card-play";
        playBtn.type = "button";
        playBtn.title = "Play 30s preview";
        playBtn.innerHTML = icon("play");
        playBtn.addEventListener("click", (e) => {
          e.stopPropagation();
          unlockAudio();
          void addPreviewToQueue(hit);
        });
        art.appendChild(playBtn);
      }
      const ct = document.createElement("div");
      ct.className = "ytm-card-title";
      ct.textContent = hit.title;
      const cs = document.createElement("div");
      cs.className = "ytm-card-sub";
      cs.textContent = hit.artist;
      card.title = hit.previewUrl ? "Play 30-second preview" : "Metadata only (no preview)";
      card.append(art, ct, cs);
      row.appendChild(card);
    }
    makeCarousel(row);
  }

  draw("");

  async function ingest(files: FileList | null): Promise<void> {
    if (!files?.length) return;
    let added = 0;
    for (const file of Array.from(files)) {
      const id = crypto.randomUUID();
      const stem = file.name.replace(/\.[^.]+$/, "");
      const dashIdx = stem.indexOf(" - ");
      // Tags FIRST: real metadata beats filename guessing. The stem
      // pattern ("Artist - Title") stays as the fallback for untagged
      // files — never shown when the file says otherwise.
      const tags = await readTrackTags(file);
      const name = tags.title ?? (dashIdx > 0 ? stem.slice(dashIdx + 3) : stem);
      const artist = tags.artist ?? (dashIdx > 0 ? stem.slice(0, dashIdx) : null);
      const album = tags.album;
      const durationMs = tags.durationMs;
      // W8 loudness: measure R128 LUFS from the file bytes (the SAME
      // decode path playback uses — the measurement is honest by
      // construction). Best-effort: a failure leaves the track
      // un-normalized, never blocks the ingest.
      let lufs: number | null = null;
      try {
        const bytes = new Uint8Array(await file.arrayBuffer());
        lufs = await analyzeLufs(bytes, (file.name.split(".").pop() ?? "").toLowerCase());
      } catch {
        lufs = null;
      }
      registerWebTrack({ id, name, artist, album, durationMs, file, lufs });
      // Embedded cover art (best-effort): extracted from the tags, stored
      // as a Blob; a storage failure never blocks the ingest.
      if (tags.picture) {
        // Browser picture bytes always sit in a plain ArrayBuffer; the
        // library's looser type just spans Node buffers too.
        const bytes = tags.picture.data as Uint8Array<ArrayBuffer>;
        putArtwork(id, new Blob([bytes], { type: tags.picture.format })).catch(() => {});
      }
      // Persist (async, best-effort): the library must survive reloads.
      try {
        await putTrack({
          id,
          name,
          artist,
          album,
          durationMs,
          extension: (file.name.split(".").pop() ?? "").toLowerCase(),
          bytes: await file.arrayBuffer(),
          addedAt: Date.now(),
          lufs,
        });
      } catch (e) {
        // Storage failure is LOUD: the user believes their library is
        // saved; a silent console.warn betrays that trust after a reload.
        console.warn("[aurora] track not persisted (storage full?)", e);
        setToast("Storage full — tracks will not survive a reload");
      }
      added += 1;
    }
    setToast(`${added} track${added === 1 ? "" : "s"} added`);
    draw(search?.value.toLowerCase() ?? "");
  }

  // Restore persisted tracks on first render of the session (lazy:
  // bytes stay in IndexedDB until a track is played).
  try {
    const restored = await getAllTracks();
    if (restored.length && webTracks.size === 0) {
      for (const t of restored) {
        registerWebTrack({
          id: t.id,
          name: t.name,
          artist: t.artist,
          album: t.album,
          durationMs: t.durationMs,
          file: null,
          extension: t.extension,
          lufs: t.lufs ?? null,
        });
      }
      draw(search?.value.toLowerCase() ?? "");
    }
  } catch (e) {
    console.warn("[aurora] library restore failed", e);
  }

  // Drag-n-drop anywhere on the page (web shell). Bound ONCE per element:
  // render() runs on every state change and re-adding listeners here
  // duplicated every drop (N renders -> N ingests -> N duplicate tracks).
  if (!root.dataset.dropBound) {
    root.dataset.dropBound = "1";
    root.addEventListener("dragover", (e) => e.preventDefault());
    root.addEventListener("drop", (e) => {
      e.preventDefault();
      void ingest(e.dataTransfer?.files ?? null);
    });
  }
}

function update(root: HTMLElement) {
  const input = root.querySelector<HTMLInputElement>("#home-search-input");
  const query = input?.value.toLowerCase() ?? "";
  const list = root.querySelector("#ytm-list");
  const rows = list ? list.querySelectorAll(".ytm-track-row").length : 0;
  // Expected row count for the CURRENT chip + query (chips other than
  // "songs"/"all" hide the tracklist — 0 rows is correct there).
  const expected = chipKeeps(activeChip, "tracks") ? filterTracks(query).length : 0;
  if (rows !== expected) {
    // Re-draw preserving the current query and chip.
    void render(root).then(() => {
      const again = root.querySelector<HTMLInputElement>("#home-search-input");
      if (again && input?.value) again.value = input.value;
      // render() re-reads activeChip (module-level) — chip stays put.
    });
    return;
  }
  // Same track count: still refresh the history shelf — a play just
  // recorded changes it without changing the row count.
  if (chipKeeps(activeChip, "history")) {
    const historyRow = root.querySelector("#ytm-history");
    if (historyRow) {
      void renderHistory(root);
    }
  }
}

export const homePage: Page = {
  id: "home",
  title: "Home",
  mount: render,
  update,
};

export { playWebTracks };
export type { WebTrack };
