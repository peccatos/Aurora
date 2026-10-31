import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import "./style.css";

interface DeviceDto {
  key: string;
  name: string;
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
      <span class="status"><i></i> PHASE 3 · PLAYBACK ENGINE</span>
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
