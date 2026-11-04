/** History page (spec 5.2): the full play log — recentPlays(200) from
 *  history-service, rendered as track rows. Title/artist are whatever
 *  the log kept (the source of truth is the honest record, not a
 *  re-read from the library); clicking a row plays the track if it is
 *  still in the session library, else the row stays inert. A positionS
 *  value renders "at 1:23" — the position the log actually kept, never
 *  an invented one. Clear all wipes the log through clearHistory(). */

import type { Page } from "../router";
import { recentPlays, clearHistory, type HistoryEntry } from "../history-service";
import { getWebTracks, playWebTracks } from "./home";
import { icon } from "../styles/ytm-icons";
import { trackArtworkUrl } from "../artwork-store";
import { setToast } from "../state";

/** Recent entries beyond the storage cap (50) would be a lie — the
 *  request clamps to what the log can hold. */
const HISTORY_LIMIT = 200;

function fmtPos(s: number): string {
  const sec = Math.floor(s % 60);
  return `${Math.floor(s / 60)}:${String(sec).padStart(2, "0")}`;
}

/** Row template: the ytm-track-row pattern (index / thumb / main / side).
 *  DOM API throughout — logged titles are user file names (XSS law). */
function makeRow(
  entry: HistoryEntry,
  index: number,
  track: ReturnType<typeof getWebTracks>[number] | undefined,
): HTMLElement {
  const row = document.createElement("div");
  row.className = "ytm-track-row";
  const idx = document.createElement("span");
  idx.className = "ytm-track-index";
  idx.textContent = String(index + 1);
  const thumb = document.createElement("div");
  thumb.className = "ytm-track-thumb";
  thumb.innerHTML = icon("library", 18); // trusted constant
  void trackArtworkUrl(entry.id).then((url) => {
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
  title.textContent = entry.title;
  const meta = document.createElement("div");
  meta.className = "ytm-track-meta";
  meta.textContent = entry.artist ?? "Unknown artist";
  main.append(title, meta);
  // Resume badge: the position the log kept, when it kept one.
  const badge = document.createElement("span");
  badge.className = "ytm-track-dur";
  if (entry.positionS != null && entry.positionS > 0) {
    badge.textContent = `at ${fmtPos(entry.positionS)}`;
  }
  row.append(idx, thumb, main, badge);
  if (track) {
    row.classList.add("playable");
    row.title = "Play";
    row.addEventListener("click", () => void playWebTracks([track], 0));
  } else {
    row.classList.add("gone");
    row.title = "Not in this session's library";
  }
  return row;
}

async function render(root: HTMLElement) {
  root.innerHTML = `
    <div class="ytm-shelf-head">
      <span class="ytm-shelf-title">History</span>
      <button class="ytm-chip" id="history-clear" type="button">Clear all</button>
    </div>
    <div class="ytm-tracklist" id="history-list"></div>
    <p class="hint" id="history-empty" hidden></p>
  `;
  const list = root.querySelector<HTMLElement>("#history-list")!;
  const empty = root.querySelector<HTMLElement>("#history-empty")!;
  const clearBtn = root.querySelector<HTMLButtonElement>("#history-clear")!;

  const entries = await recentPlays(HISTORY_LIMIT);
  if (!entries.length) {
    empty.hidden = false;
    empty.textContent = "Nothing played yet — history fills as you listen.";
    return;
  }
  const tracks = getWebTracks();
  entries.forEach((e, i) => {
    const track = tracks.find((t) => t.id === e.id);
    list.appendChild(makeRow(e, i, track));
  });

  clearBtn.addEventListener("click", async () => {
    await clearHistory();
    setToast("History cleared");
    void render(root);
  });
}

export const historyPage: Page = {
  id: "history",
  title: "History",
  mount: render,
};
