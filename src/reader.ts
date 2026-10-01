import {
  api,
  el,
  escapeHtml,
  icon,
  type Annotation,
  type BookDetail,
  type Bookmark,
  type ChapterContent,
  type Locator,
  type SearchHit,
  type TocNode,
} from "./api";
import { openContextMenu } from "./library";

const GAP = 72; // horizontal gap between page columns
const MIN_SIZE = 12;
const MAX_SIZE = 34;

export interface ReaderSettings {
  fontSize: number;
  lineHeight: number;
  marginX: number;
  marginY: number;
  fontFamily: "serif" | "sans" | "mono";
  justify: boolean;
  letterSpacing: number;
}

export interface ReaderCallbacks {
  onClose: () => void;
  onProgress: (progress: number, locator: Locator) => void;
  onBookChange: () => void;
  onNotify: (message: string) => void;
  onTogglePanel: (mode: PanelMode | null) => void;
}

export type PanelMode = "toc" | "search" | "bookmarks" | "notes";

export class Reader {
  private book: BookDetail | null = null;
  private chapter = 0;
  private page = 0;
  private pages = 1;
  private loading = false;
  private saveTimer: number | undefined;
  private annotations: Annotation[] = [];
  private bookmarks: Bookmark[] = [];
  private chromeTimer: number | undefined;
  private resizeTimer: number | undefined;

  constructor(
    private viewport: HTMLElement,
    private content: HTMLElement,
    private slider: HTMLInputElement,
    private pageLabel: HTMLElement,
    private chrome: HTMLElement,
    private subLabel: HTMLElement,
    private settings: ReaderSettings,
    private cb: ReaderCallbacks,
  ) {
    this.applySettings();

    this.slider.addEventListener("input", () => {
      this.goToPage(Number(this.slider.value));
    });

    const observer = new ResizeObserver(() => this.scheduleRelayout());
    observer.observe(this.viewport);
    window.addEventListener("resize", () => this.scheduleRelayout());

    this.bindTextSelection();
  }

  /* ------------------------------------------------------------ settings */

  applySettings() {
    const s = this.settings;
    const root = document.documentElement.style;
    root.setProperty("--reader-size", `${s.fontSize}px`);
    root.setProperty("--reader-leading", String(s.lineHeight));
    root.setProperty("--reader-tracking", `${s.letterSpacing}em`);
    root.setProperty("--reader-align", s.justify ? "justify" : "left");
    root.setProperty(
      "--reader-font",
      s.fontFamily === "serif"
        ? "var(--font-serif)"
        : s.fontFamily === "sans"
          ? "var(--font-sans)"
          : "var(--font-mono)",
    );
    this.relayout();
  }

  /* --------------------------------------------------------------- open */

  async open(book: BookDetail) {
    this.book = book;
    this.annotations = await api.listAnnotations(book.id).catch(() => []);
    this.bookmarks = await api.listBookmarks(book.id).catch(() => []);
    this.chapter = 0;
    this.page = 0;

    const saved = parseLocator(book.locator);
    if (saved && saved.chapter < book.chapters.length) this.chapter = saved.chapter;
    await this.loadChapter(this.chapter, saved?.fragment);
    if (saved && saved.page) this.goToPage(saved.page);
    else if (saved && saved.fraction) this.goToFraction(saved.fraction);
    this.showChromeTemporarily();
  }

  get currentBookId(): string | null {
    return this.book?.id ?? null;
  }

  /* ------------------------------------------------------------ geometry */

  private metrics() {
    const s = this.settings;
    const width = Math.max(240, this.viewport.clientWidth - s.marginX * 2);
    const height = Math.max(200, this.viewport.clientHeight - s.marginY * 2);
    return { width, height, gap: GAP };
  }

private relayout() {
    if (!this.book) return;
    const { width, height } = this.metrics();
    const style = this.content.style;
    style.top = `${this.settings.marginY}px`;
    style.left = `${this.settings.marginX}px`;
    style.height = `${height}px`;
    // `column-width` is only a *suggestion*: with no explicit width the browser
    // widens the single column to fill the container, which desynchronises the
    // column pitch from our page step. Pinning the width makes the column
    // exactly `width` and the pitch exactly `width + gap`.
    style.width = `${width}px`;
    style.columnWidth = `${width}px`;
    style.columnGap = `${GAP}px`;
    this.measure();
  }

  private scheduleRelayout() {
    if (this.resizeTimer) window.clearTimeout(this.resizeTimer);
    this.resizeTimer = window.setTimeout(() => this.relayout(), 120);
  }

  private measure() {
    const { width, gap } = this.metrics();
    const step = width + gap;
    // scrollWidth spans every laid-out column; divide to get the page count.
    const total = this.content.scrollWidth;
    this.pages = Math.max(1, Math.round((total + gap) / step));
    this.slider.max = String(Math.max(0, this.pages - 1));
    this.page = Math.min(this.page, this.pages - 1);
    this.paint();
  }

  private paint() {
    const { width, gap } = this.metrics();
    this.content.style.transform = `translate3d(${-this.page * (width + gap)}px,0,0)`;
    this.slider.value = String(this.page);
    const pct = Math.round(((this.page + 1) / this.pages) * 100);
    this.pageLabel.textContent = `${pct}% · ${this.page + 1}/${this.pages}`;
    this.scheduleSave();
    this.updateSubLabel();
  }

  private updateSubLabel() {
    if (!this.book) return;
    const toc = flatToc(this.book.toc);
    const chapter = this.book.chapters[this.chapter];
    let active = chapter?.title || "";
    if (chapter) {
      let best: TocNode | null = null;
      for (const node of toc) {
        if (!node.href.split("#")[0] || node.href.split("#")[0] !== chapter.href) continue;
        best = node;
      }
      if (best) active = best.title;
    }
    const percent = Math.round(this.overallProgress() * 100);
    this.subLabel.textContent = `${active || `Section ${this.chapter + 1}`} · ${percent}%`;
  }

  overallProgress(): number {
    if (!this.book) return 0;
    const total = Math.max(1, this.book.chapters.length);
    const within = this.pages > 1 ? this.page / this.pages : 0;
    return Math.min(1, (this.chapter + within) / total);
  }

  /* ----------------------------------------------------------- navigation */

  private goToPage(page: number, animate = true) {
    const next = Math.max(0, Math.min(page, this.pages - 1));
    if (next === this.page) return;
    this.page = next;
    if (!animate) this.content.classList.add("no-anim");
    this.paint();
    if (!animate) {
      void this.content.offsetWidth;
      this.content.classList.remove("no-anim");
    }
  }

  private goToFraction(fraction: number) {
    this.goToPage(Math.floor(fraction * (this.pages - 1)), false);
  }

  async next(): Promise<void> {
    if (this.page + 1 < this.pages) {
      this.goToPage(this.page + 1);
      return;
    }
    if (this.book && this.chapter + 1 < this.book.chapters.length) {
      await this.loadChapter(this.chapter + 1);
      this.showChromeTemporarily();
    } else {
      this.cb.onNotify("End of book");
    }
  }

  async prev(): Promise<void> {
    if (this.page > 0) {
      this.goToPage(this.page - 1);
      return;
    }
    if (this.chapter > 0) {
      const target = this.chapter - 1;
      await this.loadChapter(target);
      this.goToPage(this.pages - 1, false);
    }
  }

  /* -------------------------------------------------------------- loading */

  private async loadChapter(index: number, fragment?: string) {
    if (!this.book || this.loading) return;
    this.loading = true;
    const bookId = this.book.id;
    this.chapter = Math.max(0, Math.min(index, this.book.chapters.length - 1));
    this.page = 0;

    let payload: ChapterContent;
    try {
      payload = await api.loadChapter(bookId, this.chapter);
    } catch (error) {
      this.loading = false;
      this.cb.onNotify(String(error));
      return;
    }
    if (this.book?.id !== bookId) {
      this.loading = false;
      return;
    }

    this.content.classList.add("no-anim");
    this.content.replaceChildren();
    this.content.innerHTML = payload.html;

    const doc = this.content.querySelector<HTMLElement>(".epub-doc");
    if (doc) {
      const divider = el("div", "chapter-divider", payload.title || `Section ${this.chapter + 1}`);
      doc.prepend(divider);
      this.applyHighlights(doc);
      this.wireAnchors(doc);
      await this.afterLayout();
      if (fragment) this.jumpToFragment(fragment);
    } else {
      await this.afterLayout();
    }

    this.goToPage(0, false);
    this.loading = false;
  }

  /** Wait for fonts and images so page geometry is final. */
  private async afterLayout(): Promise<void> {
    this.relayout();
    try {
      await document.fonts.ready;
    } catch {
      /* ignore */
    }
    const images = Array.from(this.content.querySelectorAll("img"));
    await Promise.all(
      images.map((img) =>
        img.complete
          ? Promise.resolve()
          : new Promise<void>((resolve) => {
              img.addEventListener("load", () => resolve(), { once: true });
              img.addEventListener("error", () => resolve(), { once: true });
              window.setTimeout(resolve, 4000);
            }),
      ),
    );
    this.relayout();
    void this.content.offsetWidth;
    this.content.classList.remove("no-anim");
  }

  /* ------------------------------------------------------------- anchors */

  private wireAnchors(doc: HTMLElement) {
    doc.addEventListener("click", (event) => {
      const target = (event.target as HTMLElement)?.closest?.("a");
      if (!target) return;
      event.preventDefault();
      const href = target.getAttribute("data-href");
      const external = target.getAttribute("data-external");
      if (external) {
        openContextMenu(event.clientX, event.clientY, [
          {
            label: external.length > 40 ? `${external.slice(0, 40)}…` : external,
            iconPath: "M10 13a5 5 0 007.5.5l2-2a5 5 0 00-6.6-6.6l-1.2 1.2",
            action: () => window.open(external, "_blank", "noopener,noreferrer"),
          },
        ]);
        return;
      }
      if (!href || !this.book) return;
      const [path, fragment] = splitOnce(href, "#");
      const target2 = this.book.chapters.findIndex((c) => c.href === path);
      if (target2 >= 0 && target2 !== this.chapter) {
        void this.loadChapter(target2, fragment);
        return;
      }
      if (fragment) this.jumpToFragment(fragment);
    });
  }

  jumpToFragment(fragment: string) {
    const doc = this.content.querySelector<HTMLElement>(".epub-doc");
    if (!doc) return;
    let node: HTMLElement | null = doc.querySelector<HTMLElement>(`[id="${cssEscape(fragment)}"]`);
    if (!node) {
      node = doc.querySelector<HTMLElement>(`[name="${cssEscape(fragment)}"]`);
    }
if (!node) return;
    const { width } = this.metrics();
    const step = width + GAP;
    // offsetLeft is measured across the whole column flow, so it maps
    // directly onto a page index.
    const page = Math.max(0, Math.round((node.offsetLeft - 1) / step));
    this.goToPage(page, false);
  }

  async goToHref(href: string) {
    if (!this.book) return;
    const [path, fragment] = splitOnce(href, "#");
    const index = this.book.chapters.findIndex((c) => c.href === path);
    if (index < 0) return;
    if (index === this.chapter) {
      if (fragment) this.jumpToFragment(fragment);
      return;
    }
    await this.loadChapter(index, fragment);
  }

  /* ---------------------------------------------------------- highlights */

  private applyHighlights(doc: HTMLElement) {
    if (!this.annotations.length) return;
    const forChapter = this.annotations.filter((a) => {
      const href = this.book?.chapters[this.chapter]?.href;
      return a.href === href && a.quote.trim().length > 1;
    });
    for (const note of forChapter) {
      wrapFirstOccurrence(doc, note.quote, note.color, String(note.id));
    }
  }

  private bindTextSelection() {
    document.addEventListener("mouseup", () => {
      window.setTimeout(() => this.showSelectionActions(), 10);
    });
    document.addEventListener("keydown", (event) => {
      if (event.key === "Escape") {
        const bubble = document.querySelector(".selection-pop");
        if (bubble) {
          bubble.remove();
          event.stopPropagation();
        }
      }
    }, true);
  }

  private showSelectionActions() {
    document.querySelector(".selection-pop")?.remove();
    if (!this.book || this.viewport.offsetParent === null) return;
    const selection = window.getSelection();
    if (!selection || selection.isCollapsed || selection.rangeCount === 0) return;
    const quote = selection.toString().trim();
    if (quote.length < 2) return;
    const range = selection.getRangeAt(0);
    if (!this.content.contains(range.commonAncestorContainer)) return;

    const rect = range.getBoundingClientRect();
    const pop = el("div", "selection-pop");
    const swatches: HTMLElement[] = [];
    for (const color of ["yellow", "green", "blue", "pink"]) {
      const btn = el("button", `swatch sw-${color}`);
      btn.title = color;
      btn.addEventListener("click", () => {
        void this.highlight(color, quote);
        pop.remove();
      });
      swatches.push(btn);
      pop.appendChild(btn);
    }
    const note = el("button", "swatch sw-yellow");
    note.title = "Highlight + note";
    note.style.background = "var(--accent)";
    note.addEventListener("click", () => {
      void this.highlight("yellow", quote, true);
      pop.remove();
    });
    pop.appendChild(note);

    document.body.appendChild(pop);
    const width = pop.offsetWidth;
    pop.style.left = `${Math.min(
      window.innerWidth - width - 10,
      Math.max(10, rect.left + rect.width / 2 - width / 2),
    )}px`;
    const top = rect.top - pop.offsetHeight - 10;
    pop.style.top = `${top < 10 ? rect.bottom + 10 : top}px`;
  }

  private async highlight(color: string, quote: string, withNote = false) {
    if (!this.book) return;
    const href = this.book.chapters[this.chapter]?.href;
    if (!href) return;
    try {
      const created = await api.addAnnotation({
        book_id: this.book.id,
        href,
        selector: "",
        quote: quote.slice(0, 600),
        note: "",
        color,
      });
      this.annotations.push(created);
      this.repaintMarks();
      if (withNote) {
const doc = this.content.querySelector<HTMLElement>(".epub-doc");
        if (doc) wrapFirstOccurrence(doc, quote, color, String(created.id));
        this.cb.onNotify("Highlighted — add a note in the notes panel (N)");
        this.cb.onTogglePanel("notes");
      }
    } catch (error) {
      this.cb.onNotify(String(error));
    }
  }

  private repaintMarks() {
    const doc = this.content.querySelector<HTMLElement>(".epub-doc");
    if (!doc) return;
    doc.querySelectorAll("mark[data-id]").forEach((mark) => {
      const parent = mark.parentNode;
      if (!parent) return;
      while (mark.firstChild) parent.insertBefore(mark.firstChild, mark);
      mark.remove();
    });
    doc.normalize();
    this.applyHighlights(doc);
  }

  async refreshAnnotations() {
    if (!this.book) return;
    this.annotations = await api.listAnnotations(this.book.id).catch(() => []);
    this.repaintMarks();
  }

  async refreshBookmarks() {
    if (!this.book) return;
    this.bookmarks = await api.listBookmarks(this.book.id).catch(() => []);
  }

  getAnnotations(): Annotation[] {
    return this.annotations;
  }

  getBookmarks(): Bookmark[] {
    return this.bookmarks;
  }

  getToc(): TocNode[] {
    return this.book?.toc ?? [];
  }

  getPageList(): TocNode[] {
    return this.book?.page_list ?? [];
  }

  /* ------------------------------------------------------------ bookmarks */

  async toggleBookmark() {
    if (!this.book) return;
    const href = this.book.chapters[this.chapter]?.href;
    if (!href) return;
    const locator = JSON.stringify(this.locator());
    const existing = this.bookmarks.find(
      (b) => b.href === href && b.locator === locator,
    );
    if (existing) {
      await api.deleteBookmark(existing.id);
      this.bookmarks = this.bookmarks.filter((b) => b.id !== existing.id);
      this.cb.onNotify("Bookmark removed");
    } else {
      const created = await api.addBookmark(
        this.book.id,
        href,
        locator,
        this.chapterLabel(),
      );
      this.bookmarks.unshift(created);
      this.cb.onNotify("Bookmark added");
    }
    this.cb.onBookChange();
  }

  private chapterLabel(): string {
    if (!this.book) return "";
    const chapter = this.book.chapters[this.chapter];
    return chapter?.title || `Section ${this.chapter + 1}`;
  }

  /* ---------------------------------------------------------------- save */

  locator(): Locator {
    return {
      chapter: this.chapter,
      page: this.page,
      fraction: this.pages > 1 ? this.page / (this.pages - 1) : 0,
    };
  }

  private scheduleSave() {
    if (!this.book) return;
    if (this.saveTimer) window.clearTimeout(this.saveTimer);
    this.saveTimer = window.setTimeout(() => this.flushSave(), 900);
  }

  flushSave() {
    if (this.saveTimer) {
      window.clearTimeout(this.saveTimer);
      this.saveTimer = undefined;
    }
    if (!this.book) return;
    const locator = this.locator();
    const progress = this.overallProgress();
    this.cb.onProgress(progress, locator);
  }

  /* -------------------------------------------------------------- chrome */

  showChromeTemporarily() {
    this.chrome.classList.add("show");
    if (this.chromeTimer) window.clearTimeout(this.chromeTimer);
    this.chromeTimer = window.setTimeout(() => {
      this.chrome.classList.remove("show");
    }, 2600);
  }

  toggleChrome() {
    this.chrome.classList.toggle("show");
    if (this.chromeTimer) window.clearTimeout(this.chromeTimer);
  }

  nudgeFont(delta: number) {
    this.settings.fontSize = Math.max(
      MIN_SIZE,
      Math.min(MAX_SIZE, this.settings.fontSize + delta),
    );
    const anchor = this.pages > 1 ? this.page / (this.pages - 1) : 0;
    this.applySettings();
    void this.afterLayout().then(() => this.goToFraction(anchor));
    this.cb.onNotify(`${this.settings.fontSize}px`);
  }

  get chapterIndex() {
    return this.chapter;
  }

  get currentHref(): string | null {
    return this.book?.chapters[this.chapter]?.href ?? null;
  }

  get pageCount() {
    return this.pages;
  }

  async search(query: string): Promise<SearchHit[]> {
    if (!this.book) return [];
    return api.searchInBook(this.book.id, query);
  }
}

/* -------------------------------------------------------------- helpers */

function flatToc(nodes: TocNode[]): TocNode[] {
  const out: TocNode[] = [];
  const walk = (list: TocNode[]) => {
    for (const node of list) {
      out.push(node);
      if (node.children?.length) walk(node.children);
    }
  };
  walk(nodes ?? []);
  return out;
}

export { flatToc };

function splitOnce(value: string, sep: string): [string, string | undefined] {
  const i = value.indexOf(sep);
  if (i < 0) return [value, undefined];
  return [value.slice(0, i), value.slice(i + 1)];
}

function cssEscape(value: string): string {
  return value.replace(/["\\]/g, "\\$&");
}

/** Wrap the first occurrence of `needle` inside `root` with a <mark>. */
function wrapFirstOccurrence(
  root: HTMLElement,
  needle: string,
  color: string,
  id: string,
): boolean {
  const target = needle.replace(/\s+/g, " ").trim();
  if (target.length < 2) return false;

  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  const nodes: Text[] = [];
  let node = walker.nextNode() as Text | null;
  while (node) {
    nodes.push(node);
    node = walker.nextNode() as Text | null;
  }
  const joined = nodes.map((n) => n.data).join("");
  const pos = joined.toLowerCase().indexOf(target.toLowerCase());
  if (pos < 0) return false;
  const end = pos + target.length;

  let cursor = 0;
  for (const text of nodes) {
    const start = cursor;
    const stop = cursor + text.data.length;
    // Only wrap matches contained in a single text node; a match straddling
    // nodes is skipped rather than risking a mangled DOM.
    if (pos >= start && end <= stop) {
      const local = pos - start;
      const mark = document.createElement("mark");
      mark.dataset.color = color;
      mark.dataset.id = id;
      mark.textContent = text.data.slice(local, local + target.length);
      const tail = document.createTextNode(text.data.slice(local + target.length));
      const parent = text.parentNode;
      if (!parent) return false;
      parent.replaceChild(tail, text);
      parent.insertBefore(mark, tail);
      return true;
    }
    cursor = stop;
    if (cursor > pos) break;
  }
  return false;
}
export function parseLocator(raw: string): Locator | null {
  if (!raw) return null;
  try {
    const parsed = JSON.parse(raw) as Locator;
    if (typeof parsed?.chapter === "number") return parsed;
  } catch {
    /* ignore */
  }
  return null;
}

export function excerptHtml(excerpt: string): string {
  return escapeHtml(excerpt)
    .replace(/\[\[/g, "<mark>")
    .replace(/\]\]/g, "</mark>");
}

export { icon };