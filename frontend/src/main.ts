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

interface DspConfig {
  eq: { bands: EqBand[]; preamp_db: number };
  volume_db: number;
  replaygain: "off" | "track" | "album";
}

interface PlaybackStatus {
  state: "idle" | "playing" | "draining" | "paused";
  sample_rate: number | null;
  channels: number | null;
  bits_per_sample: number | null;
  transformations: string[];
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
        <button id="btn-play" type="button" disabled>▶ Play</button>
        <button id="btn-pause" type="button" disabled>⏸ Pause</button>
        <button id="btn-resume" type="button" disabled>⏵ Resume</button>
        <button id="btn-stop" type="button" disabled>⏹ Stop</button>
      </div>

      <dl id="signal-path" class="signal-path">
        <div><dt>State</dt><dd id="st-state">idle</dd></div>
        <div><dt>Sample rate</dt><dd id="st-rate">—</dd></div>
        <div><dt>Channels</dt><dd id="st-channels">—</dd></div>
        <div><dt>Bit depth</dt><dd id="st-bits">—</dd></div>
        <div><dt>Transformations</dt><dd id="st-transform">—</dd></div>
        <div><dt>Frames played</dt><dd id="st-frames">0</dd></div>
        <div><dt>Underruns</dt><dd id="st-underruns">0</dd></div>
      </dl>

      <div id="eq-panel" class="eq-panel">
        <div class="eq-head">
          <span class="eyebrow">PARAMETRIC EQ · 10 BAND</span>
          <div class="eq-meta">
            <label>Preamp <input id="eq-preamp" type="range" min="-12" max="12" step="0.5" value="0" /> <span id="eq-preamp-v">0.0 dB</span></label>
            <label>Volume <input id="eq-volume" type="range" min="-40" max="12" step="0.5" value="0" /> <span id="eq-volume-v">0.0 dB</span></label>
            <button id="eq-flat" type="button" class="ghost">Flat</button>
          </div>
        </div>
        <div id="eq-bands" class="eq-bands"></div>
        <p class="eq-note" id="eq-note">Flat EQ = bit-exact passthrough.</p>
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

function renderStatus(status: PlaybackStatus) {
  $<HTMLElement>("st-state")!.textContent = status.state;
  $<HTMLElement>("st-rate")!.textContent =
    status.sample_rate ? `${(status.sample_rate / 1000).toFixed(1)} kHz` : "—";
  $<HTMLElement>("st-channels")!.textContent =
    status.channels ? String(status.channels) : "—";
  $<HTMLElement>("st-bits")!.textContent =
    status.bits_per_sample ? `${status.bits_per_sample}-bit` : "—";
  $<HTMLElement>("st-transform")!.textContent =
    status.transformations.length ? status.transformations.join(", ") : "none";
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
    multiple: false,
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
  if (typeof picked === "string") {
    selectedPath = picked;
    const name = picked.split(/[\\/]/).pop();
    setNote(`Loaded: ${name}`);
    if (btnPlay) btnPlay.disabled = false;
  }
});

btnPlay?.addEventListener("click", async () => {
  if (!selectedPath) return;
  try {
    const status = await invoke<PlaybackStatus>("play_track", {
      request: { path: selectedPath, device: deviceSelect?.value || null },
    });
    renderStatus(status);
    setNote("Playing.");
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
    dspConfig.volume_db !== 0;
  note.textContent = active
    ? "DSP active — transformations reported in the signal path."
    : "Flat EQ = bit-exact passthrough.";
}

loadDsp();
