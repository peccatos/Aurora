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

interface QueueTrack {
  path: string;
  title?: string | null;
  artist?: string | null;
  replaygain_track_db?: number | null;
  replaygain_album_db?: number | null;
}

interface QueueDto {
  tracks: QueueTrack[];
  current: number | null;
  display: string[];
}

interface TrackDto {
  id: string;
  path: string;
  title: string | null;
  artist: string | null;
  album: string | null;
  genre: string | null;
  year: number | null;
  track_number: number | null;
  format: string;
  duration_ms: number | null;
  replaygain_track_db: number | null;
  replaygain_album_db: number | null;
  display: string;
  duration_display: string;
}

interface ScanSummary {
  discovered: number;
  added: number;
  updated: number;
  pruned: number;
  failed: number;
}

interface LibraryStatus {
  open: boolean;
  path: string | null;
  track_count: number;
}

interface PlaylistDto {
  id: string;
  name: string;
  track_count: number;
}

interface PlaybackStatus {
  state: "idle" | "playing" | "draining" | "paused";
  /** One-shot fault report (device loss / worker crash) for recovery UI. */
  fault?: string | null;
  sample_rate: number | null;
  channels: number | null;
  bits_per_sample: number | null;
  transformations: string[];
  integrity: "verified" | "transformed" | "unverified";
  duration_s: number | null;
  frames_played: number;
  underruns: number;
}

/** Result of `play_exclusive_track`: did we take the exclusive path? */
interface ExclusivePlayResult {
  exclusive: boolean;
  fallback_reason: string | null;
  status: PlaybackStatus;
}

/** Result of `verify_bit_perfect`: the honest loopback measurement. */
interface BitPerfectVerdict {
  exclusive_active: boolean;
  identity: number | null;
  frames_compared: number | null;
  integrity: "verified" | "transformed" | "unverified";
  note: string;
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
      <span class="status"><i></i> PHASE 6 · LIBRARY</span>
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
        <button id="btn-bitperfect" type="button" disabled title="Play through WASAPI exclusive mode — no mixer, no DSP, exact source bytes">◎ Bit-perfect</button>
        <button id="btn-verify" type="button" disabled title="Physically capture what the endpoint rendered and compare byte-for-byte">✓ Verify</button>
        <span id="bitperfect-note" class="bitperfect-note"></span>
        <button id="btn-recover" type="button" class="primary" hidden>↻ Recover & Replay</button>
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

    <section class="queue-panel">
      <div class="eq-head">
        <span class="eyebrow">QUEUE</span>
        <div class="eq-meta">
          <span id="queue-count" class="lib-count"></span>
          <button id="queue-clear" type="button" class="ghost">Clear</button>
        </div>
      </div>
      <ul id="queue-list" class="queue-list"></ul>
    </section>

    <section class="library">
      <div class="eq-head">
        <span class="eyebrow">LIBRARY · SQLITE</span>
        <div class="eq-meta">
          <button id="lib-open" type="button">Open library…</button>
          <button id="lib-scan" type="button" class="primary">Scan folder…</button>
          <button id="lib-refresh" type="button">↻ Rescan</button>
          <input id="lib-search" type="search" placeholder="Search title, artist, album…" aria-label="Search" />
          <button id="lib-play-sel" type="button" disabled>▶ Play selected</button>
          <button id="lib-queue-sel" type="button" disabled>+ Queue</button>
          <span id="lib-count" class="lib-count"></span>
        </div>
      </div>

      <div class="lib-layout">
        <div class="lib-playlists">
          <div class="lib-pl-head">
            <span>PLAYLISTS</span>
            <button id="pl-new" type="button" class="ghost" title="New playlist">+</button>
          </div>
          <ul id="lib-playlist-list"></ul>
        </div>
        <div class="lib-tracks-wrap">
          <ul id="lib-tracks" class="lib-tracks"></ul>
          <p id="lib-note" class="lib-note"></p>
        </div>
      </div>
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

  if (status.fault) {
    setNote(`⚠ ${status.fault} — playback recovered safe; use Replay to retry.`);
    const recover = document.querySelector<HTMLButtonElement>("#btn-recover");
    if (recover) recover.hidden = false;
  }

  const playing = status.state === "playing";
  const paused = status.state === "paused";
  btnPlay!.disabled = !selectedPath || playing;
  btnPause!.disabled = !playing;
  btnResume!.disabled = !paused;
  btnStop!.disabled = status.state === "idle";
  // Exclusive state resets when playback stops (status may reflect a
  // shared-mode stream started elsewhere, e.g. queue auto-advance).
  if (status.state === "idle") exclusiveActive = false;
  refreshBitPerfectButtons();
}

async function refreshStatus() {
  try {
    renderStatus(await invoke<PlaybackStatus>("playback_status"));
  } catch (e) {
    setNote(`Status failed: ${String(e)}`);
  }
}

// Device list on load; restore the persisted selection (Phase 7).
async function loadDevices() {
  try {
    const devices = await invoke<DeviceDto[]>("list_output_devices");
    if (!deviceSelect) return;
    deviceSelect.innerHTML = "";
    const defaultOpt = document.createElement("option");
    defaultOpt.value = "";
    defaultOpt.textContent = "System default";
    deviceSelect.appendChild(defaultOpt);
    for (const d of devices) {
      const opt = document.createElement("option");
      opt.value = d.key;
      opt.textContent = d.name;
      deviceSelect.appendChild(opt);
    }
    // Restore saved selection (library must be open; best effort).
    try {
      const saved = await invoke<string | null>("get_output_device_setting");
      if (saved) deviceSelect.value = saved;
    } catch {
      /* library not open yet */
    }
  } catch (e) {
    setNote(`Device list failed: ${String(e)}`);
  }
}

// Persist device selection when changed.
deviceSelect?.addEventListener("change", async () => {
  try {
    await invoke("select_output_device", {
      deviceKey: deviceSelect?.value || null,
    });
    setNote(
      deviceSelect?.value
        ? `Output device: ${deviceSelect.selectedOptions[0]?.textContent}`
        : "Output device: system default",
    );
  } catch (e) {
    setNote(`Device select failed: ${String(e)}`);
  }
});

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
    queueTracks = paths.map((path) => ({ path }));
    queueCurrent = 0;
    await invoke("set_queue", { tracks: queueTracks });
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
      const current = queueTracks[queueCurrent ?? 0];
      const status = await invoke<PlaybackStatus>("play_track", {
        request: {
          path: selectedPath,
          device: deviceSelect?.value || null,
          replaygain_track_db: current?.replaygain_track_db ?? null,
          replaygain_album_db: current?.replaygain_album_db ?? null,
        },
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

document
  .querySelector<HTMLButtonElement>("#btn-recover")
  ?.addEventListener("click", async () => {
    const recover = document.querySelector<HTMLButtonElement>("#btn-recover");
    try {
      const status = await invoke<PlaybackStatus>("recover_playback", {
        replay: true,
      });
      renderStatus(status);
      if (recover) recover.hidden = true;
      setNote("Recovered and replaying.");
    } catch (e) {
      setNote(`Recover failed: ${String(e)}`);
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
let queueTracks: QueueTrack[] = [];
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
    const q = await invoke<QueueDto>("play_queue_index", { index: idx });
    queueCurrent = q.current;
    renderQueuePanel();
    const name = q.display[q.current ?? 0] ?? `Track ${idx + 1}`;
    setNote(`${name} (${(q.current ?? 0) + 1}/${q.tracks.length})`);
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
      renderQueuePanel();
      const name = q.display[q.current ?? 0] ?? "";
      setNote(`${name} (${(q.current ?? 0) + 1}/${q.tracks.length})`);
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
    renderQueuePanel();
    const name = q.display[q.current ?? 0] ?? "";
    setNote(`${name} (${(q.current ?? 0) + 1}/${q.tracks.length})`);
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
  wireBitPerfectControls();
  refreshQueueButtons();
}

// ---------------------------------------------------------------------------
// Bit-perfect (WASAPI exclusive + physical loopback verification)
// ---------------------------------------------------------------------------

/** True while an exclusive-mode stream is playing (no shared-mode fallback). */
let exclusiveActive = false;

function setBpNote(text: string, verified?: boolean) {
  const el = $id<HTMLElement>("bitperfect-note");
  if (!el) return;
  el.textContent = text;
  el.className = "bitperfect-note" + (verified === true ? " bp-verified" : verified === false ? " bp-fallback" : "");
}

function refreshBitPerfectButtons() {
  const play = $id<HTMLButtonElement>("btn-bitperfect");
  const verify = $id<HTMLButtonElement>("btn-verify");
  if (play) play.disabled = !selectedPath;
  if (verify) verify.disabled = !exclusiveActive || lastStatus?.state !== "playing";
}

function wireBitPerfectControls() {
  $id<HTMLButtonElement>("btn-bitperfect")?.addEventListener("click", playExclusive);
  $id<HTMLButtonElement>("btn-verify")?.addEventListener("click", runBitPerfectVerify);
  refreshBitPerfectButtons();
}

/** Play the current track through WASAPI exclusive mode.
 *  Falls back to the shared path (honestly reported) when the device
 *  refuses the format. */
async function playExclusive() {
  if (!selectedPath) return;
  const btn = $id<HTMLButtonElement>("btn-bitperfect");
  const current = queueTracks[queueCurrent ?? 0];
  try {
    if (btn) { btn.disabled = true; btn.textContent = "◎ …"; }
    setBpNote("Requesting exclusive mode…");
    const result = await invoke<ExclusivePlayResult>("play_exclusive_track", {
      request: {
        path: selectedPath,
        device: deviceSelect?.value || null,
        replaygain_track_db: current?.replaygain_track_db ?? null,
        replaygain_album_db: current?.replaygain_album_db ?? null,
      },
    });
    renderStatus(result.status);
    exclusiveActive = result.exclusive;
    if (result.exclusive) {
      setBpNote("EXCLUSIVE — no mixer, no DSP. Verify to measure the wire.");
    } else {
      setBpNote(`Shared fallback: ${result.fallback_reason ?? "device refused"}`, false);
      setNote(`Exclusive refused — playing shared: ${result.fallback_reason ?? ""}`);
    }
  } catch (e) {
    exclusiveActive = false;
    setBpNote("Exclusive failed", false);
    setNote(`Bit-perfect failed: ${String(e)}`);
  } finally {
    if (btn) { btn.disabled = false; btn.textContent = "◎ Bit-perfect"; }
    refreshBitPerfectButtons();
  }
}

/** Physically capture the endpoint's rendered output and compare
 *  byte-for-byte with the reference the engine pushed. */
async function runBitPerfectVerify() {
  const btn = $id<HTMLButtonElement>("btn-verify");
  try {
    if (btn) { btn.disabled = true; btn.textContent = "✓ …"; }
    setBpNote("Measuring loopback (~2 s)…");
    const verdict = await invoke<BitPerfectVerdict>("verify_bit_perfect", { seconds: 2.0 });
    if (verdict.exclusive_active && verdict.identity != null) {
      const pct = (verdict.identity * 100).toFixed(2);
      const verified = verdict.integrity === "verified";
      setBpNote(
        verified
          ? `VERIFIED ✓ ${pct}% byte-identical (${verdict.frames_compared?.toLocaleString()} frames measured)`
          : `${verdict.integrity.toUpperCase()}: ${verdict.note}`,
        verified,
      );
    } else {
      setBpNote(`Not measurable: ${verdict.note}`, false);
    }
  } catch (e) {
    setBpNote(`Verify failed: ${String(e)}`, false);
  } finally {
    if (btn) { btn.disabled = false; btn.textContent = "✓ Verify"; }
    refreshBitPerfectButtons();
  }
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
  bar.addEventListener("input", () => (seeking = true));
  // `change` fires on release (mouse or keyboard) and always carries the
  // final value; pointerup alone could race the value update on a fast
  // click into the middle of the track.
  bar.addEventListener("change", async () => {
    const pos = (Number.parseFloat(bar.value) / 100) * currentDurationS;
    try {
      await invoke("seek", { positionS: pos });
      // Reflect the jump immediately: the position poll (400ms) would
      // otherwise show the pre-seek position for up to half a second.
      if (cur) cur.textContent = fmtTime(pos);
    } catch (e) {
      setNote(`Seek failed: ${String(e)}`);
    } finally {
      seeking = false;
    }
  });
  bar.addEventListener("pointerup", () => {
    // Keyboard/edge-case safety net: re-arm the poll on any release.
    setTimeout(() => (seeking = false), 250);
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

// ---------------------------------------------------------------------------
// Library (Phase 6): open db, scan, search, playlists, correct names
// ---------------------------------------------------------------------------

let libOpen = false;
let libTracks: TrackDto[] = [];
let libPlaylists: PlaylistDto[] = [];
let libActivePlaylist: string | null = null;
let libSelectedIds = new Set<string>();

function libNote(text: string) {
  const el = $id<HTMLElement>("lib-note");
  if (el) el.textContent = text;
}

async function refreshLibraryStatus() {
  try {
    const st = await invoke<LibraryStatus>("library_status");
    libOpen = st.open;
    const count = $id<HTMLElement>("lib-count");
    if (count) {
      count.textContent = st.open ? `${st.track_count} tracks` : "not open";
    }
  } catch {
    /* backend not ready */
  }
}

function trackToQueueTrack(t: TrackDto): QueueTrack {
  return {
    path: t.path,
    title: t.title,
    artist: t.artist,
    replaygain_track_db: t.replaygain_track_db,
    replaygain_album_db: t.replaygain_album_db,
  };
}

function renderTracks(tracks: TrackDto[]) {
  libTracks = tracks;
  const list = $id<HTMLUListElement>("lib-tracks");
  if (!list) return;
  list.innerHTML = "";
  for (const t of tracks) {
    const li = document.createElement("li");
    li.className = "lib-track";
    li.dataset.id = t.id;

    const check = document.createElement("input");
    check.type = "checkbox";
    check.checked = libSelectedIds.has(t.id);
    check.addEventListener("change", () => {
      if (check.checked) libSelectedIds.add(t.id);
      else libSelectedIds.delete(t.id);
      refreshSelectionButtons();
    });

    const label = document.createElement("button");
    label.type = "button";
    label.className = "lib-track-name";
    label.textContent = t.display;
    label.title = t.path;
    label.addEventListener("dblclick", () => playNow([t]));

    const meta = document.createElement("span");
    meta.className = "lib-track-meta";
    meta.textContent = [t.album, t.year ? String(t.year) : null, t.format]
      .filter(Boolean)
      .join(" · ");

    const dur = document.createElement("span");
    dur.className = "lib-track-dur";
    dur.textContent = t.duration_display;

    li.append(check, label, meta, dur);
    list.appendChild(li);
  }
  const count = $id<HTMLElement>("lib-count");
  if (count) count.textContent = `${tracks.length} tracks`;
  refreshSelectionButtons();
}

function refreshSelectionButtons() {
  const has = libSelectedIds.size > 0;
  const play = $id<HTMLButtonElement>("lib-play-sel");
  const add = $id<HTMLButtonElement>("lib-queue-sel");
  if (play) play.disabled = !has;
  if (add) add.disabled = !has;
}

function renderQueuePanel() {
  const ul = $id<HTMLUListElement>("queue-list");
  const count = $id<HTMLElement>("queue-count");
  if (count) count.textContent = `${queueTracks.length} tracks`;
  if (!ul) return;
  ul.innerHTML = "";
  queueTracks.forEach((t, idx) => {
    const li = document.createElement("li");
    li.className = "queue-row" + (idx === queueCurrent ? " current" : "");
    const name = document.createElement("button");
    name.type = "button";
    name.className = "queue-name";
    const stem = t.path.split(/[\\/]/).pop() ?? t.path;
    name.textContent = t.artist
      ? `${t.artist} — ${t.title ?? stem}`
      : (t.title ?? stem);
    name.addEventListener("click", () => playIndex(idx));
    const del = document.createElement("button");
    del.type = "button";
    del.className = "lib-pl-del";
    del.title = "Remove from queue";
    del.textContent = "×";
    del.addEventListener("click", async () => {
      queueTracks.splice(idx, 1);
      if (queueCurrent != null) {
        if (idx < queueCurrent) queueCurrent -= 1;
        else if (idx === queueCurrent) queueCurrent = Math.min(queueCurrent, queueTracks.length - 1);
      }
      await invoke("set_queue", { tracks: queueTracks });
      renderQueuePanel();
    });
    li.append(name, del);
    ul.appendChild(li);
  });
}

async function playNow(tracks: TrackDto[]) {
  if (!tracks.length) return;
  queueTracks = tracks.map(trackToQueueTrack);
  queueCurrent = 0;
  selectedPath = queueTracks[0].path;
  try {
    await invoke("set_queue", { tracks: queueTracks });
    await invoke("play_queue_index", { index: 0 });
    refreshQueueButtons();
    renderQueuePanel();
    if (btnPlay) btnPlay.disabled = true;
    setNote(tracks[0].display);
  } catch (e) {
    setNote(`Play failed: ${String(e)}`);
  }
}

async function loadTracks(query?: string) {
  try {
    const tracks = query
      ? await invoke<TrackDto[]>("search_library", { query })
      : await invoke<TrackDto[]>("list_tracks");
    renderTracks(tracks);
    if (query && !tracks.length) libNote(`No matches for “${query}”.`);
    else libNote("");
  } catch (e) {
    libNote(String(e));
  }
}

async function loadPlaylists() {
  try {
    libPlaylists = await invoke<PlaylistDto[]>("list_playlists");
    renderPlaylists();
  } catch (e) {
    libNote(String(e));
  }
}

function renderPlaylists() {
  const ul = $id<HTMLUListElement>("lib-playlist-list");
  if (!ul) return;
  ul.innerHTML = "";
  for (const pl of libPlaylists) {
    const li = document.createElement("li");
    li.dataset.id = pl.id;
    if (pl.id === libActivePlaylist) li.classList.add("active");

    const name = document.createElement("button");
    name.type = "button";
    name.className = "lib-pl-name";
    name.textContent = `${pl.name} (${pl.track_count})`;
    name.addEventListener("click", async () => {
      libActivePlaylist = pl.id;
      renderPlaylists();
      try {
        const tracks = await invoke<TrackDto[]>("playlist_tracks", {
          playlistId: pl.id,
        });
        renderTracks(tracks);
      } catch (e) {
        libNote(String(e));
      }
    });

    const del = document.createElement("button");
    del.type = "button";
    del.className = "lib-pl-del";
    del.title = "Delete playlist";
    del.textContent = "×";
    del.addEventListener("click", async () => {
      try {
        await invoke("delete_playlist", { playlistId: pl.id });
        if (libActivePlaylist === pl.id) {
          libActivePlaylist = null;
          await loadTracks();
        }
        await loadPlaylists();
      } catch (e) {
        libNote(String(e));
      }
    });

    li.append(name, del);
    ul.appendChild(li);
  }
}

function setupLibrary() {
  $id<HTMLButtonElement>("lib-open")?.addEventListener("click", async () => {
    const picked = await open({
      multiple: false,
      filters: [{ name: "Aurora library", extensions: ["db"] }],
    });
    const path = typeof picked === "string" ? picked : null;
    if (!path) return;
    try {
      const st = await invoke<LibraryStatus>("open_library", { path });
      libOpen = st.open;
      libNote(st.open ? `Library: ${st.path}` : "Failed to open library");
      await refreshLibraryStatus();
      await loadTracks();
      await loadPlaylists();
    } catch (e) {
      libNote(String(e));
    }
  });

  $id<HTMLButtonElement>("lib-scan")?.addEventListener("click", async () => {
    if (!libOpen) {
      libNote("Open a library (.db) first.");
      return;
    }
    const picked = await open({ multiple: false, directory: true });
    const root = typeof picked === "string" ? picked : null;
    if (!root) return;
    libNote("Scanning…");
    try {
      const summary = await invoke<ScanSummary>("scan_folder", { root });
      libNote(
        `Scanned: ${summary.discovered} found, ${summary.added} added, ` +
          `${summary.updated} updated, ${summary.pruned} removed` +
          (summary.failed ? `, ${summary.failed} failed` : ""),
      );
      await refreshLibraryStatus();
      await loadTracks();
      await loadPlaylists();
    } catch (e) {
      libNote(String(e));
    }
  });

  let searchTimer: ReturnType<typeof setTimeout> | null = null;
  $id<HTMLInputElement>("lib-search")?.addEventListener("input", (e) => {
    const q = (e.target as HTMLInputElement).value.trim();
    if (searchTimer) clearTimeout(searchTimer);
    searchTimer = setTimeout(() => {
      if (!libOpen) {
        libNote("Open a library first.");
        return;
      }
      loadTracks(q || undefined);
    }, 250);
  });

  $id<HTMLButtonElement>("pl-new")?.addEventListener("click", async () => {
    if (!libOpen) return;
    const name = prompt("Playlist name");
    if (!name) return;
    try {
      await invoke("create_playlist", { name });
      await loadPlaylists();
    } catch (e) {
      libNote(String(e));
    }
  });
}

// Double-click a track row plays it now; buttons below act on selection.
function setupLibraryActions() {
  document.addEventListener("keydown", (e) => {
    if (e.key !== "Enter") return;
    const sel = libTracks.filter((t) => libSelectedIds.has(t.id));
    if (sel.length) playNow(sel);
  });

  $id<HTMLButtonElement>("lib-play-sel")?.addEventListener("click", () => {
    const sel = libTracks.filter((t) => libSelectedIds.has(t.id));
    playNow(sel);
  });

  $id<HTMLButtonElement>("lib-queue-sel")?.addEventListener("click", async () => {
    const sel = libTracks.filter((t) => libSelectedIds.has(t.id));
    if (!sel.length) return;
    try {
      const q = await invoke<QueueDto>("queue_add_tracks", {
        tracks: sel.map(trackToQueueTrack),
      });
      queueTracks = q.tracks;
      queueCurrent = q.current;
      renderQueuePanel();
      refreshQueueButtons();
      setNote(`Added ${sel.length} to queue (now ${q.tracks.length}).`);
    } catch (e) {
      setNote(`Queue add failed: ${String(e)}`);
    }
  });

  $id<HTMLButtonElement>("queue-clear")?.addEventListener("click", async () => {
    queueTracks = [];
    queueCurrent = null;
    try {
      await invoke("set_queue", { tracks: [] });
    } catch {
      /* ignore */
    }
    renderQueuePanel();
    refreshQueueButtons();
    setNote("Queue cleared.");
  });

  $id<HTMLButtonElement>("lib-refresh")?.addEventListener("click", async () => {
    if (!libOpen) {
      libNote("No library open.");
      return;
    }
    libNote("Rescanning known roots…");
    try {
      const summary = await invoke<ScanSummary>("rescan_all");
      libNote(
        `Rescan: ${summary.discovered} found, ${summary.added} added, ` +
          `${summary.updated} updated, ${summary.pruned} removed` +
          (summary.failed ? `, ${summary.failed} failed` : ""),
      );
      await refreshLibraryStatus();
      await loadTracks();
    } catch (e) {
      libNote(String(e));
    }
  });
}

setupLibrary();
setupLibraryActions();
refreshLibraryStatus();
renderQueuePanel();

// Phase 7: the backend auto-opens the library at app start; the panel
// fills itself once that completes (bounded wait, then manual mode).
let bootTries = 0;
const bootTimer = setInterval(async () => {
  bootTries += 1;
  try {
    const st = await invoke<LibraryStatus>("library_status");
    if (st.open) {
      libOpen = true;
      await refreshLibraryStatus();
      await loadTracks();
      await loadPlaylists();
      clearInterval(bootTimer);
    } else if (bootTries > 40) {
      // ~20s: give up polling; user can open manually.
      clearInterval(bootTimer);
    }
  } catch {
    clearInterval(bootTimer);
  }
}, 500);
