/** Local artist page for the web shell. */

import type { Page } from "../router";
import { navigate } from "../router";
import { getWebTracks, playWebTracks, type WebTrack } from "./home";
import { artistRouteName, tracksForArtist } from "../artist-model";
import { icon } from "../styles/ytm-icons";

function renderTrackList(root: HTMLElement, tracks: WebTrack[]): void {
  const list = root.querySelector<HTMLElement>("#artist-tracks");
  if (!list) return;
  list.replaceChildren();
  tracks.forEach((track, index) => {
    const row = document.createElement("button");
    row.type = "button";
    row.className = "ytm-track-row artist-track-row";
    const number = document.createElement("span");
    number.className = "ytm-track-index";
    number.textContent = String(index + 1);
    const thumb = document.createElement("span");
    thumb.className = "ytm-track-thumb";
    thumb.innerHTML = icon("library", 18);
    const main = document.createElement("span");
    main.className = "ytm-track-main";
    const title = document.createElement("span");
    title.className = "ytm-track-title";
    title.textContent = track.name;
    const meta = document.createElement("span");
    meta.className = "ytm-track-meta";
    meta.textContent = track.album ?? "";
    main.append(title, meta);
    row.append(number, thumb, main);
    row.addEventListener("click", () => void playWebTracks(tracks, index));
    list.appendChild(row);
  });
}

function render(root: HTMLElement): void {
  const artist = artistRouteName(window.location.hash);
  root.innerHTML = `
    <section class="ytm-artist-page">
      <button class="btn" id="artist-back" type="button">← Назад</button>
      <header class="ytm-artist-hero">
        <div class="ytm-card-art round artist-avatar">${icon("library", 56)}</div>
        <div>
          <p class="overline">Исполнитель</p>
          <h1 class="page-title" id="artist-name"></h1>
          <p class="hint" id="artist-count"></p>
        </div>
      </header>
      <div class="ytm-artist-actions">
        <button class="btn" id="artist-play" type="button">▶ Воспроизвести всё</button>
        <button class="btn" id="artist-search" type="button">Искать онлайн</button>
      </div>
      <section class="ytm-shelf">
        <div class="ytm-shelf-head"><h2 class="ytm-shelf-title">Треки</h2></div>
        <div class="ytm-tracklist" id="artist-tracks"></div>
        <p class="hint" id="artist-empty"></p>
      </section>
    </section>
  `;
  const name = root.querySelector<HTMLElement>("#artist-name")!;
  name.textContent = artist ?? "Исполнитель не выбран";
  const tracks = artist ? tracksForArtist(getWebTracks(), artist) : [];
  root.querySelector<HTMLElement>("#artist-count")!.textContent = `${tracks.length} ${tracks.length === 1 ? "трек" : "треков"} в библиотеке`;
  renderTrackList(root, tracks);
  if (!tracks.length) root.querySelector<HTMLElement>("#artist-empty")!.textContent = "В локальной библиотеке пока нет треков этого исполнителя.";
  root.querySelector<HTMLButtonElement>("#artist-back")?.addEventListener("click", () => navigate("search"));
  root.querySelector<HTMLButtonElement>("#artist-play")?.addEventListener("click", () => void playWebTracks(tracks));
  root.querySelector<HTMLButtonElement>("#artist-search")?.addEventListener("click", () => navigate(`search?q=${encodeURIComponent(artist ?? "")}`));
}

export const artistPage: Page = { id: "artist", title: "Artist", mount: render };
