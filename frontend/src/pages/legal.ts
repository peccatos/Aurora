/** Legal page: renders privacy policy / terms of service (docs/legal/*.md),
 *  chosen via the ?doc= query in the hash route (#/legal?doc=privacy).
 *  No markdown library: headings / paragraphs / bullet lists are parsed
 *  line-by-line into DOM (textContent everywhere — the docs are ours, but
 *  the law is still DOM API). */

import type { Page } from "../router";
import { navigate } from "../router";
import privacyMd from "../../../docs/legal/privacy-policy.md?raw";
import termsMd from "../../../docs/legal/terms-of-service.md?raw";

type DocId = "privacy" | "tos";

const DOCS: Record<DocId, { title: string; md: string }> = {
  privacy: { title: "Политика конфиденциальности", md: privacyMd },
  tos: { title: "Условия использования", md: termsMd },
};

/** Current doc from the hash (e.g. "#/legal?doc=tos"). Defaults to privacy. */
function currentDoc(): DocId {
  const query = window.location.hash.split("?")[1] ?? "";
  const match = /(?:^|&)doc=([^&]*)/.exec(query);
  const value = match?.[1];
  return value === "tos" ? "tos" : "privacy";
}

/** Strip a leading "#" run, inline `code` backticks and **bold** markers —
 *  the md source uses them for emphasis only. */
function plainText(line: string): string {
  return line
    .replace(/^#{1,6}\s*/, "")
    .replace(/\*\*(.+?)\*\*/g, "$1")
    .replace(/`([^`]+)`/g, "$1");
}

/** Minimal line-based markdown rendering (headings, bullets, paragraphs,
 *  blockquotes, table rows as plain text). */
function renderMarkdown(root: HTMLElement, md: string): void {
  const lines = md.split(/\r?\n/);
  const container = document.createElement("div");
  container.className = "legal-doc";
  let para: HTMLParagraphElement | null = null;

  const flush = () => {
    para = null;
  };

  for (const raw of lines) {
    const line = raw.trimEnd();
    if (!line.trim()) {
      flush();
      continue;
    }
    const heading = /^(#{1,3})\s+(.*)$/.exec(line);
    if (heading) {
      flush();
      const level = heading[1].length;
      const el = document.createElement(level === 1 ? "h2" : level === 2 ? "h3" : "h4");
      el.textContent = plainText(heading[2]);
      el.className = level === 1 ? "page-title" : "overline";
      container.appendChild(el);
      continue;
    }
    if (/^[-*]\s+/.test(line)) {
      flush();
      const item = document.createElement("p");
      item.className = "hint legal-bullet";
      item.textContent = `• ${plainText(line.replace(/^[-*]\s+/, ""))}`;
      container.appendChild(item);
      continue;
    }
    if (line.trim().startsWith(">")) {
      flush();
      const note = document.createElement("p");
      note.className = "hint warn";
      note.textContent = plainText(line.trim().replace(/^>\s*/, ""));
      container.appendChild(note);
      continue;
    }
    // Table rows and everything else: plain paragraphs.
    if (!para) {
      para = document.createElement("p");
      para.className = "hint";
      container.appendChild(para);
    }
    para.textContent = para.textContent
      ? `${para.textContent} ${plainText(line)}`
      : plainText(line);
  }
  root.appendChild(container);
}

function render(root: HTMLElement): void {
  const doc = DOCS[currentDoc()];
  root.innerHTML = `
    <div class="ytm-shelf-head">
      <button class="ytm-chip" id="legal-back" type="button">← Настройки</button>
      <span class="ytm-shelf-title">${doc.title}</span>
    </div>
    <div id="legal-content"></div>
  `;
  root.querySelector<HTMLButtonElement>("#legal-back")?.addEventListener("click", () =>
    navigate("settings"),
  );
  const content = root.querySelector<HTMLElement>("#legal-content");
  if (content) renderMarkdown(content, doc.md);
}

export const legalPage: Page = {
  id: "legal",
  title: "Legal",
  mount: render,
};
