/** Email/password account service (backend: crates/aurora-api/src/auth.rs).
 *  The auth endpoints share the OAuth session/CSRF cookie surface, so the
 *  request wrapper mirrors web-provider-adapter's requestJson contract
 *  (credentials: include, Origin header, X-CSRF-Token echo) — but with its
 *  own semantics: a 401 from login/register is a WRONG-CREDENTIALS answer,
 *  not an expired 8h OAuth session, so the adapter's auto-retry/bootstrap
 *  loop must NOT run here. Typed error codes (AuthErrorCode) keep the page
 *  logic free of status-number matching.
 *
 *  On login/register success the server body carries the new CSRF token
 *  (the browser already got it as a Set-Cookie, but in tests / hostile
 *  caches the body is authoritative); it is mirrored into the readable
 *  cookie so subsequent state-changing calls echo the right value. */

import { configuredApiBase, csrfToken } from "./web-provider-adapter.ts";

export type AuthErrorCode =
  | "email-taken"
  | "wrong-credentials"
  | "registration-closed"
  | "validation"
  | "forbidden-origin"
  | "rate-limited"
  | "server"
  | "network";

export interface AuthError extends Error {
  code: AuthErrorCode;
  status?: number;
}

export interface AuthIdentity {
  authenticated: boolean;
  email?: string;
}

const CSRF_COOKIE = "__Host-aurora_csrf";

function authError(code: AuthErrorCode, message: string, status?: number): AuthError {
  const error = new Error(message) as AuthError;
  error.code = code;
  error.status = status;
  return error;
}

function codeForStatus(status: number): AuthErrorCode {
  switch (status) {
    case 401: return "wrong-credentials";
    case 403: return "registration-closed";
    case 409: return "email-taken";
    case 400: return "validation";
    case 429: return "rate-limited";
    default: return "server";
  }
}

/** Mirrors the server-provided CSRF token into the readable cookie, the
 *  same one the provider adapter reads at call time. */
function storeCsrfToken(token: string): void {
  if (typeof document === "undefined") return;
  document.cookie = `${CSRF_COOKIE}=${token}; Path=/; Secure; SameSite=Lax`;
}

/** Raw auth fetch: JSON body, cookies included, Origin + CSRF echo. */
async function authRequest(path: string, init: RequestInit = {}): Promise<{ status: number; body: unknown }> {
  const apiBase = configuredApiBase();
  if (!apiBase) throw authError("network", "OAuth service is not configured");
  const url = new URL(apiBase.href);
  url.pathname = `${apiBase.pathname.replace(/\/+$/, "")}/${path.replace(/^\/+/, "")}`;
  const token = csrfToken();
  const response = await fetch(url, {
    ...init,
    credentials: "include",
    headers: {
      Accept: "application/json",
      "Content-Type": "application/json",
      Origin: apiBase.origin,
      ...(token ? { "X-CSRF-Token": token } : {}),
      ...(init.headers ?? {}),
    },
  });
  const body = response.status === 204 ? null : await response.json().catch(() => null);
  return { status: response.status, body };
}

function expectSuccess(result: { status: number; body: unknown }): AuthError {
  const code = codeForStatus(result.status);
  return authError(code, authMessage(code), result.status);
}

function authMessage(code: AuthErrorCode): string {
  switch (code) {
    case "email-taken": return "Этот адрес уже зарегистрирован";
    case "wrong-credentials": return "Неверный адрес или пароль";
    case "registration-closed": return "Регистрация закрыта на этом сервере";
    case "validation": return "Проверьте адрес и пароль (минимум 10 символов)";
    case "rate-limited": return "Слишком много попыток — попробуйте позже";
    default: return "Сервис недоступен, попробуйте позже";
  }
}

function readCredentials(value: unknown): { email: string; csrfToken: string } {
  if (typeof value !== "object" || value === null) {
    throw authError("server", "Auth service returned invalid JSON");
  }
  const record = value as Record<string, unknown>;
  const email = record.email;
  const token = record.csrfToken ?? record.csrf_token;
  if (typeof email !== "string" || typeof token !== "string" || !token) {
    throw authError("server", "Auth service returned an invalid response");
  }
  return { email, csrfToken: token };
}

/** POST /api/auth/register -> 201 {email, csrfToken}. 409 = email taken. */
export async function register(email: string, password: string): Promise<{ email: string; csrfToken: string }> {
  let result: { status: number; body: unknown };
  try {
    result = await authRequest("/api/auth/register", {
      method: "POST",
      body: JSON.stringify({ email, password }),
    });
  } catch {
    throw authError("network", "Сервис недоступен, попробуйте позже");
  }
  if (result.status !== 201) throw expectSuccess(result);
  const credentials = readCredentials(result.body);
  storeCsrfToken(credentials.csrfToken);
  return credentials;
}

/** POST /api/auth/login -> 200 {email, csrfToken}. 401 = wrong credentials. */
export async function login(email: string, password: string): Promise<{ email: string; csrfToken: string }> {
  let result: { status: number; body: unknown };
  try {
    result = await authRequest("/api/auth/login", {
      method: "POST",
      body: JSON.stringify({ email, password }),
    });
  } catch {
    throw authError("network", "Сервис недоступен, попробуйте позже");
  }
  if (result.status !== 200) throw expectSuccess(result);
  const credentials = readCredentials(result.body);
  storeCsrfToken(credentials.csrfToken);
  return credentials;
}

/** POST /api/auth/logout -> 204. Requires the CSRF token bound to the
 *  login session. On success the (readable) CSRF cookie is cleared so a
 *  stale token is not echoed afterwards. */
export async function logout(): Promise<void> {
  let result: { status: number; body: unknown };
  try {
    result = await authRequest("/api/auth/logout", { method: "POST" });
  } catch {
    throw authError("network", "Сервис недоступен, попробуйте позже");
  }
  if (result.status !== 204) {
    // A non-204 logout is a CSRF/session mismatch (403) or a server fault;
    // both leave the browser cookie state authoritative, so the generic
    // server message is used rather than the register-specific 403 wording.
    throw authError("server", authMessage("server"), result.status);
  }
  storeCsrfToken("");
}

/** GET /api/auth/me -> {authenticated, email?}. Returns null when the
 *  endpoint cannot be reached (the page then shows the anonymous card). */
export async function getMe(): Promise<AuthIdentity | null> {
  let result: { status: number; body: unknown };
  try {
    result = await authRequest("/api/auth/me");
  } catch {
    return null;
  }
  if (result.status !== 200) return null;
  if (typeof result.body !== "object" || result.body === null) return null;
  const record = result.body as Record<string, unknown>;
  if (typeof record.authenticated !== "boolean") return null;
  const identity: AuthIdentity = { authenticated: record.authenticated };
  if (typeof record.email === "string" && record.email) identity.email = record.email;
  return identity;
}
