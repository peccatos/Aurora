/** Minimal hash router: each page owns a route id and a mount function.
 *  #/player, #/queue, #/library, #/eq, #/plugins, #/settings */

export interface Page {
  id: string;
  title: string;
  mount: (root: HTMLElement) => void | Promise<void>;
  /** Re-render on state change while visible. */
  update?: (root: HTMLElement) => void;
}

const pages = new Map<string, Page>();
let activeId = "player";
let root: HTMLElement | null = null;
let navEl: HTMLElement | null = null;
let onRouteChange: (() => void) | null = null;

export function registerPage(page: Page) {
  pages.set(page.id, page);
}

export function activePage(): string {
  return activeId;
}

export function setRouteChangeListener(fn: () => void) {
  onRouteChange = fn;
}

function parseHash(): string {
  const h = window.location.hash.replace(/^#\/?/, "").split("?")[0];
  return h || "player";
}

function render() {
  if (!root) return;
  const page = pages.get(activeId);
  if (!page) return;
  document.querySelectorAll(".nav-item").forEach((n) => {
    const el = n as HTMLElement;
    el.classList.toggle("active", el.dataset.route === activeId);
  });
  root.innerHTML = "";
  void page.mount(root);
  if (navEl) {
    const label = navEl.querySelector(".route-title");
    if (label) label.textContent = page.title;
  }
  onRouteChange?.();
}

export function navigate(id: string) {
  if (!pages.has(id)) return;
  window.location.hash = `#/${id}`;
}

export function startRouter(rootEl: HTMLElement, nav: HTMLElement) {
  root = rootEl;
  navEl = nav;
  activeId = parseHash();
  window.addEventListener("hashchange", () => {
    const next = parseHash();
    if (next !== activeId) {
      activeId = next;
      render();
    }
  });
  render();
}

/** Re-run the active page's update hook (state pushed a change). */
export function refreshActive() {
  if (!root) return;
  const page = pages.get(activeId);
  if (page?.update) page.update(root);
}
