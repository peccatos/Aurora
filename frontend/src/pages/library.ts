/** Library page: search, track list, playlists column, actions. */

import { invoke } from "../invoke";
const open = (...a: Parameters<typeof import("@tauri-apps/plugin-dialog")["open"]>) =>
  import("@tauri-apps/plugin-dialog").then((m) => m.open(...a));
const save = (...a: Parameters<typeof import("@tauri-apps/plugin-dialog")["save"]>) =>
  import("@tauri-apps/plugin-dialog").then((m) => m.save(...a));
import type { Page } from "../router";
import { app, refreshLibraryStatus, setQueue, setToast, trackToQueueTrack } from "../state";
import type { PlaylistDto, ScanSummary, TrackDto } from "../types";

let searchTimer: ReturnType<typeof setTimeout> | null = null;
let activePlaylist: string | null = null;
let playlists: PlaylistDto[] = [];

async function loadTracks(query?: string) {
  try {
    const tracks = query
      ? await invoke<TrackDto[]>("search_library", { query })
      : await invoke<TrackDto[]>("list_tracks");
    app.library.tracks = tracks;
  } catch (e) {
    setToast(String(e));
    app.library.tracks = [];
  }
}

async function loadPlaylists() {
  try {
    playlists = await invoke<PlaylistDto[]>("list_playlists");
  } catch {
    playlists = [];
  }
}

async function playNow(tracks: TrackDto[]) {
  if (!tracks.length) return;
  app.queueTracks = tracks.map(trackToQueueTrack);
  app.queueCurrent = 0;
  app.selectedPath = app.queueTracks[0].path;
  try {
    await setQueue(app.queueTracks);
    await invoke("play_queue_index", { index: 0 });
    setToast(tracks[0].display);
  } catch (e) {
    setToast(`Play failed: ${String(e)}`);
  }
}

function render(root: HTMLElement) {
  root.innerHTML = `
    <section class="page">
      <header class="page-head">
        <div>
          <p class="overline">Collection</p>
          <h1 class="page-title">Library</h1>
        </div>
        <div class="page-actions">
          <input id="lib-search" type="search" placeholder="Search title, artist, album…" />
          <button class="btn" id="lib-open" type="button">Open…</button>
          <button class="btn" id="lib-scan" type="button">Scan folder…</button>
          <button class="btn" id="lib-refresh" type="button">Rescan</button>
        </div>
      </header>

      <div class="lib-cols">
        <aside class="playlists-col">
          <div class="col-head">
            <span>Playlists</span>
            <span class="col-head-actions">
              <button id="pl-import" class="icon-btn" type="button"
                title="Import playlist (M3U, PLS, Spotify JSON, CSV)">↓</button>
              <button id="pl-export" class="icon-btn" type="button"
                title="Export active playlist as M3U8" disabled>↑</button>
              <button id="pl-new" class="icon-btn" type="button" title="New playlist">+</button>
            </span>
          </div>
          <ul id="playlist-list" class="playlist-list"></ul>
        </aside>

        <div class="tracks-col">
          <div class="lib-actions">
            <button class="btn" id="lib-play-sel" type="button" disabled>▶ Play</button>
            <button class="btn" id="lib-queue-sel" type="button" disabled>+ Queue</button>
            <button class="btn" id="lib-add-pl" type="button" disabled>→ Playlist</button>
            <span class="meta" id="lib-count"></span>
          </div>
          <ul id="lib-tracks" class="track-list"></ul>
          <p class="hint" id="lib-note"></p>
        </div>
      </div>
    </section>
  `;

  // --- wiring ---
  root.querySelector<HTMLButtonElement>("lib-open")?.addEventListener;
  root.querySelector<HTMLButtonElement>("#lib-open")?.addEventListener("click", async () => {
    const picked = await open({
      multiple: false,
      filters: [{ name: "Aurora library", extensions: ["db"] }],
    });
    const path = typeof picked === "string" ? picked : null;
    if (!path) return;
    try {
      const st = await invoke<{ open: boolean; path: string | null }>("open_library", { path });
      setToast(st.open ? `Library: ${st.path}` : "Failed to open library");
      await refreshLibraryStatus();
      await loadTracks();
      await loadPlaylists();
      update(root);
    } catch (e) {
      setToast(String(e));
    }
  });

  root.querySelector<HTMLButtonElement>("#lib-scan")?.addEventListener("click", async () => {
    if (!app.library.open) {
      setToast("Open a library (.db) first.");
      return;
    }
    const picked = await open({ multiple: false, directory: true });
    const rootDir = typeof picked === "string" ? picked : null;
    if (!rootDir) return;
    setToast("Scanning…");
    try {
      const s = await invoke<ScanSummary>("scan_folder", { root: rootDir });
      setToast(
        `Scanned: ${s.discovered} found, ${s.added} added, ${s.updated} updated, ${s.pruned} removed` +
          (s.failed ? `, ${s.failed} failed` : ""),
      );
      await refreshLibraryStatus();
      await loadTracks();
      update(root);
    } catch (e) {
      setToast(String(e));
    }
  });

  root.querySelector<HTMLButtonElement>("#lib-refresh")?.addEventListener("click", async () => {
    if (!app.library.open) {
      setToast("No library open.");
      return;
    }
    setToast("Rescanning known roots…");
    try {
      const s = await invoke<ScanSummary>("rescan_all");
      setToast(
        `Rescan: ${s.discovered} found, ${s.added} added, ${s.updated} updated, ${s.pruned} removed`,
      );
      await refreshLibraryStatus();
      await loadTracks();
      update(root);
    } catch (e) {
      setToast(String(e));
    }
  });

  root.querySelector<HTMLInputElement>("#lib-search")?.addEventListener("input", (e) => {
    const q = (e.target as HTMLInputElement).value.trim();
    if (searchTimer) clearTimeout(searchTimer);
    searchTimer = setTimeout(() => {
      if (!app.library.open) {
        setToast("Open a library first.");
        return;
      }
      void loadTracks(q || undefined).then(() => update(root));
    }, 250);
  });

  root.querySelector<HTMLButtonElement>("#lib-play-sel")?.addEventListener("click", () => {
    const sel = app.library.tracks.filter((t) => app.library.selectedIds.has(t.id));
    void playNow(sel);
  });

  root.querySelector<HTMLButtonElement>("#lib-queue-sel")?.addEventListener("click", async () => {
    const sel = app.library.tracks.filter((t) => app.library.selectedIds.has(t.id));
    if (!sel.length) return;
    try {
      const q = await invoke<{ tracks: typeof app.queueTracks; current: number | null }>(
        "queue_add_tracks",
        { tracks: sel.map(trackToQueueTrack) },
      );
      app.queueTracks = q.tracks;
      app.queueCurrent = q.current;
      setToast(`Added ${sel.length} to queue (now ${q.tracks.length}).`);
      update(root);
    } catch (e) {
      setToast(`Queue add failed: ${String(e)}`);
    }
  });

  root.querySelector<HTMLButtonElement>("#lib-add-pl")?.addEventListener("click", async () => {
    const sel = app.library.tracks.filter((t) => app.library.selectedIds.has(t.id));
    if (!sel.length || !activePlaylist) return;
    try {
      await invoke("set_playlist_tracks", {
        playlistId: activePlaylist,
        trackIds: sel.map((t) => t.id),
      });
      setToast(`Playlist updated (${sel.length} tracks).`);
      await loadPlaylists();
      update(root);
    } catch (e) {
      setToast(String(e));
    }
  });

  root.querySelector<HTMLButtonElement>("#pl-new")?.addEventListener("click", async () => {
    if (!app.library.open) return;
    const name = prompt("Playlist name");
    if (!name) return;
    try {
      await invoke("create_playlist", { name });
      await loadPlaylists();
      update(root);
    } catch (e) {
      setToast(String(e));
    }
  });

  // Import: M3U/M3U8/PLS (paths) or Spotify JSON / CSV (title+artist
  // matched against the local library). Honest report: matched and
  // unmatched counts, never silent fabrication.
  root.querySelector<HTMLButtonElement>("#pl-import")?.addEventListener("click", async () => {
    if (!app.library.open) {
      setToast("Open a library first");
      return;
    }
    const picked = await open({
      multiple: false,
      filters: [
        {
          name: "Playlists",
          extensions: ["m3u", "m3u8", "pls", "json", "csv", "txt"],
        },
      ],
    });
    const path = typeof picked === "string" ? picked : null;
    if (!path) return;
    setToast("Importing…");
    try {
      const report = await invoke<{
        playlist_id: string;
        playlist_name: string;
        matched: number;
        unmatched: number;
        resolved_by_scan: boolean;
        unmatched_names: string[];
      }>("import_playlist", { path });
      const scanNote = report.resolved_by_scan ? " (new files scanned in)" : "";
      setToast(
        `Imported "${report.playlist_name}": ${report.matched} matched${scanNote}, ` +
          (report.unmatched > 0
            ? `${report.unmatched} not found — ${report.unmatched_names.slice(0, 3).join("; ")}…`
            : "all found"),
      );
      activePlaylist = report.playlist_id;
      await loadPlaylists();
      await loadTracks();
      update(root);
    } catch (e) {
      setToast(`Import failed: ${String(e)}`);
    }
  });

  // Export the active playlist as M3U8 (universal format).
  root.querySelector<HTMLButtonElement>("#pl-export")?.addEventListener("click", async () => {
    if (!activePlaylist) return;
    const pl = playlists.find((p) => p.id === activePlaylist);
    if (!pl) return;
    const dest = await save({
      defaultPath: `${pl.name.replace(/[\\/:*?"<>|]/g, "_")}.m3u8`,
      filters: [{ name: "M3U8 playlist", extensions: ["m3u8"] }],
    });
    if (!dest) return;
    try {
      await invoke("export_playlist", { playlistId: pl.id, dest });
      setToast(`Exported "${pl.name}" (${pl.track_count} tracks)`);
    } catch (e) {
      setToast(`Export failed: ${String(e)}`);
    }
  });

  // Boot load.
  void (async () => {
    await refreshLibraryStatus();
    if (app.library.open) {
      await loadTracks();
      await loadPlaylists();
    }
    update(root);
    // Background auto-open poll (backend opens the library async).
    let tries = 0;
    const timer = setInterval(async () => {
      tries += 1;
      const st = await refreshLibraryStatus();
      if (st.open) {
        await loadTracks();
        await loadPlaylists();
        update(root);
        clearInterval(timer);
      } else if (tries > 40) {
        clearInterval(timer);
      }
    }, 500);
  })();
}

function update(root: HTMLElement) {
  const list = root.querySelector<HTMLUListElement>("#lib-tracks");
  const count = root.querySelector<HTMLElement>("#lib-count");
  const note = root.querySelector<HTMLElement>("#lib-note");
  const playBtn = root.querySelector<HTMLButtonElement>("#lib-play-sel");
  const queueBtn = root.querySelector<HTMLButtonElement>("#lib-queue-sel");
  const plBtn = root.querySelector<HTMLButtonElement>("#lib-add-pl");

  const has = app.library.selectedIds.size > 0;
  if (playBtn) playBtn.disabled = !has;
  if (queueBtn) queueBtn.disabled = !has;
  if (plBtn) plBtn.disabled = !has || !activePlaylist;
  const exportBtn = root.querySelector<HTMLButtonElement>("#pl-export");
  if (exportBtn) exportBtn.disabled = !activePlaylist;
  if (count) count.textContent = app.library.open ? `${app.library.tracks.length} tracks` : "";
  if (note)
    note.textContent = app.library.open
      ? ""
      : "No library open — Open… or Scan folder… to begin.";

  if (list) {
    list.innerHTML = "";
    for (const t of app.library.tracks) {
      const li = document.createElement("li");
      li.className = "track-row";
      li.dataset.id = t.id;

      const check = document.createElement("input");
      check.type = "checkbox";
      check.checked = app.library.selectedIds.has(t.id);
      check.addEventListener("change", () => {
        if (check.checked) app.library.selectedIds.add(t.id);
        else app.library.selectedIds.delete(t.id);
        update(root);
      });

      const name = document.createElement("button");
      name.type = "button";
      name.className = "track-name";
      name.textContent = t.display;
      name.title = t.path;
      name.addEventListener("dblclick", () => playNow([t]));

      const meta = document.createElement("span");
      meta.className = "track-meta";
      meta.textContent = [t.album, t.year ? String(t.year) : null, t.format]
        .filter(Boolean)
        .join(" · ");

      const dur = document.createElement("span");
      dur.className = "track-dur";
      dur.textContent = t.duration_display;

      li.append(check, name, meta, dur);
      list.appendChild(li);
    }
  }

  // Playlists column.
  const plUl = root.querySelector<HTMLUListElement>("#playlist-list");
  if (plUl) {
    plUl.innerHTML = "";
    for (const pl of playlists) {
      const li = document.createElement("li");
      li.className = "pl-row" + (pl.id === activePlaylist ? " active" : "");
      const name = document.createElement("button");
      name.type = "button";
      name.className = "pl-name";
      name.textContent = `${pl.name} · ${pl.track_count}`;
      name.addEventListener("click", async () => {
        activePlaylist = pl.id;
        try {
          const tracks = await invoke<TrackDto[]>("playlist_tracks", { playlistId: pl.id });
          app.library.tracks = tracks;
          update(root);
        } catch (e) {
          setToast(String(e));
        }
      });
      const del = document.createElement("button");
      del.type = "button";
      del.className = "row-del";
      del.title = "Delete playlist";
      del.textContent = "×";
      del.addEventListener("click", async () => {
        try {
          await invoke("delete_playlist", { playlistId: pl.id });
          if (activePlaylist === pl.id) {
            activePlaylist = null;
            await loadTracks();
          }
          await loadPlaylists();
          update(root);
        } catch (e) {
          setToast(String(e));
        }
      });
      li.append(name, del);
      plUl.appendChild(li);
    }
  }
}

export const libraryPage: Page = {
  id: "library",
  title: "Library",
  mount: render,
  update,
};
