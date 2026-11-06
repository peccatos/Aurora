/** Provider-neutral OAuth contracts and browser-safe protocol helpers.
 *
 * This module deliberately stops at the authorization boundary: it does not
 * store tokens, perform provider calls, or imply that a provider is connected.
 */

export type OAuthProviderId = string;

export type OAuthCapability =
  | "identity"
  | "playlists_read"
  | "playlists_write"
  | "library_read"
  | "library_write"
  | "playback";

export interface OAuthAccount {
  provider: OAuthProviderId;
  subject: string;
  displayName: string;
  avatarUrl: string | null;
  capabilities: readonly OAuthCapability[];
  connectedAt: string;
}

export interface OAuthState {
  value: string;
  createdAt: number;
  expiresAt: number;
}

export interface PkcePair {
  verifier: string;
  challenge: string;
  method: "S256";
}

/** Ephemeral material needed to validate and finish one authorization attempt. */
export interface PendingOAuthSession {
  provider: OAuthProviderId;
  state: OAuthState;
  pkce: PkcePair;
  redirectUri: string;
  createdAt: number;
  expiresAt: number;
}

export interface OAuthCallbackSuccess { kind: "success"; code: string; state: string; }
export interface OAuthCallbackError { kind: "error"; error: string; description: string | null; state: string | null; }
export type OAuthCallbackResult = OAuthCallbackSuccess | OAuthCallbackError;

/** In-memory only by design; never persist PKCE verifiers or authorization codes. */
export class OAuthPendingSessionStore {
  private readonly sessions = new Map<string, PendingOAuthSession>();

  async begin(provider: string, redirectUri: string, ttlMs = 10 * 60 * 1000, now = Date.now()): Promise<PendingOAuthSession> {
    const normalizedProvider = assertNonEmpty(provider, "provider", MAX_PROVIDER_LENGTH).toLowerCase();
    const redirect = new URL(redirectUri);
    const localHttp = redirect.protocol === "http:" &&
      (redirect.hostname === "localhost" || redirect.hostname === "127.0.0.1" || redirect.hostname === "[::1]");
    if (redirect.protocol !== "https:" && !localHttp) throw new Error("Invalid OAuth redirect URI");
    const state = createOAuthState(ttlMs, now);
    const pkce = await createPkcePair();
    const session: PendingOAuthSession = { provider: normalizedProvider, state, pkce, redirectUri: redirect.href, createdAt: now, expiresAt: state.expiresAt };
    this.sessions.set(state.value, session);
    return session;
  }

  consume(stateValue: string, now = Date.now()): PendingOAuthSession | null {
    const session = this.sessions.get(stateValue);
    if (!session) return null;
    this.sessions.delete(stateValue);
    return validateOAuthState(session.state, stateValue, now) ? session : null;
  }

  clearExpired(now = Date.now()): number {
    let removed = 0;
    for (const [key, session] of this.sessions) {
      if (now > session.expiresAt) { this.sessions.delete(key); removed += 1; }
    }
    return removed;
  }
}

/** Parse callback parameters without exchanging codes or persisting credentials. */
export function parseOAuthCallback(url: string | URL): OAuthCallbackResult {
  const params = new URL(url).searchParams;
  const state = params.get("state");
  const error = params.get("error");
  if (error) return { kind: "error", error, description: params.get("error_description"), state };
  const code = params.get("code");
  if (!code || !state) throw new Error("OAuth callback is missing code or state");
  return { kind: "success", code, state };
}

const STATE_BYTES = 32;
const VERIFIER_BYTES = 32;
const MAX_PROVIDER_LENGTH = 64;
const MAX_SUBJECT_LENGTH = 512;
const MAX_DISPLAY_NAME_LENGTH = 256;
const MAX_CAPABILITIES = 32;

function randomBase64Url(byteLength: number): string {
  const bytes = new Uint8Array(byteLength);
  crypto.getRandomValues(bytes);
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/g, "");
}

function assertNonEmpty(value: string, field: string, maxLength: number): string {
  const normalized = value.trim();
  if (!normalized || normalized.length > maxLength) {
    throw new Error(`Invalid OAuth ${field}`);
  }
  return normalized;
}

/** Create a high-entropy OAuth state value with an explicit expiry. */
export function createOAuthState(ttlMs = 10 * 60 * 1000, now = Date.now()): OAuthState {
  if (!Number.isFinite(ttlMs) || ttlMs <= 0 || ttlMs > 60 * 60 * 1000) {
    throw new Error("OAuth state TTL must be between 1ms and 1 hour");
  }
  return { value: randomBase64Url(STATE_BYTES), createdAt: now, expiresAt: now + ttlMs };
}

/**
 * Validate callback state without a timing-shortcut comparison. The caller
 * remains responsible for consuming the state exactly once after validation.
 */
export function validateOAuthState(expected: OAuthState, received: string, now = Date.now()): boolean {
  if (!received || !Number.isFinite(now) || now < expected.createdAt || now > expected.expiresAt) return false;
  const left = expected.value;
  if (left.length !== received.length) return false;
  let difference = 0;
  for (let index = 0; index < left.length; index += 1) {
    difference |= left.charCodeAt(index) ^ received.charCodeAt(index);
  }
  return difference === 0;
}

/** Build an RFC 7636 S256 verifier/challenge pair using Web Crypto. */
export async function createPkcePair(): Promise<PkcePair> {
  const verifier = randomBase64Url(VERIFIER_BYTES);
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(verifier));
  const bytes = new Uint8Array(digest);
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  const challenge = btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/g, "");
  return { verifier, challenge, method: "S256" };
}

/** Normalize an untrusted provider account response into the shared contract. */
export function normalizeOAuthAccount(input: {
  provider: string;
  subject: string;
  displayName?: string | null;
  avatarUrl?: string | null;
  capabilities?: readonly string[] | null;
  connectedAt?: string;
}): OAuthAccount {
  const capabilities = [...new Set((input.capabilities ?? []).filter(
    (value): value is OAuthCapability =>
      value === "identity" || value === "playlists_read" || value === "playlists_write" ||
      value === "library_read" || value === "library_write" || value === "playback",
  ))].slice(0, MAX_CAPABILITIES);
  const connectedAt = input.connectedAt ?? new Date().toISOString();
  if (Number.isNaN(Date.parse(connectedAt))) throw new Error("Invalid OAuth connectedAt");
  const avatarUrl = input.avatarUrl?.trim() || null;
  if (avatarUrl) {
    try {
      const parsed = new URL(avatarUrl);
      if (parsed.protocol !== "https:" && parsed.protocol !== "http:") throw new Error();
    } catch {
      throw new Error("Invalid OAuth avatar URL");
    }
  }
  return {
    provider: assertNonEmpty(input.provider, "provider", MAX_PROVIDER_LENGTH).toLowerCase(),
    subject: assertNonEmpty(input.subject, "subject", MAX_SUBJECT_LENGTH),
    displayName: assertNonEmpty(input.displayName ?? input.subject, "display name", MAX_DISPLAY_NAME_LENGTH),
    avatarUrl,
    capabilities,
    connectedAt: new Date(connectedAt).toISOString(),
  };
}

export function hasOAuthCapability(account: OAuthAccount, capability: OAuthCapability): boolean {
  return account.capabilities.includes(capability);
}
