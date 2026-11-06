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
import { setToast } from "../state";
import { unlockAudio } from "../audio-bridge";
import { searchTracks, searchArtists, searchArtistTracks, addPreviewToQueue, type ArtistSearchHit, type SearchHit } from "../search-service";
import { listPlaylists } from "../playlist-service";
import { icon } from "../styles/ytm-icons";
import { trackArtworkUrl } from "../artwork-store";
import { searchFreeTracks, resolveFreeTrack, attribution, type FreeTrackHit } from "../free-music-service";
import { getServerTracks, getServerUrl, type ServerTrack } from "../server-library-service";
import { getWebTracks, playWebTracks, playUrlTrack } from "./home";

/** Result filter chips (YTM filter row). */
type Filter = "all" | "songs" | "playlists" | "online";

const FILTERS: { id: Filter; label: string }[] = [
  { id: "all", label: "Все" },
  { id: "songs", label: "Треки" },
  { id: "playlists", label: "Плейлисты" },
  { id: "online", label: "Онлайн" },
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
      <input id="search-q" type="search" placeholder="Поиск треков, исполнителей, плейлистов" autocomplete="off" />
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
  let onlineSeq = 0;

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
    title.textContent = "Треки";
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
      t2.textContent = t.artist ?? "Неизвестный исполнитель";
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
      title.textContent = "Плейлисты";
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
          empty.textContent = "Ищите по своей библиотеке и онлайн-каталогам — или сначала загрузите музыку на главной.";
          results.appendChild(empty);
          return;
        }
        const shelf = document.createElement("div");
        shelf.className = "ytm-shelf";
        const head = document.createElement("div");
        head.className = "ytm-shelf-head";
        const title = document.createElement("span");
        title.className = "ytm-shelf-title";
        title.textContent = "Недавние запросы";
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
  function drawOnlineCards(hits: SearchHit[], heading = "Найдено онлайн"): HTMLElement | null {
    if (!hits.length) return null;
    const shelf = document.createElement("div");
    shelf.className = "ytm-shelf";
    shelf.id = "search-online";
    const head = document.createElement("div");
    head.className = "ytm-shelf-head";
    const title = document.createElement("span");
    title.className = "ytm-shelf-title";
    title.textContent = "Найдено онлайн";
    const more = document.createElement("span");
    more.className = "ytm-shelf-more";
    more.textContent = "превью доступны не для всех треков";
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
    const online = ++onlineSeq;
    void Promise.allSettled([searchTracks(query), searchArtists(query)]).then(([trackResult, artistResult]) => {
      if (my !== seq || online !== onlineSeq || input.value.trim() !== query) return;
      if (trackResult.status === "fulfilled") {
        const shelf = drawOnlineCards(trackResult.value);
        if (shelf) results.appendChild(shelf);
      }
      if (artistResult.status === "fulfilled") {
        const shelf = drawArtistCards(artistResult.value);
        if (shelf) results.appendChild(shelf);
      }
      if (trackResult.status === "rejected" && artistResult.status === "rejected" && filter === "online") {
        const empty = document.createElement("p");
        empty.className = "hint";
        empty.textContent = "Онлайн-каталоги недоступны. Попробуйте позже.";
        results.appendChild(empty);
      }
    });
  }

  function drawArtistCards(artists: ArtistSearchHit[]): HTMLElement | null {
    if (!artists.length) return null;
    const shelf = document.createElement("div");
    shelf.className = "ytm-shelf";
    const head = document.createElement("div");
    head.className = "ytm-shelf-head";
    const title = document.createElement("span");
    title.className = "ytm-shelf-title";
    title.textContent = "Исполнители из iTunes";
    head.appendChild(title);
    const row = document.createElement("div");
    row.className = "ytm-shelf-row";
    shelf.append(head, row);
    for (const artist of artists.slice(0, 8)) {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "ytm-card";
      const name = document.createElement("div");
      name.className = "ytm-card-title";
      name.textContent = artist.artist;
      const genre = document.createElement("div");
      genre.className = "ytm-card-sub";
      genre.textContent = artist.genre ?? "Исполнитель";
      button.append(name, genre);
      button.addEventListener("click", () => {
        const local = getWebTracks().filter((track) => track.artist?.trim().toLocaleLowerCase() === artist.artist.trim().toLocaleLowerCase());
        if (local.length) {
          navigate(`artist?name=${encodeURIComponent(artist.artist)}`);
          return;
        }
        button.disabled = true;
        button.title = "Загружаем треки исполнителя…";
        const request = ++seq;
        ++onlineSeq;
        void searchArtistTracks(artist.artist).then((hits) => {
          if (request !== seq) return;
          if (input.value.trim().length) input.value = "";
          results.replaceChildren();
          const shelf = drawOnlineCards(hits, `Треки исполнителя: ${artist.artist}`);
          if (shelf) results.appendChild(shelf);
          else {
            const message = document.createElement("p");
            message.className = "hint";
            message.textContent = `В каталоге iTunes не найдены треки исполнителя ${artist.artist}.`;
            results.appendChild(message);
          }
          syncHash("");
        }).catch(() => {
          button.disabled = false;
          button.title = "Не удалось загрузить треки. Нажмите, чтобы повторить.";
        });
      });
      row.appendChild(button);
    }
    return shelf;
  }


  function draw(q: string): void {
    results.innerHTML = "";
    if (!q.trim()) {
      void drawRecent();
      void drawServerShelf();
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
      drawFree(q);
      void drawServerTracks(q);
    }
    // Honest empty state (local half only when offline).
    if (filter !== "online" && !songs) {
      const empty = document.createElement("p");
      empty.className = "hint";
      empty.textContent =
        filter === "playlists"
          ? "Подходящих плейлистов нет."
          : "В библиотеке ничего не найдено — проверьте онлайн-результаты ниже.";
      results.appendChild(empty);
    }
  }

  /** Free full-track shelf (archive.org, CC-licensed): whole songs,
   *  attributed. Resolve-and-play on click — the file URL is fetched
   *  lazily (item metadata) then fed to the kernel. */
  function drawFree(q: string): void {
    const query = q.trim();
    if (query.length < 3) return;
    const my = ++seq;
    searchFreeTracks(query)
      .then(async (hits) => {
        if (my !== seq) return;
        if (input.value.trim() !== query) return;
        if (!hits.length) return;
        const shelf = document.createElement("div");
        shelf.className = "ytm-shelf";
        const head = document.createElement("div");
        head.className = "ytm-shelf-head";
        const title = document.createElement("span");
        title.className = "ytm-shelf-title";
        title.textContent = "Бесплатно — полные треки";
        const more = document.createElement("span");
        more.className = "ytm-shelf-more";
        more.textContent = "Creative Commons · archive.org";
        head.append(title, more);
        const row = document.createElement("div");
        row.className = "ytm-tracklist";
        shelf.append(head, row);
        for (const hit of hits) {
          const r = document.createElement("div");
          r.className = "ytm-track-row";
          const idx = document.createElement("span");
          idx.className = "ytm-track-index";
          idx.innerHTML = icon("play", 14);
          const thumb = document.createElement("div");
          thumb.className = "ytm-track-thumb";
          thumb.innerHTML = icon("explore", 18);
          const main = document.createElement("div");
          main.className = "ytm-track-main";
          const t1 = document.createElement("div");
          t1.className = "ytm-track-title";
          t1.textContent = hit.title;
          const t2 = document.createElement("div");
          t2.className = "ytm-track-meta";
          t2.textContent = attribution(hit);
          main.append(t1, t2);
          r.append(idx, thumb, main);
          r.title = "Play the full track (Creative Commons)";
          r.addEventListener("click", () => {
            unlockAudio();
            setToast(`Loading "${hit.title.slice(0, 30)}"…`);
            void resolveFreeTrack(hit)
              .then(async (url) => {
                if (!url) {
                  setToast("Track unavailable");
                  return;
                }
                try {
                  await playUrlTrack(url, hit.title, hit.creator, hit.durationS ? hit.durationS * 1000 : null, [
                    { url, title: hit.title, artist: hit.creator, license: hit.license, archiveIdentifier: hit.identifier, itemUrl: hit.itemUrl },
                  ], 0);
                } catch (e) {
                  console.error("[aurora] free-track play failed:", e);
                  setToast(`Play failed: ${String(e).slice(0, 80)}`);
                }
              })
              .catch((e) => {
                console.error("[aurora] resolveFreeTrack failed:", e);
                setToast(`Load failed: ${String(e).slice(0, 80)}`);
              });
          });
          row.appendChild(r);
        }
        results.appendChild(shelf);
      })
      .catch(() => {
        /* archive.org unreachable: honest silence */
      });
  }

  /** The owner's server library: full tracks at the configured URL.
   *  Empty query = the whole shelf; a query filters it. */
  async function drawServerTracks(q: string): Promise<void> {
    const query = q.trim().toLowerCase();
    const tracks = await getServerTracks();
    const hits = tracks.filter(
      (t) => !query || t.title.toLowerCase().includes(query) || (t.artist ?? "").toLowerCase().includes(query),
    );
    if (!hits.length) return;
    results.appendChild(buildServerShelf(hits));
  }

  async function drawServerShelf(): Promise<void> {
    const base = await getServerUrl();
    if (!base) return; // not configured: hidden (honest — nothing to show)
    const tracks = await getServerTracks();
    if (!tracks.length) return;
    results.appendChild(buildServerShelf(tracks));
  }

  function buildServerShelf(tracks: ServerTrack[]): HTMLElement {
    const shelf = document.createElement("div");
    shelf.className = "ytm-shelf";
    const head = document.createElement("div");
    head.className = "ytm-shelf-head";
    const title = document.createElement("span");
    title.className = "ytm-shelf-title";
    title.textContent = "Ваш сервер";
    head.appendChild(title);
    const row = document.createElement("div");
    row.className = "ytm-tracklist";
    shelf.append(head, row);
    const list = tracks.map((t) => ({
      url: t.url,
      title: t.title,
      artist: t.artist,
      durationMs: t.durationMs ?? null,
      license: t.license ?? null,
    }));
    tracks.forEach((t, i) => {
      const r = document.createElement("div");
      r.className = "ytm-track-row";
      const idx = document.createElement("span");
      idx.className = "ytm-track-index";
      idx.innerHTML = icon("play", 14);
      const thumb = document.createElement("div");
      thumb.className = "ytm-track-thumb";
      thumb.innerHTML = icon("home", 18);
      const main = document.createElement("div");
      main.className = "ytm-track-main";
      const t1 = document.createElement("div");
      t1.className = "ytm-track-title";
      t1.textContent = t.title;
      const t2 = document.createElement("div");
      t2.className = "ytm-track-meta";
      t2.textContent = t.license ? `${t.artist ?? "Unknown"} · ${t.license}` : (t.artist ?? "Unknown");
      main.append(t1, t2);
      const dur = document.createElement("span");
      dur.className = "ytm-track-dur";
      dur.textContent = fmtDur(t.durationMs ?? null);
      r.append(idx, thumb, main, dur);
      r.addEventListener("click", () => {
        unlockAudio();
        setToast(`Loading "${t.title.slice(0, 30)}"…`);
        void playUrlTrack(t.url, t.title, t.artist, t.durationMs ?? null, list, i);
      });
      row.appendChild(r);
    });
    return shelf;
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
