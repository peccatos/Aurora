/** Browser-only genre explorer for locally stored tracks. */

import type { Page } from "../router";
import { getAllTracks, type StoredTrack } from "../library-db";

interface GenreGroup {
  name: string;
  key: string;
  tracks: StoredTrack[];
}

/** Collapse only case and whitespace differences; do not merge subgenres. */
export function groupTracksByGenre(tracks: readonly StoredTrack[]): GenreGroup[] {
  const groups = new Map<string, GenreGroup>();
  for (const track of tracks) {
    const name = track.genre?.trim().replace(/\s+/g, " ");
    if (!name) continue;
    const key = name.toLocaleLowerCase();
    const group = groups.get(key) ?? { name, key, tracks: [] };
    group.tracks.push(track);
    groups.set(key, group);
  }
  return [...groups.values()].sort((a, b) => a.name.localeCompare(b.name));
}

function renderTrackList(root: HTMLElement, tracks: StoredTrack[], genre: string) {
  root.innerHTML = "";
  const head = document.createElement("div");
  head.className = "ytm-shelf-head";
  const title = document.createElement("h2");
  title.className = "ytm-shelf-title";
  title.textContent = genre;
  const back = document.createElement("button");
  back.type = "button";
  back.className = "ytm-chip";
  back.textContent = "All genres";
  back.addEventListener("click", () => renderGenres(root, tracks));
  head.append(title, back);
  root.appendChild(head);

  const list = document.createElement("div");
  list.className = "ytm-tracklist";
  for (const track of tracks) {
    const row = document.createElement("div");
    row.className = "ytm-track-row";
    const name = document.createElement("div");
    name.className = "ytm-track-title";
    name.textContent = track.name;
    const artist = document.createElement("div");
    artist.className = "ytm-track-meta";
    artist.textContent = track.artist || "Unknown artist";
    row.append(name, artist);
    list.appendChild(row);
  }
  root.appendChild(list);
}

function renderGenres(root: HTMLElement, tracks: StoredTrack[]) {
  root.innerHTML = "";
  const heading = document.createElement("h1");
  heading.className = "page-title";
  heading.textContent = "Genres";
  const hint = document.createElement("p");
  hint.className = "hint";
  hint.textContent = "Genres are read from your local track tags and metadata. No online guesses are applied.";
  root.append(heading, hint);

  const groups = groupTracksByGenre(tracks);
  if (!groups.length) {
    const empty = document.createElement("p");
    empty.className = "hint";
    empty.textContent = tracks.length
      ? "No genre tags found yet. Add a genre in a track's metadata to see it here."
      : "Your browser library is empty. Add audio files on Home to browse genres.";
    root.appendChild(empty);
    return;
  }

  const cards = document.createElement("div");
  cards.className = "ytm-shelf-row";
  for (const group of groups) {
    const card = document.createElement("button");
    card.type = "button";
    card.className = "ytm-card genre-card";
    const name = document.createElement("span");
    name.className = "ytm-card-title";
    name.textContent = group.name;
    const count = document.createElement("span");
    count.className = "ytm-card-sub";
    count.textContent = `${group.tracks.length} ${group.tracks.length === 1 ? "track" : "tracks"}`;
    card.append(name, count);
    card.addEventListener("click", () => renderTrackList(root, group.tracks, group.name));
    cards.appendChild(card);
  }
  root.appendChild(cards);
}

async function mount(root: HTMLElement) {
  root.innerHTML = `<section class="page"><p class="hint">Loading genres…</p></section>`;
  try {
    renderGenres(root, await getAllTracks());
  } catch (error) {
    root.innerHTML = "";
    const message = document.createElement("p");
    message.className = "hint err";
    message.textContent = `Could not load browser library: ${String(error)}`;
    root.appendChild(message);
  }
}

export const genresPage: Page = {
  id: "genres",
  title: "Genres",
  mount,
};
