import { describe, it, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";

import { getMe, login, logout, register } from "../src/auth-service.ts";
import { setApiBaseForTests } from "../src/web-provider-adapter.ts";

/** Same environment pattern as web-provider-adapter.test.ts: no real
 *  browser, so `document` (cookie jar) is stubbed and fetch is mocked.
 *  The stubbed `document.cookie` behaves like the real one for the plain
 *  `name=value; …` writes the service performs. */

const API_BASE = "https://api.example.test";

type Call = { url: string; method: string; headers: Record<string, string>; body: string | null };

let calls: Call[] = [];
let handler: (url: URL, init: RequestInit | undefined) => Response | Promise<Response>;
let cookies: Record<string, string>;

function response(body: unknown, status = 200): Response {
  return new Response(body === null ? null : JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

const originalFetch = globalThis.fetch;
const originalDocument = Object.getOwnPropertyDescriptor(globalThis, "document");

function installEnv(): void {
  cookies = {};
  Object.defineProperty(globalThis, "document", {
    value: {
      get cookie(): string {
        return Object.entries(cookies).map(([k, v]) => `${k}=${v}`).join("; ");
      },
      set cookie(raw: string) {
        const [pair] = raw.split(";");
        const eq = pair.indexOf("=");
        if (eq > 0) {
          const name = pair.slice(0, eq).trim();
          const value = pair.slice(eq + 1).trim();
          if (value === "") delete cookies[name];
          else cookies[name] = value;
        }
      },
    },
    configurable: true,
    writable: true,
  });
  setApiBaseForTests(API_BASE);
}

function restoreEnv(): void {
  if (originalDocument) {
    Object.defineProperty(globalThis, "document", originalDocument);
  } else {
    delete (globalThis as { document?: unknown }).document;
  }
  setApiBaseForTests(null);
}

beforeEach(() => {
  calls = [];
  cookies = {};
  installEnv();
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = input instanceof URL ? input : new URL(String(input));
    const headers = Object.fromEntries(
      Object.entries((init?.headers ?? {}) as Record<string, string>),
    );
    calls.push({
      url: url.href,
      method: (init?.method ?? "GET").toUpperCase(),
      headers,
      body: typeof init?.body === "string" ? init.body : null,
    });
    return handler(url, init);
  }) as typeof fetch;
});

afterEach(() => {
  globalThis.fetch = originalFetch;
  restoreEnv();
});

function callTo(path: string): Call | undefined {
  return calls.find((c) => c.url === `${API_BASE}${path}`);
}

void describe("auth service register", () => {
  it("stores the response CSRF token in the readable cookie and returns the identity", async () => {
    handler = () => response({ email: "you@example.com", csrfToken: "csrf-42" }, 201);
    const identity = await register("you@example.com", "longenoughpw");
    assert.deepEqual(identity, { email: "you@example.com", csrfToken: "csrf-42" });
    assert.equal(cookies["__Host-aurora_csrf"], "csrf-42");
    const call = callTo("/api/auth/register");
    assert.ok(call);
    assert.equal(call!.method, "POST");
    assert.equal(call!.headers["Origin"], API_BASE);
    assert.deepEqual(JSON.parse(call!.body!), { email: "you@example.com", password: "longenoughpw" });
  });

  it("surfaces 409 as a typed email-taken error", async () => {
    handler = () => response(null, 409);
    await assert.rejects(register("taken@example.com", "longenoughpw"), (error: Error & { code?: string }) => {
      assert.equal(error.code, "email-taken");
      return true;
    });
    assert.equal(cookies["__Host-aurora_csrf"], undefined);
  });

  it("surfaces 403 as registration-closed and 400 as validation", async () => {
    handler = () => response(null, 403);
    await assert.rejects(register("a@example.com", "longenoughpw"), (error: Error & { code?: string }) => {
      assert.equal(error.code, "registration-closed");
      return true;
    });
    handler = () => response(null, 400);
    await assert.rejects(register("bad", "short"), (error: Error & { code?: string }) => {
      assert.equal(error.code, "validation");
      return true;
    });
  });
});

void describe("auth service login", () => {
  it("returns the identity and echoes a fresh CSRF token on later calls", async () => {
    handler = () => response({ email: "you@example.com", csrfToken: "csrf-7" }, 200);
    const identity = await login("you@example.com", "longenoughpw");
    assert.equal(identity.email, "you@example.com");
    assert.equal(cookies["__Host-aurora_csrf"], "csrf-7");
    // A follow-up state-changing call picks the token up from the cookie.
    handler = () => response(null, 204);
    await logout();
    const logoutCall = callTo("/api/auth/logout");
    assert.ok(logoutCall);
    assert.equal(logoutCall!.method, "POST");
    assert.equal(logoutCall!.headers["X-CSRF-Token"], "csrf-7");
  });

  it("surfaces 401 as wrong-credentials without retrying", async () => {
    handler = () => response(null, 401);
    await assert.rejects(login("you@example.com", "wrongpassword"), (error: Error & { code?: string }) => {
      assert.equal(error.code, "wrong-credentials");
      return true;
    });
    // Exactly one request: a 401 from login is a real answer, not a
    // session-expiry signal to bootstrap on.
    assert.equal(calls.length, 1);
  });
});

void describe("auth service logout", () => {
  it("succeeds on 204 and clears the readable CSRF cookie", async () => {
    cookies["__Host-aurora_csrf"] = "csrf-7";
    handler = () => response(null, 204);
    await logout();
    assert.equal(cookies["__Host-aurora_csrf"], undefined);
  });

  it("surfaces failure as a typed error", async () => {
    cookies["__Host-aurora_csrf"] = "csrf-7";
    handler = () => response(null, 403);
    await assert.rejects(logout(), (error: Error & { code?: string; status?: number }) => {
      assert.equal(error.code, "server");
      assert.equal(error.status, 403);
      return true;
    });
  });
});

void describe("auth service getMe", () => {
  it("parses the authenticated shape", async () => {
    handler = () => response({ authenticated: true, email: "you@example.com" });
    assert.deepEqual(await getMe(), { authenticated: true, email: "you@example.com" });
    const call = callTo("/api/auth/me");
    assert.ok(call);
    assert.equal(call!.method, "GET");
    assert.equal(call!.headers["Origin"], API_BASE);
  });

  it("parses the anonymous shape", async () => {
    handler = () => response({ authenticated: false });
    assert.deepEqual(await getMe(), { authenticated: false });
  });

  it("returns null on network failure and non-200", async () => {
    handler = () => {
      throw new Error("offline");
    };
    assert.equal(await getMe(), null);
    handler = () => response(null, 500);
    assert.equal(await getMe(), null);
  });
});
