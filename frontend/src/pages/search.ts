/** Search page (spec 2.1 + 2.2): one input, filter chips (All / Songs /
 *  Playlists / Online), live results. Local tracks and playlists come
 *  from the session library + IndexedDB; online hits from the external
 *  catalogs (honest 30-second previews, labeled). Entering from the
 *  topbar (#/search?q=…) presets the query; every debounced keystroke
 *  rewrites the hash via replaceState — shareable URL without router
 *  churn (no hashchange, no remount). */

import type { Page } from "../router";
import { navigate } from "../router";
import { kvGet, kvPut } from "../library-db";
import { unlockAudio } from "../audio-bridge";
import { searchTracks, addPreviewToQueue, type SearchHit } from "../search-service";
import { listPlaylists } from "../playlist-service";
import { icon } from "../styles/ytm-icons";
import { trackArtworkUrl } from "../artwork-store";
import { getWebTracks, playWebTracks } from "./home";

/** Result filter chips (YTM filter row). */
type Filter = "all" | "songs" | "playlists" | "online";

const FILTERS: { id: Filter; label: string }[] = [
  { id: "all", label: "All" },
  { id: "songs", label: "Songs" },
  { id: "playlists", label: "Playlists" },
  { id: "online", label: "Online" },
];

const RECENT_KEY = "aw:recent-searches";
const RECENT_MAX = 10;

/** Third-party URL trust gate: only absolute https URLs pass (catalog
 *  data must never inject javascript:/data: into style attributes). */
function safeHttpsUrl(u: string | null | undefined): u is string {
  if (!u) return false;
  try {
    return new URL(u).protocol === "https:";
  } catch {
    return false;
  }
}

function fmtDur(ms: number | null): string {
  if (!ms) return "";
  const s = Math.round(ms / 1000);
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

/** Persist a search into the recent list (unshift + dedup, max 10). */
async function recordRecentSearch(q: string): Promise<void> {
  const query = q.trim();
  if (!query) return;
  try {
    const list = (await kvGet<string[]>(RECENT_KEY)) ?? [];
    const next = [query, ...list.filter((s) => s.toLowerCase() !== query.toLowerCase())];
    await kvPut(RECENT_KEY, next.slice(0, RECENT_MAX));
  } catch {
    /* storage full — recents are best-effort, never block the search */
  }
}

async function render(root: HTMLElement) {
  // ?q= from the hash (topbar Enter / shared URL / back nav).
  const params = new URLSearchParams(window.location.hash.split("?")[1] ?? "");
  const initialQ = params.get("q") ?? "";

  root.innerHTML = `
    <div class="ytm-search">
      ${icon("search", 22)}
      <input id="search-q" type="search" placeholder="Search songs, artists, playlists" autocomplete="off" />
    </div>
    <div class="ytm-chips" id="search-chips"></div>
    <div id="search-results"></div>
  `;

  const input = root.querySelector<HTMLInputElement>("#search-q")!;
  const chipsRow = root.querySelector<HTMLElement>("#search-chips")!;
  const results = root.querySelector<HTMLElement>("#search-results")!;

  let filter: Filter = "all";
  input.value = initialQ;

  // Chips row.
  for (const f of FILTERS) {
    const chip = document.createElement("button");
    chip.type = "button";
    chip.className = "ytm-chip" + (f.id === filter ? " active" : "");
    chip.textContent = f.label;
    chip.addEventListener("click", () => {
      filter = f.id;
      chipsRow.querySelectorAll(".ytm-chip").forEach((c) => c.classList.remove("active"));
      chip.classList.add("active");
      draw(input.value);
    });
    chipsRow.appendChild(chip);
  }

  /** Rewrite the URL without router churn: replaceState never fires
   *  hashchange, so the page is not remounted under the typist. */
  const syncHash = (q: string): void => {
    const hash = q ? `#/search?q=${encodeURIComponent(q)}` : "#/search";
    history.replaceState(null, "", hash);
  };

  // Debounced live search (200ms) + stale-response guard for the online
  // half (responses may arrive out of order).
  let timer: ReturnType<typeof setTimeout> | null = null;
  let seq = 0;

  function schedule(): void {
    if (timer) clearTimeout(timer);
    timer = setTimeout(() => {
      timer = null;
      const q = input.value.trim();
      if (q) void recordRecentSearch(q);
      draw(input.value);
    }, 200);
  }

  input.addEventListener("input", () => {
    syncHash(input.value);
    schedule();
  });
  // Enter commits immediately (no 200ms wait) — the search IS the intent.
  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      if (timer) clearTimeout(timer);
      timer = null;
      const q = input.value.trim();
      if (q) void recordRecentSearch(q);
      syncHash(input.value);
      draw(input.value);
    }
  });

  /** Local (session) track rows filtered by the query. DOM API, not
   *  innerHTML: track names are user input (XSS law). */
  function drawSongs(q: string): HTMLElement | null {
    const query = q.toLowerCase();
    const all = getWebTracks();
    const hits = all.filter(
      (t) =>
        !query ||
        t.name.toLowerCase().includes(query) ||
        (t.artist ?? "").toLowerCase().includes(query),
    );
    if (!hits.length) return null;
    const shelf = document.createElement("div");
    shelf.className = "ytm-shelf";
    const head = document.createElement("div");
    head.className = "ytm-shelf-head";
    const title = document.createElement("span");
    title.className = "ytm-shelf-title";
    title.textContent = "Songs";
    head.appendChild(title);
    const list = document.createElement("div");
    list.className = "ytm-tracklist";
    shelf.append(head, list);
    hits.forEach((t, i) => {
      const row = document.createElement("div");
      row.className = "ytm-track-row";
      const idx = document.createElement("span");
      idx.className = "ytm-track-index";
      idx.textContent = String(i + 1);
      const thumb = document.createElement("div");
      thumb.className = "ytm-track-thumb";
      thumb.innerHTML = icon("library", 18); // trusted constant
      // Embedded artwork (async): replace the icon only if this row is
      // still on screen when the URL resolves.
      void trackArtworkUrl(t.id).then((url) => {
        if (!url || !thumb.isConnected) return;
        thumb.style.backgroundImage = `url("${url}")`;
        thumb.style.backgroundSize = "cover";
        thumb.style.backgroundPosition = "center";
        thumb.innerHTML = "";
      });
      const main = document.createElement("div");
      main.className = "ytm-track-main";
      const t1 = document.createElement("div");
      t1.className = "ytm-track-title";
      t1.textContent = t.name;
      const t2 = document.createElement("div");
      t2.className = "ytm-track-meta";
      t2.textContent = t.artist ?? "Unknown artist";
      main.append(t1, t2);
      const dur = document.createElement("span");
      dur.className = "ytm-track-dur";
      dur.textContent = fmtDur(t.durationMs);
      row.append(idx, thumb, main, dur);
      row.addEventListener("click", () => void playWebTracks(hits, i));
      list.appendChild(row);
    });
    return shelf;
  }

  /** Local playlist cards filtered by name. */
  function drawPlaylists(q: string): Promise<HTMLElement | null> {
    const query = q.toLowerCase();
    return listPlaylists().then((lists) => {
      const hits = lists.filter((p) => p.name.toLowerCase().includes(query));
      if (!hits.length) return null;
      const shelf = document.createElement("div");
      shelf.className = "ytm-shelf";
      const head = document.createElement("div");
      head.className = "ytm-shelf-head";
      const title = document.createElement("span");
      title.className = "ytm-shelf-title";
      title.textContent = "Playlists";
      head.appendChild(title);
      const row = document.createElement("div");
      row.className = "ytm-shelf-row";
      shelf.append(head, row);
      for (const pl of hits) {
        const card = document.createElement("div");
        card.className = "ytm-card";
        const art = document.createElement("div");
        art.className = "ytm-card-art";
        art.innerHTML = icon("library", 48); // trusted constant
        const ct = document.createElement("div");
        ct.className = "ytm-card-title";
        ct.textContent = pl.name; // user string -> textContent (XSS law)
        const cs = document.createElement("div");
        cs.className = "ytm-card-sub";
        cs.textContent = `${pl.trackIds.length} track${pl.trackIds.length === 1 ? "" : "s"}`;
        card.append(art, ct, cs);
        card.addEventListener("click", () => navigate(`playlist?id=${pl.id}`));
        row.appendChild(card);
      }
      return shelf;
    });
  }

  /** Recent searches (empty query): chip buttons that re-search. */
  function drawRecent(): Promise<void> {
    return kvGet<string[]>(RECENT_KEY)
      .then((list) => {
        if (!list?.length) {
          const empty = document.createElement("p");
          empty.className = "hint";
          empty.textContent = "Search your library and the online catalogs — or drop music on the Home page first.";
          results.appendChild(empty);
          return;
        }
        const shelf = document.createElement("div");
        shelf.className = "ytm-shelf";
        const head = document.createElement("div");
        head.className = "ytm-shelf-head";
        const title = document.createElement("span");
        title.className = "ytm-shelf-title";
        title.textContent = "Recent searches";
        head.appendChild(title);
        const row = document.createElement("div");
        row.className = "ytm-chips";
        shelf.append(head, row);
        for (const q of list.slice(0, RECENT_MAX)) {
          const chip = document.createElement("button");
          chip.type = "button";
          chip.className = "ytm-chip";
          chip.textContent = q; // user string -> textContent
          chip.addEventListener("click", () => {
            input.value = q;
            syncHash(q);
            void recordRecentSearch(q);
            draw(q);
          });
          row.appendChild(chip);
        }
        results.appendChild(shelf);
      })
      .catch(() => {
        /* storage failed: show nothing rather than a fake empty state */
      });
  }

  /** Online catalog hits ("Found online"): the home drawExternal card
   *  pattern — DOM API for third-party data, https-only artwork,
   *  preview play buttons. */
  function drawOnlineCards(hits: SearchHit[]): HTMLElement | null {
    if (!hits.length) return null;
    const shelf = document.createElement("div");
    shelf.className = "ytm-shelf";
    shelf.id = "search-online";
    const head = document.createElement("div");
    head.className = "ytm-shelf-head";
    const title = document.createElement("span");
    title.className = "ytm-shelf-title";
    title.textContent = "Found online";
    const more = document.createElement("span");
    more.className = "ytm-shelf-more";
    more.textContent = "30-second previews";
    head.append(title, more);
    const row = document.createElement("div");
    row.className = "ytm-shelf-row";
    shelf.append(head, row);
    for (const hit of hits.slice(0, 12)) {
      const card = document.createElement("div");
      card.className = "ytm-card";
      const art = document.createElement("div");
      art.className = "ytm-card-art";
      if (safeHttpsUrl(hit.artworkUrl)) {
        art.style.backgroundImage = `url("${hit.artworkUrl}")`;
        art.style.backgroundSize = "cover";
      } else {
        art.innerHTML = icon("search", 48); // trusted constant
      }
      if (hit.previewUrl) {
        const playBtn = document.createElement("button");
        playBtn.className = "ytm-card-play";
        playBtn.type = "button";
        playBtn.title = "Play 30s preview";
        playBtn.innerHTML = icon("play");
        playBtn.addEventListener("click", (e) => {
          e.stopPropagation();
          unlockAudio(); // this click is the user gesture
          void addPreviewToQueue(hit);
        });
        art.appendChild(playBtn);
      }
      const ct = document.createElement("div");
      ct.className = "ytm-card-title";
      ct.textContent = hit.title; // third-party string -> textContent
      const cs = document.createElement("div");
      cs.className = "ytm-card-sub";
      cs.textContent = hit.artist;
      card.title = hit.previewUrl ? "Play 30-second preview" : "Metadata only (no preview)";
      card.append(art, ct, cs);
      row.appendChild(card);
    }
    return shelf;
  }

  function drawOnline(q: string): void {
    const query = q.trim();
    if (query.length < 3) return; // same floor as home: no 1-2 char network spam
    const my = ++seq;
    searchTracks(query)
      .then((hits) => {
        if (my !== seq) return; // stale response
        if (input.value.trim() !== query) return; // user moved on
        const shelf = drawOnlineCards(hits);
        if (shelf) results.appendChild(shelf);
        else if (filter === "online") {
          const empty = document.createElement("p");
          empty.className = "hint";
          empty.textContent = "No online results.";
          results.appendChild(empty);
        }
      })
      .catch(() => {
        /* offline / API blocked: local results only — honest silence */
      });
  }

  function draw(q: string): void {
    results.innerHTML = "";
    if (!q.trim()) {
      void drawRecent();
      return;
    }
    const songs = filter === "all" || filter === "songs" ? drawSongs(q) : null;
    if (songs) results.appendChild(songs);
    if (filter === "all" || filter === "playlists") {
      void drawPlaylists(q).then((shelf) => {
        if (shelf) results.appendChild(shelf);
      });
    }
    if (filter === "all" || filter === "online") {
      drawOnline(q);
    }
    // Honest empty state (local half only when offline).
    if (filter !== "online" && !songs) {
      const empty = document.createElement("p");
      empty.className = "hint";
      empty.textContent =
        filter === "playlists"
          ? "No matching playlists."
          : "Nothing matches in your library — check the online results below.";
      results.appendChild(empty);
    }
  }

  draw(initialQ);
  if (initialQ) void recordRecentSearch(initialQ);
  input.focus();
}

export const searchPage: Page = {
  id: "search",
  title: "Search",
  mount: render,
};
