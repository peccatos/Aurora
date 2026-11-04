/** Queue page: list + drag-n-drop reorder + play/clear. */

import type { Page } from "../router";
import { app, playQueueIndex, setQueue, setToast } from "../state";

function render(root: HTMLElement) {
  root.innerHTML = `
    <section class="page">
      <header class="page-head">
        <div>
          <p class="overline">Playback order</p>
          <h1 class="page-title">Queue</h1>
        </div>
        <div class="page-actions">
          <span class="meta" id="queue-count"></span>
          <button class="btn" id="queue-clear" type="button">Clear</button>
        </div>
      </header>
      <ul class="track-list" id="queue-list"></ul>
      <p class="hint">Drag ⋮⋮ to reorder — the current track follows the track, not the position.</p>
    </section>
  `;
  update(root);

  root.querySelector<HTMLButtonElement>("#queue-clear")?.addEventListener("click", async () => {
    app.queueTracks = [];
    app.queueCurrent = null;
    try {
      await setQueue([]);
      setToast("Queue cleared.");
    } catch (e) {
      setToast(`Clear failed: ${String(e)}`);
    }
    update(root);
  });
}

function update(root: HTMLElement) {
  const ul = root.querySelector<HTMLUListElement>("#queue-list");
  const count = root.querySelector<HTMLElement>("#queue-count");
  if (count) count.textContent = `${app.queueTracks.length} tracks`;
  if (!ul) return;
  ul.innerHTML = "";

  app.queueTracks.forEach((t, idx) => {
    const li = document.createElement("li");
    li.className = "track-row" + (idx === app.queueCurrent ? " current" : "");
    li.dataset.idx = String(idx);

    const handle = document.createElement("span");
    handle.className = "drag-handle";
    handle.title = "Drag to reorder";
    handle.textContent = "⋮⋮";
    li.appendChild(handle);

    const idxEl = document.createElement("span");
    idxEl.className = "track-idx";
    idxEl.textContent = String(idx + 1).padStart(2, "0");
    li.appendChild(idxEl);

    const name = document.createElement("button");
    name.type = "button";
    name.className = "track-name";
    const stem = t.path.split(/[\\/]/).pop() ?? t.path;
    name.textContent = t.artist ? `${t.artist} — ${t.title ?? stem}` : (t.title ?? stem);
    name.addEventListener("click", () => {
      void playQueueIndex(idx).then(() => {
        setToast(`${idx + 1}/${app.queueTracks.length}`);
      });
    });

    const del = document.createElement("button");
    del.type = "button";
    del.className = "row-del";
    del.title = "Remove from queue";
    del.textContent = "×";
    del.addEventListener("click", async () => {
      app.queueTracks.splice(idx, 1);
      if (app.queueCurrent != null) {
        if (idx < app.queueCurrent) app.queueCurrent -= 1;
        else if (idx === app.queueCurrent)
          app.queueCurrent = Math.min(app.queueCurrent, app.queueTracks.length - 1);
      }
      await setQueue(app.queueTracks);
      update(root);
    });

    li.append(name, del);
    ul.appendChild(li);
  });

  attachDrag(ul, root);
}

function attachDrag(ul: HTMLUListElement, root: HTMLElement) {
  let from: number | null = null;
  let over: number | null = null;

  const rowAt = (y: number): HTMLElement | null => {
    const rows = Array.from(ul.querySelectorAll<HTMLElement>(".track-row"));
    return (
      rows.find((r) => {
        const rect = r.getBoundingClientRect();
        return y >= rect.top && y <= rect.bottom;
      }) ?? null
    );
  };

  ul.querySelectorAll<HTMLElement>(".drag-handle").forEach((handle) => {
    handle.addEventListener("pointerdown", (ev) => {
      const row = handle.closest<HTMLElement>(".track-row");
      if (!row) return;
      from = Number(row.dataset.idx);
      handle.setPointerCapture(ev.pointerId);
      row.style.opacity = "0.4";
      ev.preventDefault();
    });
    handle.addEventListener("pointermove", (ev) => {
      if (from == null) return;
      const target = rowAt(ev.clientY);
      ul.querySelectorAll<HTMLElement>(".track-row").forEach((r) =>
        r.classList.remove("drag-over"),
      );
      if (target) {
        over = Number(target.dataset.idx);
        target.classList.add("drag-over");
      } else {
        over = null;
      }
    });
    const finish = async (ev: PointerEvent) => {
      const row = handle.closest<HTMLElement>(".track-row");
      if (row) row.style.opacity = "";
      handle.releasePointerCapture?.(ev.pointerId);
      ul.querySelectorAll<HTMLElement>(".track-row").forEach((r) =>
        r.classList.remove("drag-over"),
      );
      if (from == null) return;
      const src = from;
      const to = over;
      from = null;
      over = null;
      if (to == null || to === src) return;
      const currentTrack =
        app.queueCurrent != null ? app.queueTracks[app.queueCurrent] : null;
      const [moved] = app.queueTracks.splice(src, 1);
      app.queueTracks.splice(to, 0, moved);
      if (currentTrack) {
        app.queueCurrent = app.queueTracks.findIndex(
          (t) => t.path === currentTrack.path && t.title === currentTrack.title,
        );
        if (app.queueCurrent < 0) app.queueCurrent = null;
      }
      try {
        await setQueue(app.queueTracks);
        setToast(`Moved to position ${to + 1}.`);
      } catch (e) {
        setToast(`Reorder failed: ${String(e)}`);
      }
      update(root);
    };
    handle.addEventListener("pointerup", finish);
    handle.addEventListener("pointercancel", finish);
  });
}

export const queuePage: Page = {
  id: "queue",
  title: "Queue",
  mount: render,
  update,
};
