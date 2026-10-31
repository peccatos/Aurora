import { invoke } from "@tauri-apps/api/core";
import "./style.css";

const app = document.querySelector<HTMLDivElement>("#app");

if (!app) {
  throw new Error("Application root #app was not found");
}

app.innerHTML = `
  <main class="shell">
    <header class="topbar">
      <div class="brand-mark">A</div>
      <div>
        <p class="eyebrow">LOCAL AUDIO PLAYER</p>
        <h1>Aurora</h1>
      </div>
      <span class="status"><i></i> FOUNDATION BUILD</span>
    </header>

    <section class="welcome">
      <p class="eyebrow">AUDIO FIRST</p>
      <h2>Your music.<br /><span>Your signal path.</span></h2>
      <p class="description">
        Aurora's desktop foundation is running. Playback, library indexing
        and output diagnostics will arrive in the next implementation phases.
      </p>
      <div class="actions">
        <button id="check-backend" type="button">Check backend</button>
        <span id="backend-status" role="status">Backend not checked</span>
      </div>
    </section>

    <footer>
      <span> AURORA 0.1.0</span>
      <span>DESKTOP FOUNDATION</span>
    </footer>
  </main>
`;

const button = document.querySelector<HTMLButtonElement>("#check-backend");
const status = document.querySelector<HTMLSpanElement>("#backend-status");

button?.addEventListener("click", async () => {
  if (!status || !button) return;
  button.disabled = true;
  status.textContent = "Checking…";

  try {
    const [name, version] = await Promise.all([
      invoke<string>("app_name"),
      invoke<string>("app_version"),
    ]);
    status.textContent = `${name} backend connected · v${version}`;
  } catch (error) {
    status.textContent = `Backend check failed: ${String(error)}`;
  } finally {
    button.disabled = false;
  }
});
