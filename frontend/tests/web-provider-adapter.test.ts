import { describe, it, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";

import {
  fetchRemotePlaylistSnapshot,
  getRemotePlaylist,
  listRemotePlaylists,
  setApiBaseForTests,
} from "../src/web-provider-adapter.ts";

/** The adapter reads `import.meta.env` and `document.cookie`, neither of
 *  which exists under the test runner. `setApiBaseForTests` supplies the
 *  base URL; `document` is stubbed so the CSRF-cookie read path is the
 *  real one. */

const API_BASE = "https://api.example.test";

type Call = { url: string; method: string; csrf: string | null };

let calls: Call[] = [];
let handler: (url: URL, init: RequestInit | undefined) => Response | Promise<Response>;

function jsonResponse(body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { "content-type": "application/json" },
  });
}

const originalFetch = globalThis.fetch;
const originalDocument = Object.getOwnPropertyDescriptor(globalThis, "document");

function installEnv(): void {
  Object.defineProperty(globalThis, "document", {
    value: { cookie: "__Host-aurora_csrf=token-1; other=1" },
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
  installEnv();
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = input instanceof URL ? input : new URL(String(input));
    const headers = (init?.headers ?? {}) as Record<string, string>;
    calls.push({
      url: url.href,
      method: (init?.method ?? "GET").toUpperCase(),
      csrf: headers["X-CSRF-Token"] ?? null,
    });
    return handler(url, init);
  }) as typeof fetch;
});

afterEach(() => {
  globalThis.fetch = originalFetch;
  restoreEnv();
});

void describe("browser remote playlist listing", () => {
  it("maps a Spotify page to provider summaries and keeps the cursor", async () => {
    handler = () =>
      jsonResponse({
        items: [
          {
            id: "pl-1",
            name: "Road trip",
            owner: { id: "owner-1" },
            snapshot_id: "snap-1",
            tracks: { total: 12, items: [] },
          },
        ],
        next: "https://api.spotify.com/v1/me/playlists?offset=20",
      });

    const page = await listRemotePlaylists("spotify");
    assert.deepEqual(page.items, [
      { provider: "spotify", id: "pl-1", name: "Road trip", trackCount: 12, revision: "snap-1" },
    ]);
    assert.equal(page.nextCursor, "20");
    assert.equal(calls[0].url, `${API_BASE}/api/oauth/spotify/playlists`);
    // Playlist reads are GET, so no CSRF header is required.
    assert.equal(calls[0].method, "GET");
  });

  it("maps a YouTube Music page to provider summaries", async () => {
    handler = () =>
      jsonResponse({
        items: [
          {
            id: "PL-1",
            snippet: { title: "Focus" },
            contentDetails: { itemCount: 3 },
          },
        ],
        nextPageToken: "CAoQAA",
      });

    const page = await listRemotePlaylists("youtube-music");
    assert.deepEqual(page.items, [
      { provider: "youtube-music", id: "PL-1", name: "Focus", trackCount: 3, revision: null },
    ]);
    assert.equal(page.nextCursor, "CAoQAA");
  });
});

void describe("browser remote playlist snapshot", () => {
  it("preserves provider order and numbers positions from zero", async () => {
    handler = () =>
      jsonResponse({
        name: "Road trip",
        revision: "rev-9",
        tracks: [
          { id: "t1", title: "One", artists: ["A"], album: "Album", durationMs: 1000 },
          { id: "t2", title: "Two", artists: ["B", "C"], album: null, durationMs: null },
        ],
      });

    const snapshot = await getRemotePlaylist("spotify", "pl-1");
    assert.equal(snapshot.name, "Road trip");
    assert.equal(snapshot.revision, "rev-9");
    assert.equal(snapshot.trackCount, 2);
    assert.deepEqual(
      snapshot.tracks.map((track) => [track.position, track.remoteId, track.title, track.artists]),
      [
        [0, "t1", "One", ["A"]],
        [1, "t2", "Two", ["B", "C"]],
      ],
    );
  });

  it("accepts wrapped track entries and artist objects", async () => {
    handler = () =>
      jsonResponse({
        tracks: [
          { track: { id: "t1", name: "One", artists: [{ name: "A" }] } },
          { item: { id: "t2", title: "Two", artists: ["B"] } },
        ],
      });

    const snapshot = await getRemotePlaylist("spotify", "pl-1");
    assert.deepEqual(
      snapshot.tracks.map((track) => [track.remoteId, track.title, track.artists]),
      [
        ["t1", "One", ["A"]],
        ["t2", "Two", ["B"]],
      ],
    );
  });

  it("rejects an invalid playlist id before any request", async () => {
    handler = () => jsonResponse({});
    await assert.rejects(getRemotePlaylist("spotify", "bad/id"), /Invalid playlist ID/);
    assert.equal(calls.length, 0);
  });

  it("reports a missing snapshot as an invalid snapshot, not a silent empty one", async () => {
    handler = () => new Response("", { status: 404 });
    await assert.rejects(getRemotePlaylist("spotify", "pl-1"), /invalid playlist snapshot/i);
  });

  it("rejects a snapshot that carries credential fields", async () => {
    handler = () =>
      jsonResponse({
        name: "Leaky",
        tracks: [{ id: "t1", title: "One", accessToken: "secret" }],
      });
    await assert.rejects(getRemotePlaylist("spotify", "pl-1"), /forbidden credential field/);
  });

  it("walks the cursor across pages and re-numbers positions contiguously", async () => {
    handler = (url) => {
      const cursor = url.searchParams.get("cursor");
      if (!cursor) {
        return jsonResponse({
          name: "Paged",
          tracks: [{ id: "t1", title: "One", artists: ["A"] }],
          nextCursor: "page-2",
        });
      }
      assert.equal(cursor, "page-2");
      return jsonResponse({
        name: "Paged",
        tracks: [{ id: "t2", title: "Two", artists: ["B"] }],
      });
    };

    const snapshot = await fetchRemotePlaylistSnapshot("spotify", "pl-1", { pageSize: 1 });
    assert.equal(calls.length, 2);
    assert.equal(calls[1].url, `${API_BASE}/api/oauth/spotify/playlists/pl-1?cursor=page-2`);
    assert.deepEqual(
      snapshot.tracks.map((track) => [track.position, track.remoteId]),
      [
        [0, "t1"],
        [1, "t2"],
      ],
    );
    assert.equal(snapshot.trackCount, 2);
  });

  it("stops on a repeated cursor instead of looping forever", async () => {
    // A provider that echoes the cursor it was handed back as the *next*
    // cursor would otherwise be walked forever. Page 1 (no cursor) and
    // page 2 ("same") are distinct pages; fetching "same" a second time
    // is what must never happen.
    handler = (url) =>
      jsonResponse({
        name: "Sticky",
        tracks: [{ id: "t1", title: "One", artists: ["A"] }],
        nextCursor: url.searchParams.get("cursor") ?? "same",
      });

    const snapshot = await fetchRemotePlaylistSnapshot("spotify", "pl-1");
    assert.equal(calls.length, 2, "the loop stops at the repeated cursor");
    assert.deepEqual(
      calls.map((call) => new URL(call.url).searchParams.get("cursor")),
      [null, "same"],
      "each cursor is requested at most once",
    );
    assert.deepEqual(
      snapshot.tracks.map((track) => [track.position, track.remoteId]),
      [
        [0, "t1"],
        [1, "t1"],
      ],
      "both distinct pages are appended exactly once",
    );
  });
});
