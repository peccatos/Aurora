/** Now Playing fullscreen (YTM-style): big art, blurred-art background,
 *  tabs — Lyrics (synced, LRCLIB) / Up next / Signal path (honest ADR
 *  0002 report). Waveform seek, spectrum, A/B loop stay. */

import { invoke } from "../invoke";
import type { Page } from "../router";
import { app, setToast } from "../state";
import { invalidateArtwork, renderArtwork } from "../artwork";
import { updateWebTrackArtwork } from "../library-db";
import { invalidateTrackArtworkUrl } from "../artwork-store";
import type { PlaybackStatus, TrackDto } from "../types";
import { saveTrackEnrichment } from "../track-enrichment-service";
import { getWebTrackByPath } from "./home";
import { validateVideoUrl } from "../video-url";
import {
  getLyrics,
  activeLineIndex,
  lyricsToText,
  parseLyricsText,
  type LyricsResult,
} from "../lyrics-service";
import {
  translateLyrics,
  lyricsToTranslateLines,
  translationEnabled,
  TRANSLATION_LANGUAGES,
} from "../translation-service";

const SIGNAL_FIELDS: Array<[string, (s: PlaybackStatus | null) => string]> = [
  ["state", (s) => s?.state ?? "idle"],
  ["sample rate", (s) => (s?.sample_rate ? `${(s.sample_rate / 1000).toFixed(1)} kHz` : "—")],
  ["channels", (s) => (s?.channels ? String(s.channels) : "—")],
  ["bit depth", (s) => (s?.bits_per_sample ? `${s.bits_per_sample}-bit` : "—")],
  ["transform", (s) => (s?.transformations.length ? s.transformations.join(", ") : "none")],
  ["integrity", (s) => s?.integrity ?? "unverified"],
  ["frames", (s) => (s?.frames_played ?? 0).toLocaleString()],
  ["underruns", (s) => (s?.underruns ?? 0).toLocaleString()],
];

const WAVE_BINS = 160;

/** Waveform cache per track path. */
const waveCache = new Map<string, number[]>();
let waveBars: number[] | null = null;
let loopRegion: { start: number; end: number } | null = null;

/** Lyrics state: loaded for one (artist, title) pair; auto-scroll. */
let lyrics: LyricsResult | null = null;
let lyricsKey: string | null = null;
let lyricsState: "loading" | "none" | "ready" = "loading";
let lyricsAutoScroll = true;
let lyricsUserScrollUntil = 0;
let lyricsEditText = "";
let lyricsEditTranslation = "";
let lyricsEditTrackId: string | null = null;

async function loadWave(path: string) {
  if (waveCache.has(path)) {
    waveBars = waveCache.get(path) ?? null;
    return;
  }
  try {
    const peaks = await invoke<number[]>("waveform", { path, bins: WAVE_BINS });
    waveCache.set(path, peaks);
    waveBars = peaks;
  } catch {
    waveBars = null;
  }
}

function fmtTime(s: number): string {
  if (!Number.isFinite(s) || s < 0) return "0:00";
  const m = Math.floor(s / 60);
  const sec = Math.floor(s % 60);
  return `${m}:${sec.toString().padStart(2, "0")}`;
}

function currentLibraryTrack(): TrackDto | null {
  const path = app.queueCurrent != null
    ? app.queueTracks[app.queueCurrent]?.path
    : app.selectedPath;
  const desktop = app.library.tracks.find((track) => track.path === path);
  if (desktop) return desktop;
  const web = path ? getWebTrackByPath(path) : null;
  if (!web) return null;
  return {
    id: web.id,
    path: `web://${web.id}`,
    title: web.name,
    artist: web.artist,
    album: web.album,
    genre: web.genre,
    year: web.year,
    track_number: null,
    format: web.extension,
    duration_ms: web.durationMs,
    replaygain_track_db: null,
    replaygain_album_db: null,
    bpm: null,
    lyrics: web.lyrics,
    lyrics_language: web.lyrics_language,
    lyrics_translation: web.lyrics_translation,
    lyrics_translation_language: web.lyrics_translation_language,
    artwork_data_url: web.artwork_data_url,
    source_url: web.source_url,
    source_provider: web.source_provider,
    video_url: web.video_url,
    metadata_source: web.metadata_source,
    display: web.name,
    duration_display: "",
  };
}

function resetLyricsEditor(track: TrackDto | null) {
  if (track?.id === lyricsEditTrackId) return;
  lyricsEditTrackId = track?.id ?? null;
  lyricsEditText = track?.lyrics ?? (lyrics ? lyricsToText(lyrics) : "");
  lyricsEditTranslation = track?.lyrics_translation ?? "";
}

async function loadLyrics(root: HTMLElement, artist: string | null, title: string | null) {
  const stored = currentLibraryTrack();
  resetLyricsEditor(stored);
  const key = stored?.id ?? `${artist ?? ""}|${title ?? ""}`;
  if (key === lyricsKey) return;
  lyricsKey = key;
  if (stored?.lyrics) {
    lyrics = null;
    lyricsState = "ready";
    drawLyrics(root);
    return;
  }
  if (!title) {
    lyrics = null;
    lyricsState = "none";
    drawLyrics(root);
    return;
  }
  lyrics = null;
  lyricsState = "loading";
  drawLyrics(root);
  const res = await getLyrics(artist, title, stored?.duration_ms != null ? stored.duration_ms / 1000 : null);
  if (lyricsKey !== key) return; // track changed mid-fetch
  lyrics = res;
  lyricsState = res ? "ready" : "none";
  if (res && stored && !stored.lyrics) lyricsEditText = lyricsToText(res);
  drawLyrics(root);
}

function drawLyrics(root: HTMLElement) {
  const box = root.querySelector<HTMLElement>("#np-lyrics");
  if (!box) return;
  const stored = currentLibraryTrack();
  resetLyricsEditor(stored);
  box.innerHTML = "";
  const displayLyrics = stored?.lyrics
    ? parseStoredLyrics(stored.lyrics)
    : lyrics?.lines ?? [];
  // Translation lines aligned to displayLyrics when a stored translation
  // exists (same line count expected; mismatch = honest skip).
  const translationLines = stored?.lyrics_translation
    ? parseStoredLyrics(stored.lyrics_translation).map((l) => l.text)
    : [];
  const showTranslation = translationLines.length === displayLyrics.length && translationLines.length > 0;
  if (lyricsState === "loading" && !stored?.lyrics) {
    const p = document.createElement("p");
    p.className = "np-lyrics-hint";
    p.textContent = "Searching lyrics…";
    box.appendChild(p);
  } else if (!displayLyrics.length) {
    const p = document.createElement("p");
    p.className = "np-lyrics-hint";
    p.textContent = "No lyrics found — enter text below to add one.";
    box.appendChild(p);
  } else {
    displayLyrics.forEach((line, i) => {
      const el = document.createElement("p");
      el.className = "np-lyrics-line";
      el.dataset.lineIdx = String(i);
      el.textContent = line.text || "♪";
      box.appendChild(el);
      if (showTranslation && translationLines[i]) {
        const tr = document.createElement("p");
        tr.className = "np-lyrics-line-translation";
        tr.dataset.lineIdx = String(i);
        tr.textContent = translationLines[i];
        box.appendChild(tr);
      }
    });
    const src = document.createElement("p");
    src.className = "np-lyrics-source";
    src.textContent = stored?.lyrics ? "Saved in Aurora library" : `${lyrics?.synced ? "Synced" : "Plain"} lyrics · LRCLIB · ${lyrics?.trackName ?? ""}`;
    box.appendChild(src);
  }

  const editor = document.createElement("div");
  editor.className = "np-lyrics-editor";
  editor.innerHTML = `<label>Текст песни<textarea id="lyrics-edit-text" rows="5"></textarea></label><label>Перевод<textarea id="lyrics-edit-translation" rows="4"></textarea></label><button class="btn" id="lyrics-save" type="button">Сохранить текст</button><div class="np-lyrics-translate-row"><label>Перевести<select id="lyrics-translate-lang">${TRANSLATION_LANGUAGES.map((l) => `<option value="${l.code}"${l.code === "ru" ? " selected" : ""}>${l.label}</option>`).join("")}</select></label><button class="btn" id="lyrics-translate" type="button" ${stored?.lyrics && translationEnabled() ? "" : "disabled"}>Перевести</button><p class="np-lyrics-hint" id="lyrics-translate-hint">${translationEnabled() ? "" : "Перевод выключен — включите в настройках"}</p></div><p class="np-lyrics-hint" id="lyrics-save-hint"></p>`;
  const text = editor.querySelector<HTMLTextAreaElement>("#lyrics-edit-text");
  const translation = editor.querySelector<HTMLTextAreaElement>("#lyrics-edit-translation");
  if (text) text.value = lyricsEditText;
  if (translation) translation.value = lyricsEditTranslation;

  // Translate the current lyrics into the picked language. The text goes
  // to an external service (user opted in); the result is persisted via
  // the enrichment path so the work is done once.
  editor.querySelector<HTMLButtonElement>("#lyrics-translate")?.addEventListener("click", async () => {
    const track = currentLibraryTrack();
    const button = editor.querySelector<HTMLButtonElement>("#lyrics-translate");
    const hint = editor.querySelector<HTMLElement>("#lyrics-translate-hint");
    const lang = editor.querySelector<HTMLSelectElement>("#lyrics-translate-lang")?.value ?? "ru";
    const sourceText = text?.value.trim() ?? "";
    if (!track) {
      if (hint) hint.textContent = "Перевод доступен для треков открытой библиотеки";
      return;
    }
    if (!sourceText) {
      if (hint) hint.textContent = "Сначала введите или найдите текст песни";
      return;
    }
    if (button) button.disabled = true;
    if (hint) hint.textContent = "Перевожу… текст уходит во внешний сервис";
    try {
      const lines = lyricsToTranslateLines(sourceText);
      const translated = await translateLyrics(lines, lang);
      const translatedText = translated.join("\n");
      await saveTrackEnrichment({
        track_id: track.id,
        lyrics_translation: translatedText,
        lyrics_translation_language: lang,
      });
      track.lyrics_translation = translatedText;
      track.lyrics_translation_language = lang;
      lyricsEditTranslation = translatedText;
      if (translation) translation.value = translatedText;
      const webTrack = getWebTrackByPath(track.path);
      if (webTrack) {
        webTrack.lyrics_translation = translatedText;
        webTrack.lyrics_translation_language = lang;
      }
      if (hint) hint.textContent = "Перевод сохранён";
      drawLyrics(root);
      window.dispatchEvent(new CustomEvent("aurora:web-track-metadata-updated", { detail: { id: track.id } }));
    } catch (error) {
      if (hint) hint.textContent = error instanceof Error ? error.message : "Не удалось перевести";
    } finally {
      if (button) button.disabled = false;
    }
  });

  editor.querySelector<HTMLButtonElement>("#lyrics-save")?.addEventListener("click", async () => {
    if (!stored) {
      setToast("Сохранение доступно для треков открытой библиотеки");
      return;
    }
    lyricsEditText = text?.value ?? "";
    lyricsEditTranslation = translation?.value ?? "";
    const button = editor.querySelector<HTMLButtonElement>("#lyrics-save");
    const hint = editor.querySelector<HTMLElement>("#lyrics-save-hint");
    if (button) button.disabled = true;
    try {
      await saveTrackEnrichment({
        track_id: stored.id,
        lyrics: lyricsEditText || null,
        lyrics_language: stored.lyrics_language,
        lyrics_translation: lyricsEditTranslation || null,
        lyrics_translation_language: stored.lyrics_translation_language,
      });
      stored.lyrics = lyricsEditText || null;
      stored.lyrics_translation = lyricsEditTranslation || null;
      const webTrack = getWebTrackByPath(stored.path);
      if (webTrack) {
        webTrack.lyrics = stored.lyrics;
        webTrack.lyrics_translation = stored.lyrics_translation;
      }
      if (hint) hint.textContent = "Сохранено";
      window.dispatchEvent(new CustomEvent("aurora:web-track-metadata-updated", { detail: { id: stored.id } }));
    } catch (error) {
      if (hint) hint.textContent = error instanceof Error ? error.message : "Не удалось сохранить";
    } finally {
      if (button) button.disabled = false;
    }
  });
  box.appendChild(editor);
}

function parseStoredLyrics(text: string): Array<{ timeS: number | null; text: string }> {
  return text.includes("[")
    ? parseLyricsText(text)
    : text.split(/\r?\n/).map((line) => ({ timeS: null, text: line })).filter((line) => line.text);
}

function scrollLyrics(root: HTMLElement, posS: number) {
  const box = root.querySelector<HTMLElement>("#np-lyrics");
  if (!box) return;
  // Sync source: stored (sidecar/user) lyrics when present, else LRCLIB.
  const stored = currentLibraryTrack();
  const lines = stored?.lyrics
    ? parseStoredLyrics(stored.lyrics)
    : lyrics?.lines ?? null;
  if (!lines || !lines.some((l) => l.timeS != null)) return;
  const idx = activeLineIndex(lines, posS);
  box.querySelectorAll<HTMLElement>(".np-lyrics-line, .np-lyrics-line-translation").forEach((el) => {
    el.classList.toggle("active", Number(el.dataset.lineIdx) === idx);
  });
  if (!lyricsAutoScroll) return;
  if (Date.now() < lyricsUserScrollUntil) return; // user is reading
  const active = box.querySelector<HTMLElement>(".np-lyrics-line.active");
  if (active) {
    box.scrollTo({ top: active.offsetTop - box.clientHeight / 2 + active.clientHeight / 2, behavior: "smooth" });
  }
}

function render(root: HTMLElement) {
  root.innerHTML = `
    <div class="np-page" id="np-page">
      <div class="np-bg" id="np-bg" aria-hidden="true"></div>
      <div class="np-close">
        <button class="btn" id="np-close-btn" type="button" title="Назад">✕ Закрыть</button>
      </div>
      <div class="np-main">
        <div class="np-av-toggle" id="np-av-toggle" aria-label="Режим воспроизведения">
          <button class="np-av-btn active" data-av="song" type="button">Трек</button>
          <button class="np-av-btn" data-av="video" type="button" disabled title="Видео недоступно">Видео</button>
        </div>
        <div class="np-art" id="np-art"><span class="art-glyph">♪</span></div>
        <label class="np-cover-picker" id="np-cover-picker" hidden>Изменить обложку<input id="track-cover-file" type="file" accept="image/jpeg,image/png,image/webp,image/gif" /></label>
        <p class="np-lyrics-hint" id="track-cover-hint" role="status" aria-live="polite"></p>
      </div>
      <div class="np-side">
        <p class="overline">Сейчас играет</p>
        <h1 class="np-title" id="np-title">Ничего не играет</h1>
        <p class="np-artist" id="np-artist">Откройте файл или выберите из библиотеки</p>
        <section class="np-metadata-editor" id="np-web-metadata" hidden>
          <label>Название<input id="track-meta-title" type="text" maxlength="300" /></label>
          <label>Исполнитель<input id="track-meta-artist" type="text" maxlength="300" /></label>
          <label>Альбом<input id="track-meta-album" type="text" maxlength="300" /></label>
          <label>Жанр<input id="track-meta-genre" type="text" maxlength="120" /></label>
          <label>Год<input id="track-meta-year" type="number" min="1000" max="9999" /></label>
          <label>Видео URL (YouTube/Vimeo, HTTPS)<input id="track-video-url" type="url" maxlength="500" /></label>
          <button class="btn" id="track-meta-save" type="button">Сохранить данные трека</button>
          <p class="np-lyrics-hint" id="track-meta-hint"></p>
        </section>

        <div class="wave-seek" id="wave-seek">
          <canvas id="wave-canvas" width="640" height="56"></canvas>
          <div class="wave-progress" id="wave-progress"></div>
          <div class="wave-loop" id="wave-loop" hidden></div>
        </div>
        <div class="wave-times">
          <span id="wave-cur">0:00</span>
          <div class="loop-controls">
            <button class="btn" id="loop-set-a" type="button">A</button>
            <button class="btn" id="loop-set-b" type="button">B</button>
            <button class="btn" id="loop-clear" type="button">Без повтора</button>
          </div>
          <span id="wave-dur">0:00</span>
        </div>

        <div class="spectrum" id="spectrum"></div>

        <nav class="np-tabs" id="np-tabs">
          <button class="np-tab active" data-tab="queue" type="button">Далее</button>
          <button class="np-tab" data-tab="lyrics" type="button">Текст</button>
          <button class="np-tab" data-tab="comments" type="button">Комментарии</button>
          <button class="np-tab" data-tab="related" type="button">Похожие</button>
          <button class="np-tab" data-tab="signal" type="button">Signal path</button>
        </nav>

        <section class="np-tab-body" id="tab-queue" hidden>
          <ul class="np-queue" id="np-queue"></ul>
        </section>
        <section class="np-tab-body" id="tab-lyrics" hidden>
          <div class="np-lyrics" id="np-lyrics"></div>
        </section>
        <section class="np-tab-body" id="tab-comments" hidden>
          <p class="np-lyrics-hint">Комментарии пока недоступны — локальный плеер без аккаунтов.</p>
        </section>
        <section class="np-tab-body" id="tab-related" hidden>
          <p class="np-lyrics-hint">Похожие треки пока недоступны — рекомендации появятся вместе с онлайн-источниками.</p>
        </section>
        <section class="np-tab-body" id="tab-signal" hidden>
          <dl class="signal-grid" id="signal-grid"></dl>
          <p class="hint" id="np-toast"></p>
        </section>
      </div>
    </div>
  `;

  // Tabs.
  root.querySelector("#np-tabs")?.addEventListener("click", (ev) => {
    const btn = (ev.target as HTMLElement).closest<HTMLElement>(".np-tab");
    if (!btn?.dataset.tab) return;
    root.querySelectorAll<HTMLElement>(".np-tab").forEach((t) => t.classList.toggle("active", t === btn));
    for (const name of ["queue", "lyrics", "comments", "related", "signal"]) {
      const body = root.querySelector<HTMLElement>(`#tab-${name}`);
      if (body) body.hidden = name !== btn.dataset.tab;
    }
  });

  // Close: back to home (web shell) / queue (desktop keeps old nav).
  root.querySelector<HTMLButtonElement>("#np-close-btn")?.addEventListener("click", () => {
    window.location.hash = "#/home";
  });

  root.querySelector<HTMLInputElement>("#track-cover-file")?.addEventListener("change", async (event) => {
    const input = event.currentTarget as HTMLInputElement;
    const file = input.files?.[0];
    input.value = "";
    if (!file) return;
    const track = currentLibraryTrack();
    const hint = root.querySelector<HTMLElement>("#track-cover-hint");
    if (!track?.path.startsWith("web://")) return;
    if (!/^image\/(jpeg|png|webp|gif)$/.test(file.type) || file.size > 8 * 1024 * 1024) {
      if (hint) hint.textContent = "Выберите JPG, PNG, WebP или GIF размером до 8 МБ";
      return;
    }
    try {
      const dataUrl = await new Promise<string>((resolve, reject) => {
        const reader = new FileReader();
        reader.onerror = () => reject(new Error("Не удалось прочитать изображение"));
        reader.onload = () => typeof reader.result === "string" ? resolve(reader.result) : reject(new Error("Некорректное изображение"));
        reader.readAsDataURL(file);
      });
      await updateWebTrackArtwork(track.id, file, dataUrl);
      const webTrack = getWebTrackByPath(track.path);
      if (webTrack) webTrack.artwork_data_url = dataUrl;
      invalidateArtwork(track.path);
      invalidateTrackArtworkUrl(track.id);
      const art = root.querySelector<HTMLElement>("#np-art");
      if (art) await renderArtwork(art, track.path);
      const bg = root.querySelector<HTMLElement>("#np-bg");
      if (bg) bg.style.backgroundImage = `url("${dataUrl}")`;
      if (hint) hint.textContent = "Обложка сохранена";
      window.dispatchEvent(new CustomEvent("aurora:web-track-metadata-updated", { detail: { id: track.id } }));
    } catch (error) {
      if (hint) hint.textContent = error instanceof Error ? error.message : "Не удалось сохранить обложку";
    }
  });

  // Edit metadata for imported web-library tracks; desktop metadata remains unchanged.
  root.querySelector<HTMLButtonElement>("#track-meta-save")?.addEventListener("click", async () => {
    const track = currentLibraryTrack();
    if (!track?.path.startsWith("web://")) return;
    const editor = root.querySelector<HTMLElement>("#np-web-metadata");
    const button = editor?.querySelector<HTMLButtonElement>("#track-meta-save");
    const hint = editor?.querySelector<HTMLElement>("#track-meta-hint");
    const value = (selector: string) => editor?.querySelector<HTMLInputElement>(selector)?.value.trim() ?? "";
    const yearText = value("#track-meta-year");
    const year = yearText ? Number(yearText) : null;
    if (year !== null && (!Number.isInteger(year) || year < 1000 || year > 9999)) {
      if (hint) hint.textContent = "Укажите год от 1000 до 9999";
      return;
    }
    const name = value("#track-meta-title") || "Без названия";
    const artist = value("#track-meta-artist") || null;
    const album = value("#track-meta-album") || null;
    const genre = value("#track-meta-genre") || null;
    const video_url = value("#track-video-url") || null;
    const videoError = validateVideoUrl(video_url ?? "");
    if (videoError) { if (hint) hint.textContent = videoError; return; }
    if (button) button.disabled = true;
    if (hint) hint.textContent = "Сохраняю…";
    try {
      await saveTrackEnrichment({ track_id: track.id, name, artist, album, genre, year, video_url });
      track.title = name;
      track.artist = artist;
      track.album = album;
      track.genre = genre;
      track.year = year;
      const webTrack = getWebTrackByPath(track.path);
      if (webTrack) Object.assign(webTrack, { name, artist, album, genre, year, video_url });
      const queued = app.queueTracks.find((item) => item.path === track.path);
      if (queued) {
        queued.title = name;
        queued.artist = artist;
      }
      if (hint) hint.textContent = "Сохранено";
      update(root);
      window.dispatchEvent(new CustomEvent("aurora:web-track-metadata-updated", { detail: { id: track.id } }));
    } catch (error) {
      if (hint) hint.textContent = error instanceof Error ? error.message : "Не удалось сохранить";
    } finally {
      if (button) button.disabled = false;
    }
  });

  // Seek by clicking/dragging on the waveform.
  const seekEl = root.querySelector<HTMLElement>("#wave-seek");
  seekEl?.addEventListener("pointerdown", (ev) => {
    const rect = seekEl.getBoundingClientRect();
    const frac = Math.min(1, Math.max(0, (ev.clientX - rect.left) / rect.width));
    const pos = frac * app.durationS;
    if (app.durationS > 0) {
      void invoke("seek", { positionS: pos });
    }
  });

  // A/B loop controls.
  root.querySelector<HTMLButtonElement>("#loop-set-a")?.addEventListener("click", () => {
    const pos = currentPosSec(root);
    loopRegion = { start: pos, end: loopRegion?.end ?? Math.min(app.durationS, pos + 30) };
    void syncLoop();
    setToast(`Loop A at ${fmtTime(pos)}`);
  });
  root.querySelector<HTMLButtonElement>("#loop-set-b")?.addEventListener("click", () => {
    if (!loopRegion) {
      setToast("Set A first");
      return;
    }
    loopRegion.end = currentPosSec(root);
    void syncLoop();
    setToast(`Loop ${fmtTime(loopRegion.start)} → ${fmtTime(loopRegion.end)}`);
  });
  root.querySelector<HTMLButtonElement>("#loop-clear")?.addEventListener("click", () => {
    loopRegion = null;
    void invoke("set_loop", { loopStart: null, loopEnd: null });
    update(root);
    setToast("Loop off");
  });

  // Lyrics user-scroll pauses auto-scroll 4s (YTM behavior).
  const lyricsBox = root.querySelector<HTMLElement>("#np-lyrics");
  lyricsBox?.addEventListener(
    "wheel",
    () => {
      lyricsUserScrollUntil = Date.now() + 4000;
    },
    { passive: true },
  );
  lyricsBox?.addEventListener("touchmove", () => {
    lyricsUserScrollUntil = Date.now() + 4000;
  }, { passive: true });

  // Spectrum bars skeleton.
  const spec = root.querySelector<HTMLElement>("#spectrum");
  if (spec) {
    for (let i = 0; i < 16; i++) {
      const bar = document.createElement("span");
      bar.className = "spec-bar";
      spec.appendChild(bar);
    }
  }

  update(root);
}

async function syncLoop() {
  if (!loopRegion) return;
  await invoke("set_loop", {
    loopStart: loopRegion.start,
    loopEnd: loopRegion.end,
  });
}

function currentPosSec(root: HTMLElement): number {
  const cur = root.querySelector<HTMLElement>("#wave-cur")?.textContent ?? "0:00";
  const [m, s] = cur.split(":").map(Number);
  return (m || 0) * 60 + (s || 0);
}

function drawWave(root: HTMLElement) {
  const canvas = root.querySelector<HTMLCanvasElement>("#wave-canvas");
  const prog = root.querySelector<HTMLElement>("#wave-progress");
  const loopEl = root.querySelector<HTMLElement>("#wave-loop");
  const durEl = root.querySelector<HTMLElement>("#wave-dur");
  if (!canvas) return;
  const ctx = canvas.getContext("2d");
  if (!ctx) return;

  const w = canvas.width;
  const h = canvas.height;
  ctx.clearRect(0, 0, w, h);

  const bars = waveBars ?? Array<number>(WAVE_BINS).fill(0.15);
  const n = bars.length;
  const barW = w / n;
  const posFrac = app.durationS > 0 ? Math.min(1, posFromBar() / app.durationS) : 0;

  for (let i = 0; i < n; i++) {
    const v = bars[i];
    const bh = Math.max(2, v * (h - 6));
    const x = i * barW;
    const y = (h - bh) / 2;
    // Played part: accent; upcoming: quiet grey.
    ctx.fillStyle = i / n <= posFrac ? "#e8500a" : "#4a4a47";
    ctx.fillRect(x + barW * 0.15, y, Math.max(1, barW * 0.7), bh);
  }

  if (prog) prog.style.width = `${posFrac * 100}%`;

  // Loop region overlay.
  if (loopEl) {
    if (loopRegion && app.durationS > 0) {
      loopEl.hidden = false;
      loopEl.style.left = `${(loopRegion.start / app.durationS) * 100}%`;
      loopEl.style.width = `${((loopRegion.end - loopRegion.start) / app.durationS) * 100}%`;
    } else {
      loopEl.hidden = true;
    }
  }

  if (durEl && app.durationS > 0) durEl.textContent = fmtTime(app.durationS);
}

function posFromBar(): number {
  const bar = document.getElementById("ytm-seek") as HTMLInputElement | null;
  if (bar && app.durationS > 0) {
    return (Number.parseFloat(bar.value) / 1000) * app.durationS;
  }
  return 0;
}

function drawSpectrum(root: HTMLElement) {
  const spec = root.querySelector<HTMLElement>("#spectrum");
  if (!spec) return;
  const bars = spec.querySelectorAll<HTMLElement>(".spec-bar");
  if (!bars.length) return;
  void invoke<number[]>("playback_spectrum")
    .then((bands) => {
      bars.forEach((el, i) => {
        const v = bands[i] ?? 0;
        el.style.setProperty("--v", v.toFixed(3));
      });
    })
    .catch(() => {});
}

function drawQueueTab(root: HTMLElement) {
  const ul = root.querySelector<HTMLElement>("#np-queue");
  if (!ul) return;
  ul.innerHTML = "";
  app.queueTracks.forEach((t, idx) => {
    const li = document.createElement("li");
    li.className = "np-queue-item" + (idx === app.queueCurrent ? " current" : "");
    const tEl = document.createElement("span");
    tEl.textContent = t.title ?? t.path;
    const aEl = document.createElement("span");
    aEl.className = "sub";
    aEl.textContent = t.artist ?? "";
    li.append(tEl, aEl);
    ul.appendChild(li);
  });
}

function update(root: HTMLElement) {
  const s = app.status;
  const current = app.queueCurrent != null ? app.queueTracks[app.queueCurrent] : null;

  const titleEl = root.querySelector<HTMLElement>("#np-title");
  const artistEl = root.querySelector<HTMLElement>("#np-artist");
  const toastEl = root.querySelector<HTMLElement>("#np-toast");

  if (current) {
    const stem = current.path.split(/[\\/]/).pop() ?? current.path;
    const stored = currentLibraryTrack();
    if (titleEl) titleEl.textContent = stored?.title ?? current.title ?? stem;
    if (artistEl) artistEl.textContent = stored?.artist ?? current.artist ?? "—";
  } else if (app.selectedPath) {
    const stem = app.selectedPath.split(/[\\/]/).pop() ?? app.selectedPath;
    if (titleEl) titleEl.textContent = stem;
    if (artistEl) artistEl.textContent = "";
  }

  const metaTrack = currentLibraryTrack();
  const metaEditor = root.querySelector<HTMLElement>("#np-web-metadata");
  const coverPicker = root.querySelector<HTMLElement>("#np-cover-picker");
  if (coverPicker) coverPicker.hidden = !metaTrack?.path.startsWith("web://");
  if (metaEditor) {
    const isWebTrack = Boolean(metaTrack?.path.startsWith("web://"));
    metaEditor.hidden = !isWebTrack;
    if (isWebTrack && metaTrack) {
      const fields: Array<[string, string]> = [
        ["#track-meta-title", metaTrack.title ?? ""],
        ["#track-meta-artist", metaTrack.artist ?? ""],
        ["#track-meta-album", metaTrack.album ?? ""],
        ["#track-meta-genre", metaTrack.genre ?? ""],
        ["#track-meta-year", metaTrack.year == null ? "" : String(metaTrack.year)],
        ["#track-video-url", metaTrack.video_url ?? ""],
      ];
      for (const [selector, value] of fields) {
        const input = metaEditor.querySelector<HTMLInputElement>(selector);
        if (input && document.activeElement !== input && input.value !== value) input.value = value;
      }
    }
  }

  const art = root.querySelector<HTMLElement>("#np-art");
  if (art) void renderArtwork(art, current?.path ?? app.selectedPath);
  // Blurred art as page background (YTM-style atmosphere).
  const bg = root.querySelector<HTMLElement>("#np-bg");
  if (bg) void renderArtworkBg(bg, current?.path ?? app.selectedPath);

  // Load the waveform for this track (once, cached).
  const wavePath = current?.path ?? app.selectedPath;
  if (wavePath && !waveCache.has(wavePath)) {
    void loadWave(wavePath);
  }

  // Lyrics load + synced scroll for the current track.
  void loadLyrics(root, current?.artist ?? null, current?.title ?? null);
  scrollLyrics(root, posFromBar());

  drawWave(root);
  drawSpectrum(root);
  drawQueueTab(root);

  const grid = root.querySelector<HTMLElement>("#signal-grid");
  if (grid) {
    grid.innerHTML = "";
    for (const [label, get] of SIGNAL_FIELDS) {
      const cell = document.createElement("div");
      const dt = document.createElement("dt");
      dt.textContent = label;
      const dd = document.createElement("dd");
      dd.textContent = get(s);
      if (label === "integrity" && s?.integrity === "verified") {
        dd.classList.add("ok");
      }
      cell.append(dt, dd);
      grid.appendChild(cell);
    }
  }
  if (toastEl) toastEl.textContent = app.toast;
}

/** Blurred artwork background: the same artwork URL, CSS blur. */
async function renderArtworkBg(el: HTMLElement, path: string | null) {
  // Reuse the artwork module's URL resolution; blur via CSS.
  const img = el.querySelector("img");
  if (img?.dataset.path === path) return;
  el.innerHTML = "";
  if (!path) return;
  const tmp = document.createElement("div");
  tmp.style.display = "none";
  el.append(tmp);
  try {
    await renderArtwork(tmp, path);
    const url = tmp.querySelector("img")?.getAttribute("src");
    if (url) {
      const bgImg = document.createElement("img");
      bgImg.src = url;
      bgImg.dataset.path = path;
      el.innerHTML = "";
      el.appendChild(bgImg);
      return;
    }
  } catch {
    /* no artwork — gradient stays */
  }
  el.innerHTML = "";
}

/** Continuous animation: waveform progress + spectrum + lyrics sync
 *  while visible (called from the 120ms ticker). Works in BOTH shells:
 *  the desktop mounts pages under #page-root, the web shell under
 *  #ytm-content — animate looks for the player markup in either. */
export function animatePlayer() {
  const root =
    document.getElementById("page-root") ?? document.getElementById("ytm-content");
  if (!root?.querySelector("#np-title")) return;
  const curEl = root.querySelector<HTMLElement>("#wave-cur");
  if (curEl && app.durationS > 0) {
    curEl.textContent = fmtTime(posFromBar());
  }
  drawWave(root);
  drawSpectrum(root);
  scrollLyrics(root, posFromBar());
}

export const playerPage: Page = {
  id: "player",
  title: "Player",
  mount: render,
  update,
};
