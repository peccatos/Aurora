import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import "./style.css";

interface DeviceDto {
  key: string;
  name: string;
}

interface EqBand {
  kind: "peaking" | "low_shelf" | "high_shelf" | "low_pass" | "high_pass";
  freq_hz: number;
  q: number;
  gain_db: number;
  enabled: boolean;
}

interface ReverbParams {
  room_size: number;
  damping: number;
  wet: number;
  width: number;
  input_lowpass: number;
}

interface DspConfig {
  eq: { bands: EqBand[]; preamp_db: number };
  volume_db: number;
  replaygain: "off" | "track" | "album";
  reverb: ReverbParams;
  speed: number;
}

interface QueueDto {
  tracks: string[];
  current: number | null;
}

interface PlaybackStatus {
  state: "idle" | "playing" | "draining" | "paused";
  sample_rate: number | null;
  channels: number | null;
  bits_per_sample: number | null;
  transformations: string[];
  integrity: "verified" | "transformed" | "unverified";
  duration_s: number | null;
  frames_played: number;
  underruns: number;
}

const app = document.querySelector<HTMLDivElement>("#app");

if (!app) {
  throw new Error("Application root #app was not found");
}

app.innerHTML = `
  <main class="shell">
    <header class="topbar">
      <img class="brand-mark" src="/logo-128.png" alt="Aurora" width="44" height="44" />
      <div>
        <p class="eyebrow">LOCAL AUDIO PLAYER</p>
        <h1>Aurora</h1>
      </div>
      <span class="status"><i></i> PHASE 4 · DSP ENGINE</span>
    </header>

    <section class="player">
      <div class="player-controls">
        <button id="pick-file" type="button" class="primary">Open file…</button>
        <select id="device-select" aria-label="Output device"></select>
        <button id="btn-prev" type="button" disabled>⏮</button>
        <button id="btn-play" type="button" disabled>▶ Play</button>
        <button id="btn-pause" type="button" disabled>⏸ Pause</button>
        <button id="btn-resume" type="button" disabled>⏵ Resume</button>
        <button id="btn-stop" type="button" disabled>⏹ Stop</button>
        <button id="btn-next" type="button" disabled>⏭</button>
        <button id="btn-shuffle" type="button">🔀</button>
      </div>

      <div class="seek-row">
        <span id="seek-cur">0:00</span>
        <input id="seek" type="range" min="0" max="100" step="0.1" value="0" disabled />
        <span id="seek-dur">0:00</span>
      </div>

      <dl id="signal-path" class="signal-path">
        <div><dt>State</dt><dd id="st-state">idle</dd></div>
        <div><dt>Sample rate</dt><dd id="st-rate">—</dd></div>
        <div><dt>Channels</dt><dd id="st-channels">—</dd></div>
        <div><dt>Bit depth</dt><dd id="st-bits">—</dd></div>
        <div><dt>Transformations</dt><dd id="st-transform">—</dd></div>
        <div><dt>Integrity</dt><dd id="st-integrity">—</dd></div>
        <div><dt>Frames played</dt><dd id="st-frames">0</dd></div>
        <div><dt>Underruns</dt><dd id="st-underruns">0</dd></div>
      </dl>

      <div id="eq-panel" class="eq-panel">
        <div class="eq-head">
          <span class="eyebrow">PARAMETRIC EQ · 10 BAND</span>
          <div class="eq-meta">
            <select id="eq-preset" aria-label="Preset">
              <option value="flat">Flat</option>
              <option value="bass">Bass Boost</option>
              <option value="vocal">Vocal</option>
              <option value="rock">Rock</option>
              <option value="jazz">Jazz</option>
              <option value="electronic">Electronic</option>
              <option value="classical">Classical</option>
              <option value="night">Late Night</option>
            </select>
            <label>Preamp <input id="eq-preamp" type="range" min="-12" max="12" step="0.5" value="0" /> <span id="eq-preamp-v">0.0 dB</span></label>
            <label>Volume <input id="eq-volume" type="range" min="-40" max="12" step="0.5" value="0" /> <span id="eq-volume-v">0.0 dB</span></label>
            <button id="eq-flat" type="button" class="ghost">Flat</button>
          </div>
        </div>
        <div id="eq-bands" class="eq-bands"></div>
        <p class="eq-note" id="eq-note">Flat EQ = bit-exact passthrough.</p>
      </div>

      <div class="reverb-panel">
        <div class="eq-head">
          <span class="eyebrow">REVERB · FREEVERB</span>
          <button id="reverb-off" type="button" class="ghost">Off</button>
        </div>
        <div class="reverb-controls">
          <label>Wet <input id="rv-wet" type="range" min="0" max="1" step="0.01" value="0" /><b id="rv-wet-v">0%</b></label>
          <label>Room <input id="rv-room" type="range" min="0" max="1" step="0.01" value="0.5" /><b id="rv-room-v">0.50</b></label>
          <label>Damp <input id="rv-damp" type="range" min="0" max="1" step="0.01" value="0.3" /><b id="rv-damp-v">0.30</b></label>
          <label>Width <input id="rv-width" type="range" min="0" max="1" step="0.01" value="1" /><b id="rv-width-v">1.00</b></label>
          <label>Speed <input id="fx-speed" type="range" min="0.5" max="2" step="0.05" value="1" /><b id="fx-speed-v">1.00x</b></label>
          <button id="fx-slowed" type="button" class="ghost">Slowed</button>
          <button id="fx-speedup" type="button" class="ghost">Speed Up</button>
        </div>
      </div>

      <p id="player-note" role="status"></p>
    </section>

    <footer>
      <span>AURORA 0.1.0</span>
      <span>PLAYBACK ENGINE</span>
    </footer>
  </main>
`;

const $ = <T extends HTMLElement>(id: string) =>
  document.querySelector<T>(`#${id}`);

const deviceSelect = $<HTMLSelectElement>("device-select");
const pickFile = $<HTMLButtonElement>("pick-file");
const btnPlay = $<HTMLButtonElement>("btn-play");
const btnPause = $<HTMLButtonElement>("btn-pause");
const btnResume = $<HTMLButtonElement>("btn-resume");
const btnStop = $<HTMLButtonElement>("btn-stop");
const note = $<HTMLParagraphElement>("player-note");

let selectedPath: string | null = null;

function setNote(text: string) {
  if (note) note.textContent = text;
}

let lastStatus: PlaybackStatus | null = null;

function renderStatus(status: PlaybackStatus) {
  lastStatus = status;
  $<HTMLElement>("st-state")!.textContent = status.state;
  $<HTMLElement>("st-rate")!.textContent =
    status.sample_rate ? `${(status.sample_rate / 1000).toFixed(1)} kHz` : "—";
  $<HTMLElement>("st-channels")!.textContent =
    status.channels ? String(status.channels) : "—";
  $<HTMLElement>("st-bits")!.textContent =
    status.bits_per_sample ? `${status.bits_per_sample}-bit` : "—";
  $<HTMLElement>("st-transform")!.textContent =
    status.transformations.length ? status.transformations.join(", ") : "none";
  if (status.duration_s && status.duration_s > 0) {
    currentDurationS = status.duration_s;
  }
  const integrityEl = $<HTMLElement>("st-integrity");
  if (integrityEl) {
    integrityEl.textContent =
      status.integrity === "verified"
        ? "VERIFIED ✓"
        : status.integrity === "transformed"
          ? "transformed"
          : "unverified";
    integrityEl.className = status.integrity === "verified" ? "int-verified" : "";
  }
  $<HTMLElement>("st-frames")!.textContent =
    status.frames_played.toLocaleString();
  $<HTMLElement>("st-underruns")!.textContent =
    status.underruns.toLocaleString();

  const playing = status.state === "playing";
  const paused = status.state === "paused";
  btnPlay!.disabled = !selectedPath || playing;
  btnPause!.disabled = !playing;
  btnResume!.disabled = !paused;
  btnStop!.disabled = status.state === "idle";
}

async function refreshStatus() {
  try {
    renderStatus(await invoke<PlaybackStatus>("playback_status"));
  } catch (e) {
    setNote(`Status failed: ${String(e)}`);
  }
}

// Device list on load.
async function loadDevices() {
  try {
    const devices = await invoke<DeviceDto[]>("list_output_devices");
    if (!deviceSelect) return;
    deviceSelect.innerHTML = "";
    for (const d of devices) {
      const opt = document.createElement("option");
      opt.value = d.key;
      opt.textContent = d.name;
      deviceSelect.appendChild(opt);
    }
  } catch (e) {
    setNote(`Device list failed: ${String(e)}`);
  }
}

loadDevices();
refreshStatus();
// Live metrics while playing.
setInterval(() => {
  invoke<PlaybackStatus>("playback_status").then(renderStatus).catch(() => {});
}, 500);

pickFile?.addEventListener("click", async () => {
  const picked = await open({
    multiple: true,
    filters: [
      {
        name: "Audio",
        extensions: [
          "flac", "wav", "wave", "mp3", "m4a", "alac", "aac",
          "ogg", "oga", "opus", "aif", "aiff", "wv",
        ],
      },
    ],
  });
  const paths = Array.isArray(picked) ? picked : typeof picked === "string" ? [picked] : [];
  if (paths.length > 0) {
    selectedPath = paths[0];
    queueTracks = paths;
    queueCurrent = 0;
    await invoke("set_queue", { tracks: paths });
    refreshQueueButtons();
    const name = paths[0].split(/[\\/]/).pop();
    setNote(paths.length > 1 ? `Queue: ${paths.length} tracks` : `Loaded: ${name}`);
    if (btnPlay) btnPlay.disabled = false;
  }
});

btnPlay?.addEventListener("click", async () => {
  if (!selectedPath) return;
  try {
    // Play while paused = resume from the same position, NOT a restart.
    if (lastStatus?.state === "paused") {
      renderStatus(await invoke<PlaybackStatus>("resume_playback"));
      setNote("Resumed.");
      return;
    }
    if (queueTracks.length > 1) {
      await playIndex(queueCurrent ?? 0);
    } else {
      const status = await invoke<PlaybackStatus>("play_track", {
        request: { path: selectedPath, device: deviceSelect?.value || null },
      });
      renderStatus(status);
      setNote("Playing.");
    }
  } catch (e) {
    setNote(`Play failed: ${String(e)}`);
  }
});

btnPause?.addEventListener("click", async () => {
  try {
    renderStatus(await invoke<PlaybackStatus>("pause_playback"));
  } catch (e) {
    setNote(`Pause failed: ${String(e)}`);
  }
});

btnResume?.addEventListener("click", async () => {
  try {
    renderStatus(await invoke<PlaybackStatus>("resume_playback"));
  } catch (e) {
    setNote(`Resume failed: ${String(e)}`);
  }
});

btnStop?.addEventListener("click", async () => {
  try {
    renderStatus(await invoke<PlaybackStatus>("stop_playback"));
    setNote("Stopped.");
  } catch (e) {
    setNote(`Stop failed: ${String(e)}`);
  }
});

// ---------------------------------------------------------------------------
// EQ panel
// ---------------------------------------------------------------------------

const BAND_FREQS = [31, 63, 125, 250, 500, 1000, 2000, 4000, 8000, 16000];

let dspConfig: DspConfig = {
  eq: {
    bands: BAND_FREQS.map((f) => ({
      kind: "peaking" as const,
      freq_hz: f,
      q: 1.0,
      gain_db: 0,
      enabled: true,
    })),
    preamp_db: 0,
  },
  volume_db: 0,
  replaygain: "off",
  reverb: { room_size: 0.5, damping: 0.3, wet: 0.0, width: 1.0, input_lowpass: 0.0 },
  speed: 1.0,
};

async function loadDsp() {
  try {
    dspConfig = await invoke<DspConfig>("get_dsp");
    renderEq();
  } catch {
    // Backend not ready; defaults are fine.
  }
}

async function saveDsp() {
  try {
    await invoke("set_dsp", { config: dspConfig });
  } catch (e) {
    const note = document.querySelector<HTMLElement>("#eq-note");
    if (note) note.textContent = `Save failed: ${String(e)}`;
  }
}

function renderEq() {
  const wrap = document.querySelector<HTMLElement>("#eq-bands");
  if (!wrap) return;
  wrap.innerHTML = "";
  dspConfig.eq.bands.forEach((band, idx) => {
    const cell = document.createElement("div");
    cell.className = "eq-band";
    const label = document.createElement("span");
    label.textContent = band.freq_hz >= 1000 ? `${band.freq_hz / 1000}k` : `${band.freq_hz}`;
    const slider = document.createElement("input");
    slider.type = "range";
    slider.min = "-12";
    slider.max = "12";
    slider.step = "0.5";
    slider.value = String(band.gain_db);
    slider.dataset.band = String(idx);
    const value = document.createElement("b");
    value.textContent = `${band.gain_db > 0 ? "+" : ""}${band.gain_db.toFixed(1)}`;
    slider.addEventListener("input", () => {
      const v = Number.parseFloat(slider.value);
      dspConfig.eq.bands[idx].gain_db = v;
      value.textContent = `${v > 0 ? "+" : ""}${v.toFixed(1)}`;
      updateEqNote();
      saveDsp();
    });
    cell.append(label, slider, value);
    wrap.appendChild(cell);
  });

  const preamp = document.querySelector<HTMLInputElement>("#eq-preamp");
  const preampV = document.querySelector<HTMLElement>("#eq-preamp-v");
  if (preamp && preampV) {
    preamp.value = String(dspConfig.eq.preamp_db);
    preampV.textContent = `${dspConfig.eq.preamp_db.toFixed(1)} dB`;
    preamp.addEventListener("input", () => {
      dspConfig.eq.preamp_db = Number.parseFloat(preamp.value);
      preampV.textContent = `${dspConfig.eq.preamp_db.toFixed(1)} dB`;
      updateEqNote();
      saveDsp();
    });
  }

  const volume = document.querySelector<HTMLInputElement>("#eq-volume");
  const volumeV = document.querySelector<HTMLElement>("#eq-volume-v");
  if (volume && volumeV) {
    volume.value = String(dspConfig.volume_db);
    volumeV.textContent = `${dspConfig.volume_db.toFixed(1)} dB`;
    volume.addEventListener("input", () => {
      dspConfig.volume_db = Number.parseFloat(volume.value);
      volumeV.textContent = `${dspConfig.volume_db.toFixed(1)} dB`;
      updateEqNote();
      saveDsp();
    });
  }

  const flat = document.querySelector<HTMLButtonElement>("#eq-flat");
  flat?.addEventListener("click", () => {
    dspConfig.eq.bands.forEach((b) => (b.gain_db = 0));
    dspConfig.eq.preamp_db = 0;
    renderEq();
    updateEqNote();
    saveDsp();
  });

  updateEqNote();
}

function updateEqNote() {
  const note = document.querySelector<HTMLElement>("#eq-note");
  if (!note) return;
  const active =
    dspConfig.eq.bands.some((b) => b.gain_db !== 0) ||
    dspConfig.eq.preamp_db !== 0 ||
    dspConfig.volume_db !== 0 ||
    dspConfig.reverb.wet > 0 ||
    Math.abs(dspConfig.speed - 1) > 0.01;
  note.textContent = active
    ? "DSP active — transformations reported in the signal path."
    : "Flat EQ = bit-exact passthrough.";
}

loadDsp();

// ---------------------------------------------------------------------------
// Queue / seek / reverb / shuffle
// ---------------------------------------------------------------------------

const $id = <T extends HTMLElement>(id: string) => document.querySelector<T>(`#${id}`);

let currentDurationS = 0;
let queueTracks: string[] = [];
let queueCurrent: number | null = null;
let shuffleMode = false;

function fmtTime(s: number): string {
  if (!Number.isFinite(s) || s < 0) return "0:00";
  const m = Math.floor(s / 60);
  const sec = Math.floor(s % 60);
  return `${m}:${sec.toString().padStart(2, "0")}`;
}

async function playIndex(idx: number) {
  try {
    await invoke("play_queue_index", { index: idx });
    setNote(`Track ${idx + 1}/${queueTracks.length}`);
  } catch (e) {
    setNote(`Queue play failed: ${String(e)}`);
  }
}

async function nextTrack() {
  try {
    const q = await invoke<QueueDto | null>("next_track");
    if (q) {
      queueTracks = q.tracks;
      queueCurrent = q.current;
      setNote(`Track ${(q.current ?? 0) + 1}/${q.tracks.length}`);
    } else if (queueTracks.length === 1) {
      // Single track: replay from the top.
      await playIndex(0);
    } else {
      setNote("End of queue");
    }
  } catch (e) {
    setNote(`Next failed: ${String(e)}`);
  }
}

async function prevTrack() {
  try {
    const q = await invoke<QueueDto>("prev_track");
    queueTracks = q.tracks;
    queueCurrent = q.current;
    setNote(`Track ${(q.current ?? 0) + 1}/${q.tracks.length}`);
  } catch (e) {
    setNote(`Prev failed: ${String(e)}`);
  }
}

async function toggleShuffle() {
  shuffleMode = !shuffleMode;
  if (shuffleMode && queueTracks.length > 1) {
    // Fisher-Yates: keep current track first.
    const cur = queueCurrent ?? 0;
    const head = queueTracks[cur];
    const rest = queueTracks.filter((_, i) => i !== cur);
    for (let i = rest.length - 1; i > 0; i--) {
      const j = Math.floor(Math.random() * (i + 1));
      [rest[i], rest[j]] = [rest[j], rest[i]];
    }
    queueTracks = [head, ...rest];
    queueCurrent = 0;
    await invoke("set_queue", { tracks: queueTracks });
    setNote("Shuffled queue");
  } else {
    setNote("Shuffle off");
  }
}

function wireQueueControls() {
  $id<HTMLButtonElement>("btn-prev")?.addEventListener("click", prevTrack);
  $id<HTMLButtonElement>("btn-next")?.addEventListener("click", nextTrack);
  $id<HTMLButtonElement>("btn-shuffle")?.addEventListener("click", toggleShuffle);
  refreshQueueButtons();
}

function refreshQueueButtons() {
  const has = queueTracks.length > 0;
  const prev = $id<HTMLButtonElement>("btn-prev");
  const next = $id<HTMLButtonElement>("btn-next");
  if (prev) prev.disabled = !has;
  if (next) next.disabled = !has;
  // Single track: next restarts it, prev rewinds — both meaningful.
}

// Seek: click-drag sends the position; polling updates the bar.
let seeking = false;
async function setupSeek() {
  const bar = $id<HTMLInputElement>("seek");
  const cur = $id<HTMLElement>("seek-cur");
  const dur = $id<HTMLElement>("seek-dur");
  if (!bar) return;

  bar.addEventListener("pointerdown", () => (seeking = true));
  bar.addEventListener("pointerup", async () => {
    seeking = false;
    const pos = (Number.parseFloat(bar.value) / 100) * currentDurationS;
    try {
      await invoke("seek", { positionS: pos });
    } catch (e) {
      setNote(`Seek failed: ${String(e)}`);
    }
  });

  setInterval(async () => {
    try {
      const pos = await invoke<number>("playback_position");
      if (!seeking && bar && currentDurationS > 0) {
        bar.disabled = false;
        bar.value = String((pos / currentDurationS) * 100);
      }
      if (cur) cur.textContent = fmtTime(pos);
      if (dur && currentDurationS > 0) dur.textContent = fmtTime(currentDurationS);
      // Auto-advance at natural end.
      if (pos >= currentDurationS - 0.3 && currentDurationS > 0) {
        await nextTrack();
      }
    } catch {
      /* not playing */
    }
  }, 400);
}

function trackDurationHint(path: string) {
  // Duration comes from the backend on play; approximate until then.
  currentDurationS = 0;
}

// Reverb panel.
function setupReverbPanel() {
  const bind = (
    id: string,
    label: string,
    key: keyof ReverbParams,
    fmt: (v: number) => string,
  ) => {
    const el = $id<HTMLInputElement>(id);
    const out = $id<HTMLElement>(label);
    if (!el) return;
    el.addEventListener("input", () => {
      const v = Number.parseFloat(el.value);
      dspConfig.reverb[key] = v;
      if (out) out.textContent = fmt(v);
      updateEqNote();
      saveDsp();
    });
  };

  bind("rv-wet", "rv-wet-v", "wet", (v) => `${Math.round(v * 100)}%`);
  bind("rv-room", "rv-room-v", "room_size", (v) => v.toFixed(2));
  bind("rv-damp", "rv-damp-v", "damping", (v) => v.toFixed(2));
  bind("rv-width", "rv-width-v", "width", (v) => v.toFixed(2));

  // Speed slider.
  const speedEl = $id<HTMLInputElement>("fx-speed");
  const speedV = $id<HTMLElement>("fx-speed-v");
  speedEl?.addEventListener("input", () => {
    const v = Number.parseFloat(speedEl.value);
    dspConfig.speed = v;
    if (speedV) speedV.textContent = `${v.toFixed(2)}x`;
    updateEqNote();
    saveDsp();
  });
  $id<HTMLButtonElement>("fx-slowed")?.addEventListener("click", () => {
    dspConfig.speed = 0.8;
    if (speedEl) speedEl.value = "0.8";
    if (speedV) speedV.textContent = "0.80x";
    updateEqNote();
    saveDsp();
  });
  $id<HTMLButtonElement>("fx-speedup")?.addEventListener("click", () => {
    dspConfig.speed = 1.25;
    if (speedEl) speedEl.value = "1.25";
    if (speedV) speedV.textContent = "1.25x";
    updateEqNote();
    saveDsp();
  });

  $id<HTMLButtonElement>("reverb-off")?.addEventListener("click", () => {
    dspConfig.reverb.wet = 0;
    const el = $id<HTMLInputElement>("rv-wet");
    const out = $id<HTMLElement>("rv-wet-v");
    if (el) el.value = "0";
    if (out) out.textContent = "0%";
    updateEqNote();
    saveDsp();
  });
}

wireQueueControls();
setupSeek();
setupReverbPanel();

// ---------------------------------------------------------------------------
// EQ presets (gain_db per band index: 31..16k)
// ---------------------------------------------------------------------------

const EQ_PRESETS: Record<string, number[]> = {
  flat: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
  bass: [6, 5, 3.5, 1.5, 0, 0, 0, 0, 0, 0],
  vocal: [-2, -1, 0, 2, 4, 4.5, 3.5, 1.5, 0, -1],
  rock: [4.5, 3.5, 1.5, -1, -2, 1, 3, 4.5, 4, 3.5],
  jazz: [3, 2, 1, 1.5, -1, -1, 0, 1.5, 2.5, 3],
  electronic: [5, 4, 1, 0, -1.5, -1, 1.5, 3, 4.5, 5],
  classical: [3.5, 2.5, 1, 0, 0, 0, 0, 1, 2, 3],
  night: [-3, -1.5, 0, 1.5, 3, 3.5, 3, 2, 1, 0],
};

function applyPreset(name: string) {
  const gains = EQ_PRESETS[name];
  if (!gains) return;
  dspConfig.eq.bands.forEach((b, i) => (b.gain_db = gains[i] ?? 0));
  dspConfig.eq.preamp_db = name === "night" ? -2 : 0;
  renderEq();
  saveDsp();
}

document
  .querySelector<HTMLSelectElement>("#eq-preset")
  ?.addEventListener("change", (e) => {
    applyPreset((e.target as HTMLSelectElement).value);
  });
