/** Account metadata and provider authorization boundary.
 *
 * Only non-secret account metadata is persisted in the browser. Access and
 * refresh tokens belong to the native/backend secure-storage flow and never
 * cross this module's storage boundary.
 */

import type { OAuthAccount, OAuthCallbackResult } from "./oauth.ts";
import { normalizeOAuthAccount } from "./oauth.ts";
import { canonicalProviderId } from "./provider-registry.ts";

export type AccountConnectionState = "connected" | "disconnected" | "error";

export interface AccountRecord extends OAuthAccount {
  state: AccountConnectionState;
  lastError: string | null;
}

export interface ProviderAuthAdapter {
  readonly provider: string;
  beginAuthorization(): Promise<string>;
  getStatus(): Promise<OAuthAccount | null>;
  revoke(accountId?: string): Promise<void>;
}

const STORAGE_KEY = "aurora.account-metadata.v1";
const accounts = new Map<string, AccountRecord>();
const adapters = new Map<string, ProviderAuthAdapter>();

function providerKey(provider: string): string {
  return canonicalProviderId(provider);
}

function storage(): Storage | null {
  try { return typeof localStorage === "undefined" ? null : localStorage; } catch { return null; }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function load(): void {
  const raw = storage()?.getItem(STORAGE_KEY);
  if (!raw) return;
  try {
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return;
    for (const item of parsed) {
      if (!isRecord(item) || (item.state !== "connected" && item.state !== "disconnected" && item.state !== "error")) continue;
      try {
        const account = normalizeOAuthAccount(item as Parameters<typeof normalizeOAuthAccount>[0]);
        accounts.set(account.provider, { ...account, state: item.state, lastError: typeof item.lastError === "string" ? item.lastError : null });
      } catch { /* discard malformed metadata */ }
    }
  } catch { /* discard malformed storage */ }
}

function save(): void {
  // Deliberately serializes only AccountRecord fields; adapters and tokens are
  // never part of this object.
  storage()?.setItem(STORAGE_KEY, JSON.stringify([...accounts.values()]));
}

load();

export function listAccountRecords(): readonly AccountRecord[] {
  return [...accounts.values()];
}

export function getAccountRecord(provider: string): AccountRecord | null {
  return accounts.get(providerKey(provider)) ?? null;
}

export function registerProviderAuthAdapter(adapter: ProviderAuthAdapter): void {
  adapters.set(providerKey(adapter.provider), adapter);
}

export function hasProviderAuthAdapter(provider: string): boolean {
  return adapters.has(providerKey(provider));
}

/** Starts only a configured real adapter; absent adapters fail closed. */
export async function startAuthorization(provider: string): Promise<string> {
  const adapter = adapters.get(providerKey(provider));
  if (!adapter) throw new Error(`${provider} authorization is not configured`);
  return adapter.beginAuthorization();
}

/** Records a callback outcome without exchanging or storing credentials. */
export function recordCallback(provider: string, result: OAuthCallbackResult): void {
  const key = providerKey(provider);
  const existing = accounts.get(key);
  if (result.kind === "error") {
    if (existing) accounts.set(key, { ...existing, state: "error", lastError: result.description ?? result.error });
    save();
  }
}

export async function disconnectAccount(provider: string): Promise<void> {
  const key = providerKey(provider);
  const existing = accounts.get(key);
  if (!existing) return;
  const adapter = adapters.get(key);
  if (adapter) await adapter.revoke(existing.subject);
  accounts.delete(key);
  save();
}

/** Synchronize sanitized provider identity/status from the native runtime. */
export async function refreshAccountStatus(provider: string): Promise<AccountRecord | null> {
  const key = providerKey(provider);
  const adapter = adapters.get(key);
  if (!adapter) return accounts.get(key) ?? null;
  const account = await adapter.getStatus();
  if (!account) {
    accounts.delete(key);
    save();
    return null;
  }
  const sanitized = normalizeOAuthAccount({ ...account, provider: key });
  const record: AccountRecord = { ...sanitized, state: "connected", lastError: null };
  accounts.set(key, record);
  save();
  return record;
}

/** Records a native callback failure using sanitized text only. */
export function recordAuthorizationError(provider: string, message: string): void {
  const key = providerKey(provider);
  const existing = accounts.get(key);
  if (existing) accounts.set(key, { ...existing, state: "error", lastError: message.slice(0, 256) });
  save();
}

/** Test/backend bridge: persist sanitized identity metadata after secure exchange. */
export function recordConnectedAccount(input: Parameters<typeof normalizeOAuthAccount>[0]): AccountRecord {
  const account = normalizeOAuthAccount(input);
  const record: AccountRecord = { ...account, state: "connected", lastError: null };
  accounts.set(account.provider, record);
  save();
  return record;
}

export function clearAccountMetadataForTests(): void {
  accounts.clear();
  storage()?.removeItem(STORAGE_KEY);
}
