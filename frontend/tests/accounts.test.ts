import { describe, it } from "node:test";
import assert from "node:assert/strict";
import {
  clearAccountMetadataForTests,
  disconnectAccount,
  hasProviderAuthAdapter,
  recordCallback,
  recordConnectedAccount,
  registerProviderAuthAdapter,
  startAuthorization,
} from "../src/accounts.ts";

describe("account metadata boundary", () => {
  it("persists sanitized identity metadata but never requires tokens", () => {
    clearAccountMetadataForTests();
    const record = recordConnectedAccount({ provider: "spotify", subject: "u1", displayName: "Listener", capabilities: ["identity"] });
    assert.equal(record.state, "connected");
    assert.equal("accessToken" in record, false);
  });

  it("fails closed until a real provider adapter is registered", async () => {
    clearAccountMetadataForTests();
    assert.equal(hasProviderAuthAdapter("spotify"), false);
    await assert.rejects(startAuthorization("spotify"), /not configured/);
  });

  it("reports callback errors as metadata and revokes through the adapter", async () => {
    clearAccountMetadataForTests();
    let revoked = false;
    registerProviderAuthAdapter({ provider: "spotify", beginAuthorization: async () => "https://provider.example/auth", getStatus: async () => null, revoke: async () => { revoked = true; } });
    recordConnectedAccount({ provider: "spotify", subject: "u1", displayName: "Listener" });
    recordCallback("spotify", { kind: "error", error: "access_denied", description: "Denied", state: "state" });
    await disconnectAccount("spotify");
    assert.equal(revoked, true);
  });
});
