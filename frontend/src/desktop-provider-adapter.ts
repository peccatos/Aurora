import { invoke, platform } from "./invoke";
import { registerProviderAuthAdapter } from "./accounts";
import type { OAuthAccount } from "./oauth";
import type { ProviderId } from "./provider-registry";

/** Native-only bridge. The browser never receives OAuth codes or tokens. */
export function registerDesktopProviderAdapters(): void {
  if (platform !== "tauri") return;
  for (const provider of ["spotify", "youtube-music"] as const) {
    registerProviderAuthAdapter({
      provider,
      beginAuthorization: async () => {
        const result = await invoke<{ authorization_url: string }>("oauth_begin", { provider });
        if (!result?.authorization_url) throw new Error("Native OAuth did not provide an authorization URL");
        return result.authorization_url;
      },
      getStatus: async () => {
        // The native runtime returns sanitized identity only; no credential
        // fields are part of this wire contract.
        await invoke<OAuthAccount | null>("oauth_poll", { provider });
        return invoke<OAuthAccount | null>("provider_account_status", { provider });
      },
      revoke: async (accountId?: string) => {
        await invoke("provider_disconnect", { provider, accountId: accountId ?? null });
      },
    });
  }
}

export function isDesktopProvider(provider: string): provider is ProviderId {
  return platform === "tauri" && (provider === "spotify" || provider === "youtube-music");
}
