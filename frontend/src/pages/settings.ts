/** Settings page: output device, bit-perfect (exclusive + verify), files. */

import { invoke, platform } from "../invoke";
import { navigate } from "../router";
import { PROVIDERS } from "../provider-registry";
import { disconnectAccount, getAccountRecord, hasProviderAuthAdapter, markAuthorizationPending, markAuthorizationTimeout, recordAuthorizationError, refreshAccountStatus, startAuthorization } from "../accounts";
import { pickAudioFiles } from "../file-picker";
import type { Page } from "../router";
import { app, loadStatus, setQueue, setToast, trackToQueueTrack } from "../state";
import type { BitPerfectVerdict, DeviceDto, ExclusivePlayResult, PlaybackStatus, QueueDto } from "../types";

function render(root: HTMLElement, syncAccounts = true) {
  root.innerHTML = `
    <section class="page">
      <header class="page-head">
        <div>
          <p class="overline">Preferences</p>
          <h1 class="page-title">Settings</h1>
        </div>
      </header>

      <div class="eq-card">
        <header class="card-head">
          <p class="overline">Output device</p>
        </header>
        <div class="settings-row">
          <select id="device-select" aria-label="Output device"></select>
        </div>
      </div>

      <div class="eq-card">
        <header class="card-head">
          <p class="overline">Your music server</p>
        </header>
        <div class="settings-row">
          <input id="server-url" type="url" placeholder="https://your-server.example/music" style="flex:1;min-width:260px" />
          <button class="btn" id="server-save" type="button">Save</button>
        </div>
        <p class="hint">A static library.json on the server lists your tracks and videos (url, title, artist, durationMs, license). The server must allow CORS. Tracks appear in Search → "Your server".</p>
      </div>

      <div class="eq-card" aria-labelledby="accounts-heading">
        <header class="card-head">
          <p class="overline" id="accounts-heading">Accounts &amp; providers</p>
        </header>
        <div class="settings-row">
          <strong>Аккаунт</strong>
          <span class="hint" id="auth-identity">Проверка…</span>
        </div>
        <div class="settings-row" id="auth-identity-actions"></div>
        <div class="settings-row">
          <strong>Local library</strong>
          <span class="hint ok">Available</span>
        </div>
        <p class="hint">Your music is stored in this app’s library. No online account is required.</p>
        <div class="settings-row">
          <strong>Your music server</strong>
          <span class="hint" id="server-status">Checking configuration…</span>
        </div>
        <p class="hint">Configured with a library URL above. Aurora reads the public library.json; no sign-in or token is used.</p>
        <div id="account-cards" class="account-cards" aria-live="polite"></div>
        <p class="hint">Aurora stores only connection metadata here. Tokens remain in the native/backend secure store; providers without a configured adapter cannot start authorization.</p>
        <p class="hint" id="account-callback-status" role="status"></p>
      </div>

      <div class="eq-card">
        <header class="card-head">
          <p class="overline">Правовые документы</p>
        </header>
        <div class="settings-row">
          <button class="btn" data-legal-link="privacy" type="button">Политика конфиденциальности</button>
          <button class="btn" data-legal-link="tos" type="button">Условия использования</button>
        </div>
      </div>

      <div class="eq-card">
        <header class="card-head">
          <p class="overline">Перевод текста песен</p>
        </header>
        <div class="settings-row">
          <label class="hint" style="display:flex;gap:8px;align-items:center"><input id="translation-enabled" type="checkbox" /> Включить построчный перевод</label>
        </div>
        <div class="settings-row">
          <input id="translation-endpoint" type="url" placeholder="https://translate.disroot.org/translate" style="flex:1;min-width:260px" />
          <button class="btn" id="translation-endpoint-save" type="button">Сохранить</button>
        </div>
        <p class="hint">Текст песни уходит на внешний сервис перевода (LibreTranslate, по умолчанию translate.disroot.org). Перевод сохраняется в библиотеке и выполняется один раз.</p>
      </div>

      <div class="eq-card">
        <header class="card-head">
          <p class="overline">Integrity</p>
        </header>
        <div class="settings-row">
          <button class="btn" id="pick-file" type="button">Open file…</button>
          <button class="btn" id="btn-bitperfect" type="button" disabled title="Play through WASAPI exclusive mode — no mixer, no DSP, exact source bytes">◎ Bit-perfect</button>
          <button class="btn" id="btn-verify" type="button" disabled title="Physically capture what the endpoint rendered and compare byte-for-byte">✓ Verify</button>
          <button class="btn" id="btn-recover" type="button" hidden>↻ Recover &amp; replay</button>
        </div>
        <p class="hint" id="bitperfect-note"></p>
      </div>
    </section>
  `;

  const deviceSel = root.querySelector<HTMLSelectElement>("#device-select");

  // Identity card (email/password account, web shell only — the auth
  // endpoints live on the API service). getMe() is a cheap same-origin
  // GET; on failure the card falls back to the anonymous state.
  const identityEl = root.querySelector<HTMLElement>("#auth-identity");
  const identityActions = root.querySelector<HTMLElement>("#auth-identity-actions");
  const renderIdentity = (identity: { authenticated: boolean; email?: string } | null): void => {
    if (!identityEl || !identityActions) return;
    identityEl.className = "hint";
    identityActions.innerHTML = "";
    if (identity?.authenticated && identity.email) {
      identityEl.textContent = identity.email;
      const logoutBtn = document.createElement("button");
      logoutBtn.className = "btn";
      logoutBtn.type = "button";
      logoutBtn.textContent = "Выйти";
      logoutBtn.addEventListener("click", async () => {
        logoutBtn.disabled = true;
        try {
          const { logout } = await import("../auth-service");
          await logout();
          setToast("Вы вышли из аккаунта");
        } catch {
          // A stale CSRF token (session rotated elsewhere) surfaces here;
          // the honest recovery is a reload, which re-reads the cookies.
          setToast("Не удалось выйти — обновите страницу и попробуйте снова");
        }
        render(root, false);
      });
      identityActions.appendChild(logoutBtn);
    } else {
      identityEl.textContent = "Вы не вошли — подключённые аккаунты привязаны к этому браузеру";
      const loginBtn = document.createElement("button");
      loginBtn.className = "btn";
      loginBtn.type = "button";
      loginBtn.textContent = "Войти";
      loginBtn.addEventListener("click", () => navigate("auth?mode=login"));
      const registerBtn = document.createElement("button");
      registerBtn.className = "btn";
      registerBtn.type = "button";
      registerBtn.textContent = "Создать аккаунт";
      registerBtn.addEventListener("click", () => navigate("auth?mode=register"));
      identityActions.append(loginBtn, registerBtn);
    }
  };
  if (platform === "web" && identityEl && identityActions) {
    void import("../auth-service").then(({ getMe }) => getMe()).then(renderIdentity).catch(() => renderIdentity(null));
  } else if (identityEl && identityActions) {
    // Desktop shell: no API auth surface — the card stays hidden entirely.
    identityEl.closest(".settings-row")?.remove();
    identityActions.remove();
  }

  const accountCards = root.querySelector<HTMLElement>("#account-cards");
  if (accountCards) {
    if (syncAccounts) void Promise.all(PROVIDERS.filter((provider) => hasProviderAuthAdapter(provider.id)).map((provider) => refreshAccountStatus(provider.id)))
      .then(() => render(root, false))
      .catch(() => { /* native status is optional; remain disconnected */ });
    accountCards.innerHTML = PROVIDERS
      .filter((provider) => provider.availability !== "unsupported")
      .map((provider) => {
        const account = getAccountRecord(provider.id);
        const configured = hasProviderAuthAdapter(provider.id);
        const state = account?.state === "connected" ? "Connected" : account?.state === "error" ? `Error: ${account.lastError ?? "authorization failed"}` : account?.state === "timeout" ? "Timed out — try again" : account?.state === "pending" ? "Waiting for browser sign-in…" : "Not connected";
        const action = account?.state === "connected"
          ? `<button class="btn" data-account-action="disconnect" data-provider="${provider.id}" type="button">Disconnect</button>`
          : `<button class="btn" data-account-action="connect" data-provider="${provider.id}" type="button" ${configured ? "" : "disabled"}>${configured ? "Connect" : "Adapter unavailable"}</button>${configured ? ` <button class="btn" data-account-action="check" data-provider="${provider.id}" type="button">Check connection</button>` : ""}`;
        return `<div class="settings-row account-card"><div><strong>${provider.name}</strong><div class="hint">${state}</div></div><div>${action}</div></div>`;
      }).join("");
    accountCards.addEventListener("click", async (event) => {
      const button = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-account-action]");
      const provider = button?.dataset.provider;
      if (!button || !provider) return;
      button.disabled = true;
      try {
        if (button.dataset.accountAction === "connect") {
          const url = await startAuthorization(provider);
          const link = document.createElement("a");
          link.href = url;
          link.target = "_blank";
          link.rel = "noopener noreferrer";
          link.textContent = "Continue sign-in in your browser";
          const opened = platform === "web" ? window.open(url, "_blank", "noopener,noreferrer") : null;
          const status = root.querySelector<HTMLElement>("#account-callback-status");
          if (status) {
            status.replaceChildren(
              document.createTextNode(platform === "web" && opened ? "Sign-in opened in a secure provider window. " : platform === "web" ? "Popup blocked — " : "Continue sign-in in your browser: "),
              link,
            );
            status.className = "hint";
          }
          markAuthorizationPending(provider);
          const deadline = Date.now() + 2 * 60 * 1000;
          const poll = async (): Promise<void> => {
            try {
              const account = await refreshAccountStatus(provider);
              if (account?.state === "connected") { render(root, false); return; }
              if (Date.now() >= deadline) { markAuthorizationTimeout(provider); render(root, false); return; }
              window.setTimeout(() => void poll(), 1500);
            } catch (error) {
              recordAuthorizationError(provider, String(error));
              render(root, false);
            }
          };
          void poll();
          button.disabled = false;
        } else if (button.dataset.accountAction === "check") {
          await refreshAccountStatus(provider);
          render(root, false);
        } else {
          await disconnectAccount(provider);
          render(root);
        }
      } catch (error) {
        const status = root.querySelector<HTMLElement>("#account-callback-status");
        if (status) { status.textContent = String(error); status.className = "hint warn"; }
        button.disabled = false;
      }
    });
  }

  // The backend/native adapter owns callback handling and token exchange. The
  // page never parses callback query parameters or receives authorization codes.
  const callbackStatus = root.querySelector<HTMLElement>("#account-callback-status");
  if (callbackStatus && platform === "web" && !hasProviderAuthAdapter("spotify") && !hasProviderAuthAdapter("youtube-music")) {
    callbackStatus.textContent = "Online provider sign-in is not configured for this deployment.";
    callbackStatus.className = "hint";
  }

  // Server library URL: persisted (kv); empty = feature hidden.
  const serverUrlInput = root.querySelector<HTMLInputElement>("#server-url");
  void import("../server-library-service").then(async (svc) => {
    const configuredUrl = await svc.getServerUrl();
    if (serverUrlInput) serverUrlInput.value = configuredUrl;
    const status = root.querySelector<HTMLElement>("#server-status");
    if (status) {
      status.textContent = configuredUrl ? "Configured" : "Not configured";
      status.className = configuredUrl ? "hint ok" : "hint";
    }
  });
  root.querySelector<HTMLButtonElement>("#server-save")?.addEventListener("click", async () => {
    const url = serverUrlInput?.value.trim() ?? "";
    const { setServerUrl, getServerTracks } = await import("../server-library-service");
    await setServerUrl(url);
    if (!url) {
      setToast("Server library cleared");
      return;
    }
    const tracks = await getServerTracks();
    setToast(
      tracks.length
        ? `Server library: ${tracks.length} track${tracks.length === 1 ? "" : "s"}`
        : "Saved — no tracks found (check library.json and CORS)",
    );
  });

  // Lyrics translation: LibreTranslate-compatible endpoint, OFF by default.
  const translationToggle = root.querySelector<HTMLInputElement>("#translation-enabled");
  const translationEndpointInput = root.querySelector<HTMLInputElement>("#translation-endpoint");
  void import("../translation-service").then(({ translationEnabled, translationEndpoint, setTranslationEnabled, setTranslationEndpoint }) => {
    if (translationToggle) translationToggle.checked = translationEnabled();
    if (translationEndpointInput) translationEndpointInput.value = translationEndpoint();
    translationToggle?.addEventListener("change", () => {
      setTranslationEnabled(translationToggle.checked);
      setToast(translationToggle.checked ? "Перевод включён" : "Перевод выключен");
    });
    root.querySelector<HTMLButtonElement>("#translation-endpoint-save")?.addEventListener("click", () => {
      const value = translationEndpointInput?.value.trim() ?? "";
      if (value && !/^https:\/\//i.test(value)) {
        setToast("Endpoint must use HTTPS");
        return;
      }
      setTranslationEndpoint(value);
      setToast("Endpoint сохранён");
    });
  });

  void (async () => {
    try {
      const devices = await invoke<DeviceDto[]>("list_output_devices");
      if (!deviceSel) return;
      deviceSel.innerHTML = "";
      const def = document.createElement("option");
      def.value = "";
      def.textContent = "System default";
      deviceSel.appendChild(def);
      for (const d of devices) {
        const opt = document.createElement("option");
        opt.value = d.key;
        opt.textContent = d.name;
        deviceSel.appendChild(opt);
      }
      try {
        const saved = await invoke<string | null>("get_output_device_setting");
        if (saved) {
          deviceSel.value = saved;
          app.device = saved;
        }
      } catch {
        /* library not open yet */
      }
    } catch (e) {
      setToast(`Device list failed: ${String(e)}`);
    }
  })();

  deviceSel?.addEventListener("change", async () => {
    app.device = deviceSel.value;
    try {
      await invoke("select_output_device", { deviceKey: deviceSel.value || null });
      setToast(
        deviceSel.value
          ? `Output device: ${deviceSel.selectedOptions[0]?.textContent}`
          : "Output device: system default",
      );
    } catch (e) {
      setToast(`Device select failed: ${String(e)}`);
    }
  });

  root.querySelectorAll<HTMLButtonElement>("[data-legal-link]").forEach((btn) => {
    btn.addEventListener("click", () => navigate(`legal?doc=${btn.dataset.legalLink}`));
  });

  root.querySelector<HTMLButtonElement>("#pick-file")?.addEventListener("click", async () => {
    // Platform-aware: native dialog on desktop, <input type=file> on the
    // web (the old plugin-dialog import crashed silently in browsers).
    if (platform === "web") {
      // Web: picking goes through the Home pipeline (register + persist
      // + play). Send the user there; the button stays honest.
      setToast("Adding music lives on the Home page — drop or pick files there");
      return;
    }
    const { paths } = await pickAudioFiles();
    if (paths.length > 0) {
      app.selectedPath = paths[0];
      app.queueTracks = paths.map((path) => ({ path }));
      app.queueCurrent = 0;
      await setQueue(app.queueTracks);
      const name = paths[0].split(/[\\/]/).pop();
      setToast(paths.length > 1 ? `Queue: ${paths.length} tracks` : `Loaded: ${name}`);
      refreshBp(root);
    }
  });

  root.querySelector<HTMLButtonElement>("#btn-bitperfect")?.addEventListener("click", () =>
    playExclusive(root),
  );
  root.querySelector<HTMLButtonElement>("#btn-verify")?.addEventListener("click", () =>
    runVerify(root),
  );
  root.querySelector<HTMLButtonElement>("#btn-recover")?.addEventListener("click", async () => {
    try {
      const status = await invoke<PlaybackStatus>("recover_playback", { replay: true });
      app.status = status;
      const btn = root.querySelector<HTMLButtonElement>("#btn-recover");
      if (btn) btn.hidden = true;
      setToast("Recovered and replaying.");
    } catch (e) {
      setToast(`Recover failed: ${String(e)}`);
    }
  });

  refreshBp(root);
}

function setBpNote(root: HTMLElement, text: string, verified?: boolean) {
  const el = root.querySelector<HTMLElement>("#bitperfect-note");
  if (!el) return;
  el.textContent = text;
  el.className = "hint" + (verified === true ? " ok" : verified === false ? " warn" : "");
}

function refreshBp(root: HTMLElement) {
  const play = root.querySelector<HTMLButtonElement>("#btn-bitperfect");
  const verify = root.querySelector<HTMLButtonElement>("#btn-verify");
  if (play) play.disabled = !app.selectedPath;
  if (verify) verify.disabled = !app.exclusiveActive || app.status?.state !== "playing";
}

async function playExclusive(root: HTMLElement) {
  if (!app.selectedPath) return;
  const btn = root.querySelector<HTMLButtonElement>("#btn-bitperfect");
  const current = app.queueTracks[app.queueCurrent ?? 0];
  try {
    if (btn) {
      btn.disabled = true;
      btn.textContent = "◎ …";
    }
    setBpNote(root, "Requesting exclusive mode…");
    const result = await invoke<ExclusivePlayResult>("play_exclusive_track", {
      request: {
        path: app.selectedPath,
        device: app.device || null,
        replaygain_track_db: current?.replaygain_track_db ?? null,
        replaygain_album_db: current?.replaygain_album_db ?? null,
      },
    });
    app.status = result.status;
    app.exclusiveActive = result.exclusive;
    if (result.exclusive) {
      setBpNote(root, "EXCLUSIVE — no mixer, no DSP. Verify to measure the wire.");
    } else {
      setBpNote(root, `Shared fallback: ${result.fallback_reason ?? "device refused"}`, false);
      setToast(`Exclusive refused — playing shared: ${result.fallback_reason ?? ""}`);
    }
  } catch (e) {
    app.exclusiveActive = false;
    setBpNote(root, "Exclusive failed", false);
    setToast(`Bit-perfect failed: ${String(e)}`);
  } finally {
    if (btn) {
      btn.disabled = false;
      btn.textContent = "◎ Bit-perfect";
    }
    refreshBp(root);
  }
}

async function runVerify(root: HTMLElement) {
  const btn = root.querySelector<HTMLButtonElement>("#btn-verify");
  try {
    if (btn) {
      btn.disabled = true;
      btn.textContent = "✓ …";
    }
    setBpNote(root, "Measuring loopback (~2 s)…");
    const verdict = await invoke<BitPerfectVerdict>("verify_bit_perfect", { seconds: 2.0 });
    if (verdict.exclusive_active && verdict.identity != null) {
      const pct = (verdict.identity * 100).toFixed(2);
      const verified = verdict.integrity === "verified";
      setBpNote(
        root,
        verified
          ? `VERIFIED ✓ ${pct}% byte-identical (${verdict.frames_compared?.toLocaleString()} frames measured)`
          : `${verdict.integrity.toUpperCase()}: ${verdict.note}`,
        verified,
      );
    } else {
      setBpNote(root, `Not measurable: ${verdict.note}`, false);
    }
  } catch (e) {
    setBpNote(root, `Verify failed: ${String(e)}`, false);
  } finally {
    if (btn) {
      btn.disabled = false;
      btn.textContent = "✓ Verify";
    }
    refreshBp(root);
  }
}

export const settingsPage: Page = {
  id: "settings",
  title: "Settings",
  mount: render,
};
