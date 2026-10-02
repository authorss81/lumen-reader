import { api, el, formatDate, type BookMeta } from "./api";
import { closeOverlays, Library, promptModal } from "./library";
import { Reader, excerptHtml, flatToc, parseLocator, type PanelMode } from "./reader";
import {
  applyTheme,
  effectiveTheme,
  loadSettings,
  watchSystemTheme,
  openSettingsSheet,
  type AppSettings,
} from "./settings";

/* --------------------------------------------------------------- helpers */

function $<T extends HTMLElement>(id: string): T {
  const node = document.getElementById(id);
  if (!node) throw new Error(`missing element #${id}`);
  return node as T;
}

let toastTimer: number | undefined;
function toast(message: string) {
  const node = $("toast");
  node.textContent = message;
  node.hidden = false;
  if (toastTimer) window.clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => {
    node.hidden = true;
  }, 3400);
}

let loadingDepth = 0;
function busy(text?: string) {
  loadingDepth += 1;
  const node = $("loading");
  if (text) $("loading-text").textContent = text;
  node.hidden = false;
}
function idle() {
  loadingDepth = Math.max(0, loadingDepth - 1);
  if (loadingDepth === 0) $("loading").hidden = true;
}

/* ------------------------------------------------------------------ state */

let settings: AppSettings;
let library: Library;
let reader: Reader;
let books: BookMeta[] = [];
let panelMode: PanelMode | null = null;
let searchTimer: number | undefined;
let searchResults: Awaited<ReturnType<Reader["search"]>> = [];

const libraryEl = $<HTMLElement>("library");
const readerEl = $<HTMLElement>("reader");
const panelEl = $<HTMLElement>("panel");
const scrimEl = $<HTMLElement>("scrim");
const panelInput = $<HTMLInputElement>("panel-input");
let panelFilter = "";

/* ---------------------------------------------------------------- library */

async function refreshLibrary() {
  books = await api.listBooks();
  library.setBooks(books);
}

function showLibrary() {
  closeOverlays();
  closePanel();
  reader.flushSave();
  libraryEl.hidden = false;
  readerEl.hidden = true;
  $("topbar").hidden = true;
  void refreshLibrary();
}

async function openBook(book: BookMeta) {
  busy("Opening…");
  try {
    const detail = await api.openBook(book.id);
    closePanel();
    closeOverlays();
    libraryEl.hidden = true;
    readerEl.hidden = false;
    $("topbar").hidden = false;
    $("topbar-book").textContent = detail.title;
    await reader.open(detail);
    void refreshLibrary();
  } catch (error) {
    toast(String(error));
  } finally {
    idle();
  }
}

/* ------------------------------------------------------------------ panel */

function setPanel(mode: PanelMode | null) {
  panelMode = mode;
  const tabs = $("panel-tabs");
  const body = $("panel-body");

  if (!mode) {
    panelEl.hidden = true;
    scrimEl.hidden = true;
    body.replaceChildren();
    tabs.replaceChildren();
    panelInput.value = "";
    panelFilter = "";
    for (const id of ["btn-toc", "btn-search", "btn-bookmarks", "btn-notes"]) {
      $(id).classList.remove("active");
    }
    return;
  }

  panelEl.hidden = false;
  scrimEl.hidden = false;
  const modes: PanelMode[] = ["toc", "search", "bookmarks", "notes"];
  const labels: Record<PanelMode, string> = {
    toc: "Contents",
    search: "Search",
    bookmarks: "Bookmarks",
    notes: "Notes",
  };
  const placeholders: Record<PanelMode, string> = {
    toc: "Filter contents",
    search: "Search this book",
    bookmarks: "Filter bookmarks",
    notes: "Filter notes",
  };
  tabs.replaceChildren();
  for (const m of modes) {
    const tab = el("button", "tab", labels[m]);
    tab.type = "button";
    if (m === mode) tab.classList.add("active");
    tab.addEventListener("click", () => setPanel(m));
    tabs.appendChild(tab);
  }
  panelInput.placeholder = placeholders[mode];
  if (mode !== "search") {
    panelInput.value = "";
    panelFilter = "";
  }

  for (const [id, m] of [
    ["btn-toc", "toc"],
    ["btn-search", "search"],
    ["btn-bookmarks", "bookmarks"],
    ["btn-notes", "notes"],
  ] as [string, PanelMode][]) {
    $(id).classList.toggle("active", m === mode);
  }

  if (mode === "toc") renderToc();
  if (mode === "bookmarks") void renderBookmarks();
  if (mode === "notes") void renderNotes();
  if (mode === "search") {
    body.replaceChildren(
      el(
        "p",
        "panel-empty",
        searchResults.length
          ? `${searchResults.length} result${searchResults.length === 1 ? "" : "s"}`
          : "Type to search the whole book.",
      ),
    );
    renderSearchResults();
    if (mode === "search") window.setTimeout(() => panelInput.focus(), 30);
  }
}

function closePanel() {
  if (panelMode) setPanel(null);
}

function togglePanel(mode: PanelMode) {
  setPanel(panelMode === mode ? null : mode);
}

function matchesPanelFilter(...fields: (string | null | undefined)[]): boolean {
  if (!panelFilter) return true;
  const needle = panelFilter.toLowerCase();
  return fields.some((field) => (field ?? "").toLowerCase().includes(needle));
}

function renderToc() {
  const body = $("panel-body");
  const nodes = flatToc(reader.getToc());
  const currentHref = reader.currentHref ?? "";
  // Prefer an exact document match so only one row lights up; a nested
  // "#fragment" entry for the same document must not also highlight.
  const activeHref =
    nodes.find((n) => n.href === currentHref)?.href ??
    nodes.find((n) => n.href.split("#")[0] === currentHref)?.href ??
    "";

  const visible = nodes.filter((n) => matchesPanelFilter(n.title));
  body.replaceChildren();
  if (!nodes.length) {
    body.appendChild(el("p", "panel-empty", "This book has no table of contents."));
    return;
  }
  if (!visible.length) {
    body.appendChild(el("p", "panel-empty", `No section matches “${panelFilter}”.`));
    return;
  }
  for (const node of visible) {
    const btn = el("button", "toc-item");
    btn.type = "button";
    if (node.href === activeHref) btn.classList.add("active");
    btn.style.paddingLeft = `${10 + Math.min(node.depth, 5) * 13}px`;
    btn.appendChild(document.createTextNode(node.title || "Untitled"));
    btn.addEventListener("click", async () => {
      await reader.goToHref(node.href);
      closePanel();
    });
    body.appendChild(btn);
  }
}

function renderSearchResults() {
  const body = $("panel-body");
  if (!searchResults.length) return;
  body.replaceChildren();
  for (const hit of searchResults) {
    const btn = el("button", "result");
    btn.type = "button";
    const head = el("div", "result-head");
    head.appendChild(el("span", undefined, hit.title || `Section ${hit.chapter_index + 1}`));
    head.appendChild(el("span", undefined, `${Math.round(hit.chapter_index + 1)}`));
    const excerpt = el("div", "result-excerpt");
    excerpt.innerHTML = excerptHtml(hit.excerpt);
    btn.append(head, excerpt);
    btn.addEventListener("click", async () => {
      await reader.goToHref(hit.href);
      closePanel();
    });
    body.appendChild(btn);
  }
}

async function runSearch(query: string) {
  if (searchTimer) window.clearTimeout(searchTimer);
  searchTimer = window.setTimeout(async () => {
    if (query.trim().length < 2) {
      searchResults = [];
      renderSearchResults();
      return;
    }
    const count = $("panel-body").childElementCount;
    if (count) $("panel-body").replaceChildren(el("p", "panel-empty", "Searching…"));
    try {
      searchResults = await reader.search(query.trim());
    } catch (error) {
      toast(String(error));
      searchResults = [];
    }
    if (panelMode !== "search") return;
    if (!searchResults.length) {
      $("panel-body").replaceChildren(
        el("p", "panel-empty", `No matches for “${query.trim()}”.`),
      );
      return;
    }
    renderSearchResults();
  }, 220);
}

/** window.prompt is disabled in WebView2, so renaming goes through our modal. */
async function renameBookmarkFlow(mark: { id: number; label: string }) {
  await promptModal({
    title: "Rename bookmark",
    label: "Label",
    value: mark.label,
    confirmLabel: "Rename",
    onConfirm: async (value) => {
      await api.renameBookmark(mark.id, value);
      mark.label = value;
      await renderBookmarks();
    },
  });
}

async function renderBookmarks() {
  await reader.refreshBookmarks();
  const body = $("panel-body");
  const all = reader.getBookmarks();
  const list = all.filter((b) => matchesPanelFilter(b.label, b.href));
  body.replaceChildren();
  if (!all.length) {
    body.appendChild(
      el("p", "panel-empty", "No bookmarks yet. Press B to bookmark this page."),
    );
    return;
  }
  if (!list.length) {
    body.appendChild(el("p", "panel-empty", `No bookmark matches “${panelFilter}”.`));
    return;
  }

  const current = reader.currentBookmark();
  const chapterTitle = (href: string) => {
    const detail = reader.getToc();
    const flat = flatToc(detail);
    const match = flat.find((n) => n.href.split("#")[0] === href);
    return match?.title ?? href.split("/").pop() ?? href;
  };

  for (const mark of list) {
    const row = el("div", "list-row");
    if (current && current.id === mark.id) row.classList.add("is-current");
    const main = el("div", "list-row-main");

    const title = document.createElement("div");
    title.className = "list-row-title";
    title.textContent = mark.label;
    title.title = "Double-click to rename";
    title.addEventListener("dblclick", () => void renameBookmarkFlow(mark));

    // Show where in the book this bookmark sits, not just the date.
    const saved = parseLocator(mark.locator);
    const position = saved?.page
      ? `page ${saved.page + 1}`
      : saved?.fraction
        ? `${Math.round(saved.fraction * 100)}% in`
        : "chapter start";

    main.append(
      title,
      el(
        "div",
        "list-row-sub",
        `${chapterTitle(mark.href)} · ${position} · ${formatDate(mark.created_at)}`,
      ),
    );

    const actions = el("div", "list-row-actions");
    const jump = el("button", "icon-btn");
    jump.title = "Go to bookmark";
    jump.innerHTML = '<svg viewBox="0 0 24 24"><path d="M5 12h14M13 6l6 6-6 6"/></svg>';
    jump.addEventListener("click", async () => {
      await reader.goToBookmark(mark);
      closePanel();
    });
    const rename = el("button", "icon-btn");
    rename.title = "Rename";
    rename.innerHTML = '<svg viewBox="0 0 24 24"><path d="M4 20h4L19 9a2.1 2.1 0 0 0-3-3L5 17v3z"/></svg>';
    rename.addEventListener("click", () => void renameBookmarkFlow(mark));
    const remove = el("button", "icon-btn");
    remove.title = "Delete";
    remove.innerHTML = '<svg viewBox="0 0 24 24"><path d="M5 7h14M10 7V5h4v2M7 7l1 13h8l1-13"/></svg>';
    remove.addEventListener("click", async () => {
      await api.deleteBookmark(mark.id);
      await renderBookmarks();
    });
    actions.append(jump, rename, remove);
    row.append(main, actions);
    row.addEventListener("click", (event) => {
      if ((event.target as HTMLElement).closest("button")) return;
      void reader.goToBookmark(mark).then(closePanel);
    });
    body.appendChild(row);
  }
}

async function renderNotes() {
  await reader.refreshAnnotations();
  const body = $("panel-body");
  const all = reader.getAnnotations();
  const list = all.filter((a) =>
    matchesPanelFilter(a.quote, a.note, a.color),
  );
  body.replaceChildren();
  if (!all.length) {
    body.appendChild(
      el("p", "panel-empty", "Select text while reading to highlight it, then add a note."),
    );
    return;
  }
  if (!list.length) {
    body.appendChild(el("p", "panel-empty", `No note matches “${panelFilter}”.`));
    return;
  }
  for (const note of list) {
    const row = el("div", "list-row");
    const main = el("div", "list-row-main");
    const quote = el("div", "quote");
    quote.textContent = `“${note.quote}”`;
    const input = document.createElement("textarea");
    input.className = "note-input";
    input.rows = note.note ? 3 : 1;
    input.placeholder = "Add a note…";
    input.value = note.note;
    input.addEventListener("change", async () => {
      await api.updateAnnotation({ id: note.id, note: input.value });
      note.note = input.value;
    });

    const swatches = el("div", "swatch-row");
    for (const color of ["yellow", "green", "blue", "pink"]) {
      const swatch = el("button", `swatch sw-${color}`);
      if (note.color === color) swatch.classList.add("active");
      swatch.title = color;
      swatch.addEventListener("click", async () => {
        await api.updateAnnotation({ id: note.id, color });
        note.color = color;
        swatches.querySelectorAll(".swatch").forEach((s) => s.classList.remove("active"));
        swatch.classList.add("active");
        await reader.refreshAnnotations();
      });
      swatches.appendChild(swatch);
    }

    const actions = el("div", "list-row-actions");
    const jump = el("button", "icon-btn");
    jump.title = "Go to highlight";
    jump.innerHTML = '<svg viewBox="0 0 24 24"><path d="M5 12h14M13 6l6 6-6 6"/></svg>';
    jump.addEventListener("click", async () => {
      await reader.goToHref(note.href);
      closePanel();
    });
    const remove = el("button", "icon-btn");
    remove.title = "Delete highlight";
    remove.innerHTML = '<svg viewBox="0 0 24 24"><path d="M5 7h14M10 7V5h4v2M7 7l1 13h8l1-13"/></svg>';
    remove.addEventListener("click", async () => {
      await api.deleteAnnotation(note.id);
      await renderNotes();
    });
    actions.append(jump, remove);

    main.append(quote, input, swatches);
    row.append(main, actions);
    body.appendChild(row);
  }
}

/* ------------------------------------------------------------------ input */

function isTyping(target: EventTarget | null): boolean {
  const node = target as HTMLElement | null;
  if (!node) return false;
  const tag = node.tagName;
  return (
    tag === "INPUT" ||
    tag === "TEXTAREA" ||
    tag === "SELECT" ||
    node.isContentEditable
  );
}

function bindKeys() {
  window.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      // Blur first: removing a focused <textarea> without a `change` event
      // discards what the user had typed. This must run before the isTyping
      // guard below.
      if (isTyping(event.target)) {
        (event.target as HTMLElement).blur();
        return;
      }
      if (panelMode) {
        setPanel(null);
        return;
      }
      if (document.querySelector(".sheet") || document.querySelector(".context-menu")) {
        closeOverlays();
        return;
      }
      if (!readerEl.hidden) {
        showLibrary();
        return;
      }
    }

    if (readerEl.hidden || isTyping(event.target)) return;
    if (document.querySelector(".sheet")) return;

    switch (event.key) {
      case "ArrowRight":
      case "PageDown":
      case " ":
      case "j":
        event.preventDefault();
        void reader.next();
        break;
      case "ArrowLeft":
      case "PageUp":
      case "k":
        event.preventDefault();
        void reader.prev();
        break;
      case "Home":
        event.preventDefault();
        void reader.goToHref(reader.getToc()[0]?.href ?? "");
        break;
      case "t":
        event.preventDefault();
        togglePanel("toc");
        break;
      case "s":
        event.preventDefault();
        togglePanel("search");
        break;
      case "b":
        event.preventDefault();
        void reader.toggleBookmark();
        break;
      case "n":
        event.preventDefault();
        togglePanel("notes");
        break;
      case "f":
        event.preventDefault();
        void toggleFullscreen();
        break;
      case "+":
      case "=":
        event.preventDefault();
        reader.nudgeFont(1);
        break;
      case "-":
      case "_":
        event.preventDefault();
        reader.nudgeFont(-1);
        break;
      default:
        break;
    }
  });
}

async function toggleFullscreen() {
  try {
    if (document.fullscreenElement) await document.exitFullscreen();
    else await readerEl.requestFullscreen();
  } catch {
    toast("Fullscreen is unavailable in this window.");
  }
}

function bindChrome() {
  $("zone-next").addEventListener("click", () => {
    void reader.next();
    reader.showChromeTemporarily();
  });
  $("zone-prev").addEventListener("click", () => {
    void reader.prev();
    reader.showChromeTemporarily();
  });
  $("btn-next").addEventListener("click", () => void reader.next());
  $("btn-prev").addEventListener("click", () => void reader.prev());
  $("btn-fs").addEventListener("click", () => void toggleFullscreen());
  readerEl.addEventListener(
    "mousemove",
    (event) => {
      const bottom = window.innerHeight - event.clientY < 90;
      (reader as unknown as { chrome: HTMLElement }).chrome.classList.toggle("show", bottom);
    },
    { passive: true },
  );
  $("btn-back").addEventListener("click", () => showLibrary());
  $("btn-toc").addEventListener("click", () => togglePanel("toc"));
  $("btn-bookmarks").addEventListener("click", () => togglePanel("bookmarks"));
  $("btn-notes").addEventListener("click", () => togglePanel("notes"));
  $("btn-search").addEventListener("click", () => togglePanel("search"));
  $("btn-settings").addEventListener("click", showSettings);
  $("panel-close").addEventListener("click", () => setPanel(null));
  scrimEl.addEventListener("click", () => setPanel(null));
  panelInput.addEventListener("input", () => {
    if (panelMode === "search") {
      runSearch(panelInput.value);
      return;
    }
    panelFilter = panelInput.value.trim();
    if (panelMode === "toc") renderToc();
    if (panelMode === "bookmarks") void renderBookmarks();
    if (panelMode === "notes") void renderNotes();
  });
}

function showSettings() {
  openSettingsSheet(settings, (next) => {
    // Mutate in place. The Reader holds a reference to this exact object, so
    // rebinding the variable here (`settings = next`) would leave the two
    // pointing at different objects and every later change would be silently
    // discarded by the reader.
    Object.assign(settings, next);
    reader.applySettings();
  });
}

/* ------------------------------------------------------------------ setup */

async function bindDragDrop() {
  try {
    const { getCurrentWebview } = await import("@tauri-apps/api/webview");
    const webview = getCurrentWebview();
    await webview.onDragDropEvent(async (event) => {
      if (event.payload.type !== "drop") return;
      const paths = event.payload.paths.filter((p) =>
        p.toLowerCase().endsWith(".epub"),
      );
      if (!paths.length) return;
      if (readerEl.hidden) await library.importPaths(paths);
      else toast("Open the library first to add more books.");
    });
  } catch {
    /* drag & drop is optional */
  }
}

/**
 * Import and open any EPUB the shell asked for. Covers double-clicking a .epub
 * (file association) and launching a second copy while we already run, in which
 * case the Rust side queues the path and we drain it here.
 *
 * `library.importPaths` returns the rows it inserted, and those carry the
 * canonicalised path — which on Windows is not the string the shell handed us.
 * Matching on the input paths instead would always miss.
 */
async function drainPendingOpens(): Promise<void> {
  let paths: string[] = [];
  try {
    paths = await api.takePendingOpens();
  } catch {
    return;
  }
  const epubs = paths.filter((p) => p.toLowerCase().endsWith(".epub"));
  if (!epubs.length) return;

  const added = await library.importPaths(epubs);
  await refreshLibrary();

  const first = added[0];
  if (!first) return;

  const book = books.find((b) => b.id === first.id) ?? first;
  if (readerEl.hidden) {
    await openBook(book);
  } else {
    // Already reading something: the book is now on the shelf, but do not
    // yank the reader away from the page they are on.
    toast(`Added “${book.title}” to your shelf.`);
  }
}

async function boot() {
  settings = await loadSettings();
  // effectiveTheme, not the raw stored value: with "Match Windows" enabled the
  // stored theme is only a fallback, and using it directly meant the setting
  // reverted to the last manual pick on every launch.
  applyTheme(effectiveTheme(settings));
  watchSystemTheme(settings, applyTheme);

  library = new Library(
    $("lib-grid"),
    $("lib-empty"),
    $<HTMLInputElement>("lib-filter"),
    $<HTMLSelectElement>("lib-sort"),
    {
      onOpen: (book) => void openBook(book),
      onImport: () => void library.importViaDialog(),
      onChanged: () => void refreshLibrary(),
      onNotify: toast,
    },
  );

  reader = new Reader(
    $("epub-viewport"),
    $("epub-content"),
    $<HTMLInputElement>("page-slider"),
    $("page-label"),
    $("chrome"),
    $("topbar-sub"),
    settings,
    {
      onClose: () => showLibrary(),
      onProgress: (progress, locator) => {
        const id = reader.currentBookId;
        if (!id) return;
        void api.saveProgress(id, progress, JSON.stringify(locator));
        const local = books.find((b) => b.id === id);
        if (local) {
          local.progress = progress;
          local.locator = JSON.stringify(locator);
        }
      },
      onBookChange: () => {
        const id = reader.currentBookId;
        const local = books.find((b) => b.id === id);
        if (local) library.patch(local);
      },
      onNotify: toast,
      onTogglePanel: (mode) => mode && setPanel(mode),
    },
  );

  $("btn-import").addEventListener("click", () => void library.importViaDialog());
  $("btn-import-empty").addEventListener("click", () => void library.importViaDialog());
  bindKeys();
  bindChrome();
  window.addEventListener("beforeunload", () => reader.flushSave());

  busy("Loading library…");
  try {
    await refreshLibrary();
  } catch (error) {
    toast(String(error));
  } finally {
    idle();
  }
  void bindDragDrop();
  void drainPendingOpens();
  // A relaunch of the exe (for example double-clicking a second book) lands in
  // the already-running instance; draining again picks it up.
  window.addEventListener("focus", () => void drainPendingOpens());
}

void boot();