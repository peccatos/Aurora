/** Auth page: email/password login and registration (backend /api/auth/*).
 *  One page, two modes via the hash query: #/auth?mode=login (default)
 *  or #/auth?mode=register. Inline errors only — never window.alert.
 *  Client-side validation mirrors the backend shape checks: email-like
 *  address, password >= 10 chars. On success -> settings + toast. */

import type { Page } from "../router";
import { navigate } from "../router";
import { setToast } from "../state";
import { login, register, type AuthError } from "../auth-service";
import { platform } from "../invoke";

type AuthMode = "login" | "register";

function currentMode(): AuthMode {
  const query = window.location.hash.split("?")[1] ?? "";
  const match = /(?:^|&)mode=([^&]*)/.exec(query);
  return match?.[1] === "register" ? "register" : "login";
}

/** Backend shape sanity: an @, length 3..=254, ASCII-graphic only. */
function validEmail(value: string): boolean {
  return value.length >= 3 && value.length <= 254
    && value.includes("@")
    && [...value].every((ch) => ch.charCodeAt(0) >= 33 && ch.charCodeAt(0) <= 126);
}

function errorText(error: AuthError | string): string {
  return error instanceof Error ? error.message : String(error);
}

function setFieldError(root: HTMLElement, message: string): void {
  const el = root.querySelector<HTMLElement>("#auth-error");
  if (el) {
    el.textContent = message;
    el.hidden = !message;
  }
}

function render(root: HTMLElement): void {
  const mode = currentMode();
  const isRegister = mode === "register";
  root.innerHTML = `
    <div class="ytm-shelf-head">
      <button class="ytm-chip" id="auth-back" type="button">← Настройки</button>
      <span class="ytm-shelf-title">${isRegister ? "Создать аккаунт" : "Вход"}</span>
    </div>
    <div class="eq-card" style="max-width:460px">
      <header class="card-head">
        <p class="overline">${isRegister ? "Регистрация" : "Аккаунт Aurora"}</p>
      </header>
      <div class="settings-row" style="flex-direction:column;align-items:stretch;gap:12px">
        <label class="hint" style="display:flex;flex-direction:column;gap:4px">
          Email
          <input id="auth-email" type="email" autocomplete="username" inputmode="email" placeholder="you@example.com" />
        </label>
        <label class="hint" style="display:flex;flex-direction:column;gap:4px">
          Пароль
          <input id="auth-password" type="password" autocomplete="${isRegister ? "new-password" : "current-password"}" placeholder="Минимум 10 символов" />
        </label>
        ${isRegister
          ? `<label class="hint" style="display:flex;flex-direction:column;gap:4px">
              Повторите пароль
              <input id="auth-password-confirm" type="password" autocomplete="new-password" />
            </label>`
          : ""}
        <p class="hint err" id="auth-error" role="alert" hidden></p>
        <div class="settings-row">
          <button class="btn" id="auth-submit" type="button">${isRegister ? "Создать аккаунт" : "Войти"}</button>
          <button class="btn" id="auth-switch" type="button">${isRegister ? "Уже есть аккаунт — войти" : "Нет аккаунта — создать"}</button>
        </div>
        ${isRegister ? `<p class="hint">Аккаунт привязывает подключённые сервисы к вам: вход с любого браузера сохраняет их.</p>` : `<p class="hint">Без входа подключённые аккаунты остаются привязанными к этому браузеру.</p>`}
      </div>
    </div>
  `;

  const emailInput = root.querySelector<HTMLInputElement>("#auth-email")!;
  const passwordInput = root.querySelector<HTMLInputElement>("#auth-password")!;
  const confirmInput = root.querySelector<HTMLInputElement>("#auth-password-confirm");
  const submit = root.querySelector<HTMLButtonElement>("#auth-submit")!;

  root.querySelector<HTMLButtonElement>("#auth-back")?.addEventListener("click", () => navigate("settings"));
  root.querySelector<HTMLButtonElement>("#auth-switch")?.addEventListener("click", () =>
    navigate(isRegister ? "auth?mode=login" : "auth?mode=register"),
  );

  const submitForm = async (): Promise<void> => {
    const email = emailInput.value.trim();
    const password = passwordInput.value;
    if (!validEmail(email)) {
      setFieldError(root, "Введите корректный email-адрес");
      return;
    }
    if (password.length < 10) {
      setFieldError(root, "Пароль — минимум 10 символов");
      return;
    }
    if (isRegister && password !== confirmInput?.value) {
      setFieldError(root, "Пароли не совпадают");
      return;
    }
    setFieldError(root, "");
    submit.disabled = true;
    try {
      const identity = isRegister
        ? await register(email, password)
        : await login(email, password);
      setToast(isRegister ? `Аккаунт создан: ${identity.email}` : `Вы вошли как ${identity.email}`);
      navigate("settings");
    } catch (error) {
      setFieldError(root, errorText(error as AuthError));
      submit.disabled = false;
    }
  };

  submit.addEventListener("click", () => void submitForm());
  for (const input of [emailInput, passwordInput, confirmInput]) {
    input?.addEventListener("keydown", (event) => {
      if (event.key === "Enter") {
        event.preventDefault();
        void submitForm();
      }
    });
  }
}

export const authPage: Page = {
  id: "auth",
  title: platform === "web" ? "Вход" : "Auth",
  mount: render,
};
