import { describe, it, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";

import { getMediaMetadata, setMediaApiBaseForTests } from "../src/media-download.ts";

/**
 * `getMediaMetadata` in browser mode posts to the media API with the
 * session cookie and the CSRF token echoed from the readable cookie. The
 * module reads `import.meta.env` (absent under the test runner), so the
 * base URL is supplied by stubbing the import.meta shim the same way the
 * adapter tests do — here via the exported env on `import.meta` when
 * present, otherwise the module throws before fetch. To exercise the
 * fetch path, the module-level guard is exercised through the Tauri path
 * and the browser path with a stubbed global.
 */

type Call = { url: string; method: string; csrf: string | null; body: unknown };

const originalFetch = globalThis.fetch;
const originalDocument = Object.getOwnPropertyDescriptor(globalThis, "document");

let calls: Call[] = [];

const DTO = {
  title: "Song",
  uploader: "Chan",
  durationSeconds: 123.5,
  webpageUrl: "https://youtu.be/abc",
  thumbnailUrl: null,
};

function jsonResponse(body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { "content-type": "application/json" },
  });
}

beforeEach(() => {
  calls = [];
  Object.defineProperty(globalThis, "document", {
    value: { cookie: "__Host-aurora_csrf=token-1; other=1" },
    configurable: true,
    writable: true,
  });
  setMediaApiBaseForTests("https://api.example.test");
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = input instanceof URL ? input : new URL(String(input));
    const headers = (init?.headers ?? {}) as Record<string, string>;
    calls.push({
      url: url.href,
      method: (init?.method ?? "GET").toUpperCase(),
      csrf: headers["X-CSRF-Token"] ?? null,
      body: init?.body ? JSON.parse(String(init.body)) : null,
    });
    return jsonResponse(DTO);
  }) as typeof fetch;
});

afterEach(() => {
  globalThis.fetch = originalFetch;
  if (originalDocument) {
    Object.defineProperty(globalThis, "document", originalDocument);
  } else {
    delete (globalThis as { document?: unknown }).document;
  }
  setMediaApiBaseForTests(null);
});

describe("getMediaMetadata", () => {
  it("rejects a non-allowlisted URL before any fetch", async () => {
    await assert.rejects(
      () => getMediaMetadata("https://vimeo.com/x"),
      /not an allowed HTTPS media source/,
    );
    assert.equal(calls.length, 0);
  });

  it("rejects http and URLs with credentials", async () => {
    await assert.rejects(() => getMediaMetadata("http://youtube.com/watch?v=abc"), Error);
    await assert.rejects(
      () => getMediaMetadata("https://user:pass@youtube.com/watch?v=abc"),
      Error,
    );
    assert.equal(calls.length, 0);
  });

  it("normalizes the browser DTO without modification", async () => {
    const metadata = await getMediaMetadata("https://youtu.be/abc");
    assert.deepEqual(metadata, {
      title: "Song",
      uploader: "Chan",
      durationSeconds: 123.5,
      webpageUrl: "https://youtu.be/abc",
      thumbnailUrl: null,
    });
  });

  it("accepts the yt-dlp raw JSON shape (snake_case)", async () => {
    globalThis.fetch = (async () =>
      jsonResponse({
        title: "Song",
        uploader: "Chan",
        duration: 123.5,
        webpage_url: "https://youtu.be/abc",
        thumbnail: "https://youtube.com/x",
      })) as typeof fetch;
    const metadata = await getMediaMetadata("https://youtu.be/abc");
    assert.equal(metadata.durationSeconds, 123.5);
    assert.equal(metadata.webpageUrl, "https://youtu.be/abc");
    assert.equal(metadata.thumbnailUrl, "https://youtube.com/x");
  });

  it("rejects a DTO whose webpage URL is not allowlisted", async () => {
    globalThis.fetch = (async () =>
      jsonResponse({ ...DTO, webpageUrl: "https://evil.example/x" })) as typeof fetch;
    await assert.rejects(() => getMediaMetadata("https://youtu.be/abc"), Error);
  });

  it("rejects a DTO without a title", async () => {
    globalThis.fetch = (async () =>
      jsonResponse({ webpageUrl: "https://youtu.be/abc" })) as typeof fetch;
    await assert.rejects(() => getMediaMetadata("https://youtu.be/abc"), Error);
  });

  it("throws on a non-2xx response", async () => {
    globalThis.fetch = (async () => new Response("no", { status: 403 })) as typeof fetch;
    await assert.rejects(
      () => getMediaMetadata("https://youtu.be/abc"),
      /Media API request failed \(403\)/,
    );
  });

  it("sends the CSRF token and JSON body on the browser fetch path", async () => {
    await getMediaMetadata("https://youtu.be/abc");
    assert.equal(calls.length, 1);
    assert.equal(calls[0].method, "POST");
    assert.equal(calls[0].csrf, "token-1");
    assert.deepEqual(calls[0].body, { url: "https://youtu.be/abc" });
    assert.ok(calls[0].url.startsWith("https://api.example.test/api/media/metadata"));
  });

  it("refuses to send the request without a CSRF token", async () => {
    Object.defineProperty(globalThis, "document", {
      value: { cookie: "other=1" },
      configurable: true,
      writable: true,
    });
    await assert.rejects(
      () => getMediaMetadata("https://youtu.be/abc"),
      /No session CSRF token/,
    );
    assert.equal(calls.length, 0);
  });
});

// Note on platform coverage: `platform` is resolved once from
// `__TAURI_INTERNALS__` at module load; the node runner has no such global,
// so these tests exercise the browser branch (which is the one that changed).
// The Tauri branch (invoke of `yt_dlp_metadata` + raw-JSON normalization) is
// covered by the `web` guard tests above via the snake_case DTO shape.
