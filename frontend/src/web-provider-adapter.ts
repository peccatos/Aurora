import { registerProviderAuthAdapter } from "./accounts";
import { normalizeOAuthAccount } from "./oauth";
import { platform } from "./invoke";

declare global {
  interface ImportMeta {
    readonly env?: { readonly VITE_OAUTH_API_BASE_URL?: string };
  }
}

const WEB_PROVIDERS = ["spotify", "youtube-music"] as const;
type WebProvider = (typeof WEB_PROVIDERS)[number];

function configuredApiBase(): URL | null {
  const raw = import.meta.env?.VITE_OAUTH_API_BASE_URL?.trim();
  if (!raw) return null;
  try {
    const url = new URL(raw);
    if (url.protocol !== "https:") return null;
    url.pathname = url.pathname.replace(/\/+$/, "");
    url.search = "";
    url.hash = "";
    return url;
  } catch {
    return null;
  }
}

const AUTH_HOSTS: Record<WebProvider, string> = {
  spotify: "accounts.spotify.com",
  "youtube-music": "accounts.google.com",
};

const FORBIDDEN_KEYS = new Set([
  "accesstoken",
  "refreshtoken",
  "authorizationcode",
  "code",
  "verifier",
  "codeverifier",
  "pkceverifier",
  "clientsecret",
  "token",
  "secret",
]);

function normalizedKey(key: string): string {
  return key.toLowerCase().replace(/[^a-z0-9]/g, "");
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function assertNoCredentialFields(value: unknown): void {
  if (Array.isArray(value)) {
    for (const item of value) assertNoCredentialFields(item);
    return;
  }
  if (!isObject(value)) return;
  for (const [key, nested] of Object.entries(value)) {
    if (FORBIDDEN_KEYS.has(normalizedKey(key))) throw new Error("OAuth response contained a forbidden credential field");
    assertNoCredentialFields(nested);
  }
}

async function readJson(response: Response): Promise<unknown> {
  try {
    return await response.json();
  } catch {
    throw new Error("OAuth service returned invalid JSON");
  }
}

function authorizationUrl(provider: WebProvider, value: unknown): string {
  if (!isObject(value) || typeof value.authorization_url !== "string") {
    throw new Error("OAuth service did not return an authorization URL");
  }
  assertNoCredentialFields(value);
  let url: URL;
  try { url = new URL(value.authorization_url); } catch { throw new Error("OAuth service returned an invalid authorization URL"); }
  if (url.protocol !== "https:" || url.hostname !== AUTH_HOSTS[provider]) {
    throw new Error("OAuth service returned an untrusted authorization URL");
  }
  return url.href;
}

async function requestJson(apiBase: URL, path: string, init?: RequestInit): Promise<unknown> {
  const url = new URL(apiBase.href);
  url.pathname = `${apiBase.pathname.replace(/\/+$/, "")}/${path.replace(/^\/+/, "")}`;
  const response = await fetch(url, {
    ...init,
    credentials: "include",
    headers: { Accept: "application/json", ...(init?.headers ?? {}) },
  });
  if (!response.ok) {
    if (response.status === 404) return undefined;
    throw new Error(`OAuth service request failed (${response.status})`);
  }
  return response.status === 204 ? null : readJson(response);
}

function register(provider: WebProvider, apiBase: URL): void {
  registerProviderAuthAdapter({
    provider,
    beginAuthorization: async () => authorizationUrl(provider, await requestJson(apiBase, `/api/oauth/${provider}/begin`, { method: "POST" })),
    getStatus: async () => {
      const value = await requestJson(apiBase, `/api/oauth/${provider}/status`);
      if (value === null || value === undefined) return null;
      assertNoCredentialFields(value);
      if (!isObject(value)) throw new Error("OAuth service returned an invalid account response");
      return normalizeOAuthAccount({
        provider,
        subject: typeof value.subject === "string" ? value.subject : "",
        displayName: typeof value.displayName === "string" ? value.displayName : null,
        avatarUrl: typeof value.avatarUrl === "string" ? value.avatarUrl : null,
        capabilities: Array.isArray(value.capabilities) ? value.capabilities.filter((item): item is string => typeof item === "string") : [],
        connectedAt: typeof value.connectedAt === "string" ? value.connectedAt : undefined,
      });
    },
    revoke: async () => {
      await requestJson(apiBase, `/api/oauth/${provider}`, { method: "DELETE" });
    },
  });
}

/** Browser OAuth is enabled only as a server-session adapter. */
export function registerWebProviderAdapters(): void {
  if (platform !== "web") return;
  const apiBase = configuredApiBase();
  if (!apiBase) return;
  for (const provider of WEB_PROVIDERS) register(provider, apiBase);
}
