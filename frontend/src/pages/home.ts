/** YTM Home page: shelves of cards (your music, quick mix).
 *  Web-shell-only route; the desktop shell keeps its own player page. */

import { invoke, platform, registerWebFile } from "../invoke";
import { app, playQueueIndex, setQueue, setToast } from "../state";
import { unlockAudio } from "../audio-bridge";
import { searchTracks, addPreviewToQueue, type SearchHit } from "../search-service";
import { icon } from "../styles/ytm-icons";
import type { Page } from "../router";

interface WebTrack {
  id: string;
  name: string;
  artist: string | null;
  album: string | null;
  durationMs: number | null;
  /** In-memory file handle (page lifetime). */
  file: File;
  /** The web:// path registered with the kernel (after first play). */
  kernelPath: string | null;
}

const webTracks = new Map<string, WebTrack>();

export function registerWebTrack(t: Omit<WebTrack, "kernelPath">) {
  webTracks.set(t.id, { ...t, kernelPath: null });
}

export function webTrackCount(): number {
  return webTracks.size;
}

function fmtDur(ms: number | null): string {
  if (!ms) return "";
  const s = Math.round(ms / 1000);
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

/** Play a set of web tracks through the kernel queue (registering each
 *  file with the wasm kernel first — the registry clones on play). */
async function playWebTracks(tracks: WebTrack[], index = 0): Promise<void> {
  if (!tracks.length) return;
  unlockAudio(); // the click that got here is the user gesture
  const queue = [];
  for (const t of tracks) {
    t.kernelPath = await registerWebFile(t.file);
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
      const row = document.createElement("div");
      row.className = "ytm-track-row";
      row.innerHTML = `
        <span class="ytm-track-index">${i + 1}</span>
        <div class="ytm-track-thumb">${icon("library", 18)}</div>
        <div class="ytm-track-main">
          <div class="ytm-track-title">${t.name}</div>
          <div class="ytm-track-meta">${t.artist ?? "Unknown artist"}${t.album ? " • " + t.album : ""}</div>
        </div>
        <span class="ytm-track-dur">${fmtDur(t.durationMs)}</span>
      `;
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
      const card = document.createElement("div");
      card.className = "ytm-card";
      card.innerHTML = `
        <div class="ytm-card-art">${icon("library", 48)}
          <button class="ytm-card-play" type="button" title="Play">${icon("play")}</button>
        </div>
        <div class="ytm-card-title">${t.name}</div>
        <div class="ytm-card-sub">${t.artist ?? "Unknown artist"}</div>
      `;
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
      const card = document.createElement("div");
      card.className = "ytm-card";
      card.innerHTML = `
        <div class="ytm-card-art" style="${hit.artworkUrl ? `background-image:url('${hit.artworkUrl}');background-size:cover` : ""}">
          ${hit.artworkUrl ? "" : icon("search", 48)}
          ${hit.previewUrl ? `<button class="ytm-card-play" type="button" title="Play 30s preview">${icon("play")}</button>` : ""}
        </div>
        <div class="ytm-card-title">${hit.title}</div>
        <div class="ytm-card-sub">${hit.artist}</div>
      `;
      card.querySelector<HTMLButtonElement>(".ytm-card-play")?.addEventListener("click", (e) => {
        e.stopPropagation();
        unlockAudio();
        void addPreviewToQueue(hit);
      });
      card.title = hit.previewUrl ? "Play 30-second preview" : "Metadata only (no preview)";
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
      added += 1;
    }
    setToast(`${added} track${added === 1 ? "" : "s"} added`);
    draw(search?.value.toLowerCase() ?? "");
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
