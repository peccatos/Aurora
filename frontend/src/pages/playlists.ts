/** Playlists pages (YTM parity): the library shelf list + the detail
 *  page with the shared tracklist renderer. Playlists are local
 *  (IndexedDB) — see playlist-service. */

import type { Page } from "../router";
import { navigate } from "../router";
import { setToast } from "../state";
import { unlockAudio } from "../audio-bridge";
import { icon } from "../styles/ytm-icons";
import {
  listPlaylists,
  createPlaylist,
  getPlaylist,
  renamePlaylist,
  deletePlaylist,
  removeFromPlaylist,
  reorderPlaylist,
} from "../playlist-service";
import { getAllTracks, type StoredTrack } from "../library-db";
import { importWebPlaylist } from "../web-playlist-import";
import {
  importRemotePlaylist,
  type PlaylistImportReport,
  type RemotePlaylistSummary,
  type RemoteProvider,
} from "../remote-playlist-import";

/** Renders one import report (shared by the file and provider paths). */
function showImportReport(root: HTMLElement, report: PlaylistImportReport): void {
  const box = root.querySelector<HTMLElement>("#pl-import-report");
  if (!box) return;
  box.hidden = false;
  box.innerHTML = "";
  const heading = document.createElement("h2");
  heading.textContent =
    `Import report: ${report.matched} matched, ${report.unmatched} not found` +
    (report.ambiguous ? `, ${report.ambiguous} ambiguous` : "") +
    (report.duplicateMatches ? `, ${report.duplicateMatches} duplicate` : "") +
    ` of ${report.totalRemoteTracks} source position${report.totalRemoteTracks === 1 ? "" : "s"}` +
    (report.uniqueLocalTracks !== report.matched
      ? ` (${report.uniqueLocalTracks} unique local track${report.uniqueLocalTracks === 1 ? "" : "s"})`
      : "");
  const close = document.createElement("button");
  close.type = "button";
  close.className = "btn";
  close.textContent = "Dismiss";
  close.addEventListener("click", () => box.remove());
  box.append(heading, close);

  const section = (title: string, items: string[]) => {
    if (!items.length) return;
    const sub = document.createElement("h3");
    sub.textContent = title;
    const list = document.createElement("ul");
    for (const item of items) {
      const row = document.createElement("li");
      row.textContent = item;
      list.appendChild(row);
    }
    box.append(sub, list);
  };
  section("Not in your library", report.unmatchedLabels);
  section(
    "Ambiguous (skipped, needs a manual pick)",
    report.ambiguousEntries.map((entry) =>
      `${entry.sourcePosition + 1}. ${entry.remote.title} — ${entry.candidates.length} candidates`,
    ),
  );
  section(
    "Repeated in the source playlist",
    report.matchedEntries
      .filter((entry) => entry.duplicate)
      .map((entry) => `${entry.sourcePosition + 1}. ${entry.remote.title} (already added)`),
  );
}

/** Provider song-list import: pick a provider, list its playlists, fetch
 *  one snapshot and match it against the local library. */
async function importFromProvider(root: HTMLElement, provider: RemoteProvider): Promise<void> {
  const { listRemotePlaylists, getRemotePlaylist } = await import("../web-provider-adapter");
  const page = await listRemotePlaylists(provider);
  if (!page.items.length) {
    setToast(`No ${provider} playlists found for the connected account`);
    return;
  }
  const chosen = pickRemotePlaylist(page.items);
  if (!chosen) return;
  const requestedName = window.prompt("Playlist name", chosen.name);
  if (requestedName === null) return;
  const snapshot = await getRemotePlaylist(provider, chosen.id);
  const localTracks = await getAllTracks();
  const report = await importRemotePlaylist(snapshot, { name: requestedName, localTracks });
  setToast(
    `Imported "${report.playlist.name}": ${report.matched} matched` +
      (report.unmatched ? `, ${report.unmatched} not found` : ""),
  );
  renderList(root);
  showImportReport(root, report);
}

/** Minimal provider picker (no in-app modal component exists yet). */
function pickRemotePlaylist(items: RemotePlaylistSummary[]): RemotePlaylistSummary | null {
  const labels = items.map(
    (item, i) => `${i + 1}. ${item.name}${item.trackCount === null ? "" : ` (${item.trackCount})`}`,
  );
  const answer = window.prompt(`Import which playlist?\n${labels.join("\n")}`, "1");
  if (answer === null) return null;
  const index = Number(answer.trim()) - 1;
  return Number.isInteger(index) && index >= 0 && index < items.length ? items[index] : null;
}

/** Play a playlist: resolve track ids -> entries, queue them. The play
 *  pipeline (register bytes, LUFS) lives with the Home module — reimport
 *  its exported player. */
async function playPlaylist(playlistId: string, startIndex = 0): Promise<void> {
  const { playWebTracksByIds } = await import("./home");
  await playWebTracksByIds(playlistId, startIndex);
}

function trackRows(
  container: HTMLElement,
  entries: StoredTrack[],
  opts: { onPlay: (index: number) => void; onRemove?: (id: string) => void },
): void {
  entries.forEach((t, i) => {
    const row = document.createElement("div");
    row.className = "ytm-track-row";
    row.draggable = Boolean(opts.onRemove); // reorder only in edit contexts
    const idx = document.createElement("span");
    idx.className = "ytm-track-index";
    idx.textContent = String(i + 1);
    const thumb = document.createElement("div");
    thumb.className = "ytm-track-thumb";
    thumb.innerHTML = icon("library", 18);
    const main = document.createElement("div");
    main.className = "ytm-track-main";
    const title = document.createElement("div");
    title.className = "ytm-track-title";
    title.textContent = t.name;
    const meta = document.createElement("div");
    meta.className = "ytm-track-meta";
    meta.textContent = t.artist ?? "Unknown artist";
    main.append(title, meta);
    row.append(idx, thumb, main);
    if (opts.onRemove) {
      const del = document.createElement("button");
      del.type = "button";
      del.className = "ytm-track-del";
      del.title = "Remove from playlist";
      del.textContent = "✕";
      del.addEventListener("click", (e) => {
        e.stopPropagation();
        opts.onRemove?.(t.id);
      });
      row.appendChild(del);
    }
    row.addEventListener("click", () => opts.onPlay(i));
    // Drag reorder (pointer-based; simple HTML5 DnD within the list).
    row.dataset.dragId = t.id;
    container.appendChild(row);
  });
}

function renderList(root: HTMLElement) {
  root.innerHTML = `
    <div class="ytm-playlists-head">
      <h1 class="page-title">Playlists</h1>
      <button class="btn" id="pl-new" type="button">＋ New playlist</button>
      <button class="btn" id="pl-import-button" type="button">⇩ Import playlist</button>
      <button class="btn" id="pl-remote-spotify" type="button">⇩ Spotify</button>
      <button class="btn" id="pl-remote-ytm" type="button">⇩ YouTube Music</button>
      <input id="pl-import" class="visually-hidden-file" type="file" accept=".m3u,.m3u8,.pls,.json,.csv,.txt" aria-label="Choose a playlist file" />
      <button class="btn" id="pl-back-home" type="button">⌂ Home</button>
    </div>
    <div class="ytm-shelf-row" id="pl-grid"></div>
    <p class="hint" id="pl-empty"></p>
    <section class="pl-import-report" id="pl-import-report" aria-live="polite" hidden></section>
  `;

  const grid = root.querySelector<HTMLElement>("#pl-grid")!;
  const empty = root.querySelector<HTMLElement>("#pl-empty")!;

  root.querySelector<HTMLButtonElement>("#pl-back-home")?.addEventListener("click", () => navigate("home"));
  root.querySelector<HTMLButtonElement>("#pl-import-button")?.addEventListener("click", () => {
    root.querySelector<HTMLInputElement>("#pl-import")?.click();
  });

  for (const [buttonId, provider] of [["#pl-remote-spotify", "spotify"], ["#pl-remote-ytm", "youtube-music"]] as const) {
    root.querySelector<HTMLButtonElement>(buttonId)?.addEventListener("click", () => {
      void importFromProvider(root, provider).catch((error: unknown) => {
        setToast(`Import failed: ${error instanceof Error ? error.message : String(error)}`);
      });
    });
  }

  root.querySelector<HTMLButtonElement>("#pl-new")?.addEventListener("click", async () => {
    const name = window.prompt("Playlist name");
    if (name === null) return;
    const pl = await createPlaylist(name);
    setToast(`Created "${pl.name}"`);
    navigate(`playlist?id=${pl.id}`);
  });

  root.querySelector<HTMLInputElement>("#pl-import")?.addEventListener("change", async (event) => {
    const input = event.currentTarget as HTMLInputElement;
    const file = input.files?.[0];
    input.value = "";
    if (!file) return;
    try {
      const requestedName = window.prompt("Playlist name", file.name.replace(/\.[^.]+$/, ""));
      if (requestedName === null) return;
      const report = await importWebPlaylist(file, requestedName);
      setToast(`Imported "${report.playlist.name}": ${report.matched} matched` + (report.unmatched.length ? `, ${report.unmatched.length} not found` : ""));
      renderList(root);
      const box = root.querySelector<HTMLElement>("#pl-import-report");
      if (box) {
        box.hidden = false;
        box.innerHTML = "";
        const heading = document.createElement("h2");
        heading.textContent = `Import report: ${report.matched} matched, ${report.unmatched.length} not found of ${report.totalEntries} entries`;
        const close = document.createElement("button");
        close.type = "button";
        close.className = "btn";
        close.textContent = "Dismiss";
        close.addEventListener("click", () => box.remove());
        box.append(heading, close);
        if (report.unmatched.length) {
          const list = document.createElement("ul");
          for (const item of report.unmatched) {
            const row = document.createElement("li");
            row.textContent = item;
            list.appendChild(row);
          }
          box.appendChild(list);
        }
      }
    } catch (error) {
      setToast(`Import failed: ${error instanceof Error ? error.message : String(error)}`);
    }
  });

  void (async () => {
    const lists = await listPlaylists();
    const tracks = await getAllTracks();
    if (!lists.length) {
      empty.textContent = "No playlists yet — create one and add tracks from the track menu.";
      return;
    }
    for (const pl of lists) {
      const card = document.createElement("button");
      card.type = "button";
      card.className = "ytm-card";
      const art = document.createElement("div");
      art.className = "ytm-card-art";
      // Cover collage: up to 4 track thumbnails (icons stand in for
      // missing artwork).
      const count = Math.min(4, pl.trackIds.length);
      for (let i = 0; i < count; i++) {
        const q = document.createElement("div");
        q.className = "ytm-card-collage-q";
        q.innerHTML = icon("library", 22);
        art.appendChild(q);
      }
      if (count === 0) art.innerHTML = icon("library", 48);
      const ct = document.createElement("div");
      ct.className = "ytm-card-title";
      ct.textContent = pl.name;
      const cs = document.createElement("div");
      cs.className = "ytm-card-sub";
      cs.textContent = `${pl.trackIds.length} track${pl.trackIds.length === 1 ? "" : "s"}`;
      void tracks;
      card.append(art, ct, cs);
      card.addEventListener("click", () => navigate(`playlist?id=${pl.id}`));
      grid.appendChild(card);
    }
  })();
}

function renderDetail(root: HTMLElement, playlistId: string) {
  root.innerHTML = `
    <div class="ytm-playlists-head">
      <button class="btn" id="pl-back" type="button">← Playlists</button>
      <h1 class="page-title" id="pl-title">Playlist</h1>
      <button class="btn" id="pl-play" type="button">▶ Play all</button>
      <button class="btn" id="pl-rename" type="button">Rename</button>
      <button class="btn" id="pl-delete" type="button">Delete</button>
    </div>
    <div class="ytm-tracklist" id="pl-tracks"></div>
    <p class="hint" id="pl-empty-tracks"></p>
  `;

  const tracksEl = root.querySelector<HTMLElement>("#pl-tracks")!;
  const emptyEl = root.querySelector<HTMLElement>("#pl-empty-tracks")!;

  let dragId: string | null = null;
  tracksEl.addEventListener("dragstart", (e) => {
    const row = (e.target as HTMLElement).closest<HTMLElement>(".ytm-track-row");
    dragId = row?.dataset.dragId ?? null;
  });
  tracksEl.addEventListener("dragover", (e) => e.preventDefault());
  tracksEl.addEventListener("drop", async (e) => {
    e.preventDefault();
    const row = (e.target as HTMLElement).closest<HTMLElement>(".ytm-track-row");
    if (!row || !dragId) return;
    const ids = [...tracksEl.querySelectorAll<HTMLElement>(".ytm-track-row")].map(
      (item) => item.dataset.dragId,
    );
    const toIndex = Math.max(0, ids.indexOf(row.dataset.dragId));
    const movingId = dragId;
    dragId = null;
    await reorderPlaylist(playlistId, movingId, toIndex);
    await draw();
  });

  async function draw() {
    const pl = await getPlaylist(playlistId);
    if (!pl) {
      root.innerHTML = `<p class="hint">Playlist not found.</p>`;
      return;
    }
    const title = root.querySelector<HTMLElement>("#pl-title");
    if (title) title.textContent = pl.name;
    tracksEl.innerHTML = "";
    const all = await getAllTracks();
    const entries = pl.trackIds
      .map((id) => all.find((t) => t.id === id))
      .filter((t): t is StoredTrack => Boolean(t));
    if (!entries.length) {
      emptyEl.textContent = "Empty — add tracks from the track ✕ menu on Home.";
      return;
    }
    emptyEl.textContent = "";
    trackRows(tracksEl, entries, {
      onPlay: (i) => {
        unlockAudio();
        void playPlaylist(playlistId, i);
      },
      onRemove: async (trackId) => {
        await removeFromPlaylist(playlistId, trackId);
        await draw();
      },
    });
  }

  root.querySelector<HTMLButtonElement>("#pl-back")?.addEventListener("click", () => {
    navigate("playlists");
  });
  root.querySelector<HTMLButtonElement>("#pl-play")?.addEventListener("click", () => {
    unlockAudio();
    void playPlaylist(playlistId, 0);
  });
  root.querySelector<HTMLButtonElement>("#pl-rename")?.addEventListener("click", async () => {
    const pl = await getPlaylist(playlistId);
    if (!pl) return;
    const name = window.prompt("New name", pl.name);
    if (name === null) return;
    await renamePlaylist(playlistId, name);
    await draw();
  });
  root.querySelector<HTMLButtonElement>("#pl-delete")?.addEventListener("click", async () => {
    if (!window.confirm("Delete this playlist?")) return;
    await deletePlaylist(playlistId);
    navigate("playlists");
  });

  void draw();
}

const pages: Record<string, Page> = {
  playlists: {
    id: "playlists",
    title: "Playlists",
    mount: renderList,
  },
  playlist: {
    id: "playlist",
    title: "Playlist",
    mount: (root) => {
      const id = new URLSearchParams(window.location.hash.split("?")[1] ?? "").get("id") ?? "";
      renderDetail(root, id);
    },
  },
};

export const playlistsPage = pages.playlists;
export const playlistDetailPage = pages.playlist;
