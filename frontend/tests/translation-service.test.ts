import { describe, it } from "node:test";
import assert from "node:assert/strict";
import {
  translateLyrics,
  lyricsToTranslateLines,
  translationEndpoint,
  translationEnabled,
  DEFAULT_TRANSLATION_ENDPOINT,
} from "../src/translation-service.ts";

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
}

describe("translation service", () => {
  it("translates line-by-line and caches per session", async () => {
    let calls = 0;
    const fetchFn = (async (input: RequestInfo | URL, init?: RequestInit) => {
      calls += 1;
      const body = JSON.parse(String(init?.body)) as { q: string[]; target: string };
      assert.deepEqual(body.q, ["hello", "world"]);
      assert.equal(body.source, "auto");
      assert.equal(body.format, "text");
      assert.equal(String(input), "https://example.test/translate");
      return jsonResponse({ translatedText: ["привет", "мир"] });
    }) as typeof fetch;
    const out = await translateLyrics(["hello", "world"], "ru", { fetchFn, endpoint: "https://example.test/translate" });
    assert.deepEqual(out, ["привет", "мир"]);
    // Session cache: a second identical request does not hit the network.
    const again = await translateLyrics(["hello", "world"], "ru", { fetchFn, endpoint: "https://example.test/translate" });
    assert.deepEqual(again, ["привет", "мир"]);
    assert.equal(calls, 1);
    // A different target is a different cache key.
    const other = await translateLyrics(["hello", "world"], "de", { fetchFn, endpoint: "https://example.test/translate" });
    assert.equal(calls, 2);
    assert.deepEqual(other, ["привет", "мир"]);
  });

  it("batches at most 20 lines and rejects mismatched line counts", async () => {
    const seenBatches: string[][] = [];
    const fetchFn = (async (_input: RequestInfo | URL, init?: RequestInit) => {
      const body = JSON.parse(String(init?.body)) as { q: string[] };
      seenBatches.push(body.q);
      return jsonResponse({ translatedText: body.q.map((line) => `«${line}»`) });
    }) as typeof fetch;
    const lines = Array.from({ length: 45 }, (_, i) => `line ${i + 1}`);
    const out = await translateLyrics(lines, "en", { fetchFn, endpoint: "https://example.test/translate" });
    assert.deepEqual(seenBatches.map((b) => b.length), [20, 20, 5]);
    assert.deepEqual(out, lines.map((l) => `«${l}»`));

    const bad = (async () => jsonResponse({ translatedText: ["only one"] })) as typeof fetch;
    await assert.rejects(
      () => translateLyrics(["a", "b"], "en", { fetchFn: bad, endpoint: "https://example.test/translate" }),
      /mismatched line count/,
    );
  });

  it("retries once on failure and surfaces persistent errors", async () => {
    let attempts = 0;
    const flaky = (async () => {
      attempts += 1;
      if (attempts === 1) throw new Error("boom");
      return jsonResponse({ translatedText: ["ok"] });
    }) as typeof fetch;
    assert.deepEqual(await translateLyrics(["hi"], "ru", { fetchFn: flaky, endpoint: "https://e.test/translate" }), ["ok"]);
    assert.equal(attempts, 2);

    let deadAttempts = 0;
    const dead = (async () => {
      deadAttempts += 1;
      throw new Error("network down");
    }) as typeof fetch;
    await assert.rejects(
      () => translateLyrics(["hi again"], "ru", { fetchFn: dead, endpoint: "https://e.test/translate" }),
      /network down/,
    );
    assert.equal(deadAttempts, 2); // one retry, then the error surfaces
  });

  it("rejects string-collapse responses and HTTP errors", async () => {
    const collapse = (async () => jsonResponse({ translatedText: "one big string" })) as typeof fetch;
    await assert.rejects(
      () => translateLyrics(["a", "b"], "ru", { fetchFn: collapse, endpoint: "https://e.test/translate" }),
      /per-line/,
    );
    const httpError = (async () => jsonResponse({ translatedText: ["x"] }, 500)) as typeof fetch;
    await assert.rejects(
      () => translateLyrics(["a"], "ru", { fetchFn: httpError, endpoint: "https://e.test/translate" }),
      /HTTP 500/,
    );
  });

  it("defaults are off and point at disroot; settings are opt-in via localStorage", () => {
    // Node has no localStorage — these functions must not throw and must
    // report the safe defaults.
    assert.equal(translationEnabled(), false);
    assert.equal(translationEndpoint(), DEFAULT_TRANSLATION_ENDPOINT);
    assert.equal(DEFAULT_TRANSLATION_ENDPOINT, "https://translate.disroot.org/translate");
  });

  it("strips LRC timestamps for translation and keeps placeholders aligned", () => {
    assert.deepEqual(lyricsToTranslateLines("[00:01.00]hello\n[00:02]world"), ["hello", "world"]);
    assert.deepEqual(lyricsToTranslateLines("a\n\nb"), ["a", "♪", "b"]);
  });
});
