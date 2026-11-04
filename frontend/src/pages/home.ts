/** YTM Home page: shelves of cards (your music, quick mix).
 *  Web-shell-only route; the desktop shell keeps its own player page. */

import { invoke, platform, registerWebFile, registerWebBytes } from "../invoke";
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
  type StoredTrack,
} from "../library-db";

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
  /** The web:// path registered with the kernel (after first play). */
  kernelPath: string | null;
}

const webTracks = new Map<string, WebTrack>();

export function registerWebTrack(t: Omit<WebTrack, "kernelPath" | "extension"> & { extension?: string }) {
  webTracks.set(t.id, {
    ...t,
    extension: t.extension ?? (t.file ? (t.file.name.split(".").pop() ?? "").toLowerCase() : ""),
    kernelPath: null,
  });
}

export function webTrackCount(): number {
  return webTracks.size;
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
  await doPlayWebTracks(tracks, index);
}

async function doPlayWebTracks(tracks: WebTrack[], index = 0): Promise<void> {
  const queue = [];
  for (const t of tracks) {
    t.kernelPath = t.file
      ? await registerWebFile(t.file)
      : await registerWebBytes(t.id, new Uint8Array((await loadStoredBytes(t.id)) ?? []), t.extension);
    queue.push({
      path: t.kernelPath,
      title: t.name,
      artist: t.artist,
      replaygain_track_db: null as number | null,
      replaygain_album_db: null as number | null,
      bpm: null as number | null,
    });
  }
  await setQueue(queue);
  await playQueueIndex(index);
}

async function render(root: HTMLElement) {
  root.innerHTML = `
    <div class="ytm-search">
      ${icon("search", 22)}
      <input id="ytm-search-input" type="search" placeholder="Search" />
    </div>
    <div id="ytm-shelves"></div>
    <p class="hint" id="ytm-empty"></p>
  `;

  const shelves = root.querySelector<HTMLElement>("#ytm-shelves")!;
  const empty = root.querySelector<HTMLElement>("#ytm-empty")!;
  const search = root.querySelector<HTMLInputElement>("#ytm-search-input");

  function draw(query: string): void {
    const all = [...webTracks.values()].filter(
      (t) =>
        !query ||
        t.name.toLowerCase().includes(query) ||
        (t.artist ?? "").toLowerCase().includes(query),
    );
    shelves.innerHTML = "";
    if (!all.length) {
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
    empty.textContent = "";

    // Shelf 1: all tracks as YTM song rows.
    shelves.insertAdjacentHTML(
      "beforeend",
      `<div class="ytm-shelf">
         <div class="ytm-shelf-head"><span class="ytm-shelf-title">Your music</span></div>
         <div class="ytm-tracklist" id="ytm-list"></div>
       </div>`,
    );
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
      row.addEventListener("click", () => void playWebTracks(all, i));
      list.appendChild(row);
    });

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
  }

  draw("");

  async function ingest(files: FileList | null): Promise<void> {
    if (!files?.length) return;
    let added = 0;
    for (const file of Array.from(files)) {
      const id = crypto.randomUUID();
      const stem = file.name.replace(/\.[^.]+$/, "");
      const dashIdx = stem.indexOf(" - ");
      const name = dashIdx > 0 ? stem.slice(dashIdx + 3) : stem;
      const artist = dashIdx > 0 ? stem.slice(0, dashIdx) : null;
      registerWebTrack({ id, name, artist, album: null, durationMs: null, file });
      // Persist (async, best-effort): the library must survive reloads.
      try {
        await putTrack({
          id,
          name,
          artist,
          album: null,
          durationMs: null,
          extension: (file.name.split(".").pop() ?? "").toLowerCase(),
          bytes: await file.arrayBuffer(),
          addedAt: Date.now(),
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
  const list = root.querySelector("#ytm-list");
  const rows = list ? list.querySelectorAll(".ytm-track-row").length : 0;
  if (rows !== webTracks.size) {
    const input = root.querySelector<HTMLInputElement>("#ytm-search-input");
    // Re-draw preserving the current query.
    void render(root).then(() => {
      const again = root.querySelector<HTMLInputElement>("#ytm-search-input");
      if (again && input?.value) again.value = input.value;
    });
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
