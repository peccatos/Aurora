import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";

/** Legal page sanity: the docs are non-empty and the page source is wired
 *  to the "legal" route. (The md?raw imports cannot run under node's type
 *  stripping, so the sources are read from disk instead.) */

const repoRoot = join(import.meta.dirname, "../..");
const privacyMd = readFileSync(join(repoRoot, "docs/legal/privacy-policy.md"), "utf8");
const termsMd = readFileSync(join(repoRoot, "docs/legal/terms-of-service.md"), "utf8");
const legalPageSource = readFileSync(join(repoRoot, "frontend/src/pages/legal.ts"), "utf8");

describe("legal documents", () => {
  it("privacy policy markdown is non-empty and dated", () => {
    assert.ok(privacyMd.trim().length > 500, "privacy policy looks truncated");
    assert.ok(privacyMd.includes("2026-11-09"), "last-updated date missing");
  });

  it("terms of service markdown is non-empty and dated", () => {
    assert.ok(termsMd.trim().length > 500, "terms look truncated");
    assert.ok(termsMd.includes("2026-11-09"), "last-updated date missing");
  });

  it("legal page module registers route id 'legal' and imports both docs", () => {
    assert.match(legalPageSource, /id:\s*"legal"/);
    assert.match(legalPageSource, /privacy-policy\.md\?raw/);
    assert.match(legalPageSource, /terms-of-service\.md\?raw/);
  });
});
