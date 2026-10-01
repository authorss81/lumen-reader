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
  /** false = paginated columns, true = continuous vertical scrolling. */
  scrollMode: boolean;
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
private annotations: Annotation[] = [];
  private bookmarks: Bookmark[] = [];
  private chromeTimer: number | undefined;
  private resizeTimer: number | undefined;
  private wheelAccumulator = 0;
  private wheelLocked = false;
  private dragStart: { x: number; y: number } | null = null;
  /** Annotation ids that could not be located in the current chapter. */
  private unrendered: number[] = [];
  private saveTimer: number | undefined;

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

    this.bindWheel();
    this.bindDragPaging();
    this.bindTextSelection();
  }

  /* ------------------------------------------------- wheel and drag paging */

  /**
   * In paginated mode the wheel turns pages. A trackpad emits a long stream of
   * small deltas, so accumulate them and only flip once per threshold, with a
   * short lockout to stop one flick from running away through the book.
   */
  private bindWheel() {
    this.viewport.addEventListener(
      "wheel",
      (event) => {
        if (this.settings.scrollMode || !this.book) return;
        event.preventDefault();
        if (this.wheelLocked) return;
        const dominant = Math.abs(event.deltaX) > Math.abs(event.deltaY) ? event.deltaX : event.deltaY;
        this.wheelAccumulator += dominant;
        const threshold = event.deltaMode === 1 ? 3 : 110;
        if (Math.abs(this.wheelAccumulator) < threshold) return;
        const forward = this.wheelAccumulator > 0;
        this.wheelAccumulator = 0;
        this.wheelLocked = true;
        window.setTimeout(() => (this.wheelLocked = false), 180);
        if (forward) void this.next();
        else void this.prev();
        this.showChromeTemporarily();
      },
      { passive: false },
    );
  }

  /** Click-and-drag past a threshold pages forward or back. */
  private bindDragPaging() {
    this.viewport.addEventListener("pointerdown", (event) => {
      if (this.settings.scrollMode) return;
      if (event.button !== 0) return;
      this.dragStart = { x: event.clientX, y: event.clientY };
    });
    this.viewport.addEventListener("pointerup", (event) => {
      const start = this.dragStart;
      this.dragStart = null;
      if (!start || !this.book) return;

      // Selecting text is a drag. Selecting a sentence inside one line gives a
      // large horizontal delta and a tiny vertical one, which is exactly the
      // gesture this handler used to treat as "turn the page" — so every
      // ordinary selection yanked the page out from under the user and the rest
      // of the line appeared to vanish. Never page while a selection exists.
      const selection = window.getSelection();
      if (selection && !selection.isCollapsed && selection.toString().trim()) {
        return;
      }

      const dx = event.clientX - start.x;
      const dy = event.clientY - start.y;
      // Require a decisively horizontal drag, and more travel than before.
      if (Math.abs(dx) < 120 || Math.abs(dx) < Math.abs(dy) * 2) return;
      if (dx < 0) void this.next();
      else void this.prev();
      this.showChromeTemporarily();
    });
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
    const scrolling = this.settings.scrollMode;

    this.viewport.classList.toggle("is-scrolling", scrolling);
    this.slider.hidden = scrolling;
    this.slider.parentElement?.classList.toggle("is-scrolling", scrolling);
    style.top = `${this.settings.marginY}px`;
    style.left = `${this.settings.marginX}px`;

    if (scrolling) {
      // Continuous vertical reading: no columns, natural document flow, and the
      // viewport does the scrolling. `.epub-content` is `position: relative`
      // here, so the offsets are zeroed and the margins are expressed as
      // padding instead. With `box-sizing: border-box` the width is the full
      // viewport and the padding creates the gutters.
      style.top = "0px";
      style.left = "0px";
      style.height = "";
      style.width = `${this.viewport.clientWidth}px`;
      style.columnWidth = "";
      style.columnGap = "";
      style.transform = "";
      style.padding = `${this.settings.marginY}px ${this.settings.marginX}px`;
      style.paddingBottom = `${Math.round(this.viewport.clientHeight * 0.5)}px`;
      this.pages = 1;
      this.page = 0;
      this.slider.max = "0";
      this.paint();
      return;
    }

    style.padding = "0";
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
    if (this.settings.scrollMode) return;
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
    if (this.settings.scrollMode) {
      const scroller = this.viewport;
      const max = Math.max(1, scroller.scrollHeight - scroller.clientHeight);
      const pct = Math.round((scroller.scrollTop / max) * 100);
      this.pageLabel.textContent = `${pct}%`;
      this.scheduleSave();
      this.updateSubLabel();
      return;
    }
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
    const within = this.settings.scrollMode
      ? this.scrollFraction()
      : this.pages > 1
        ? this.page / (this.pages - 1)
        : 0;
    return Math.min(1, (this.chapter + within) / total);
  }

  /** How far down the current chapter we are, 0 - 1. */
  private scrollFraction(): number {
    const max = this.viewport.scrollHeight - this.viewport.clientHeight;
    if (max <= 0) return 0;
    return Math.max(0, Math.min(1, this.viewport.scrollTop / max));
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
    if (this.settings.scrollMode) {
      const max = this.viewport.scrollHeight - this.viewport.clientHeight;
      this.viewport.scrollTop = Math.max(0, Math.min(max, fraction * max));
      this.paint();
      return;
    }
    this.goToPage(Math.floor(fraction * (this.pages - 1)), false);
  }

  async next(): Promise<void> {
    if (this.settings.scrollMode) {
      const step = this.viewport.clientHeight * 0.9;
      const target = this.viewport.scrollTop + step;
      const max = this.viewport.scrollHeight - this.viewport.clientHeight;
      if (target < max - 4) {
        this.viewport.scrollTo({ top: target, behavior: "smooth" });
        return;
      }
    } else if (this.page + 1 < this.pages) {
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
    if (this.settings.scrollMode) {
      const step = this.viewport.clientHeight * 0.9;
      const target = this.viewport.scrollTop - step;
      if (target > 4) {
        this.viewport.scrollTo({ top: target, behavior: "smooth" });
        return;
      }
    } else if (this.page > 0) {
      this.goToPage(this.page - 1);
      return;
    }
    if (this.chapter > 0) {
      const target = this.chapter - 1;
      await this.loadChapter(target);
      if (this.settings.scrollMode) {
        this.viewport.scrollTop = this.viewport.scrollHeight;
        this.paint();
      } else {
        this.goToPage(this.pages - 1, false);
      }
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
    if (this.settings.scrollMode) this.viewport.scrollTop = 0;
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

async goToHref(href: string, locator?: Locator | null) {
    if (!this.book) return;
    const [path, fragment] = splitOnce(href, "#");
    const index = this.book.chapters.findIndex((c) => c.href === path);
    if (index < 0) return;
    if (index === this.chapter) {
      if (fragment) this.jumpToFragment(fragment);
      if (locator) this.restoreLocator(locator);
      return;
    }
    await this.loadChapter(index, fragment);
    // A bookmark stores the exact position inside the chapter, so a jump has to
    // land on the same page rather than the top of the chapter.
    if (locator) this.restoreLocator(locator);
  }

  /** Put the reader back on a saved chapter/page/fraction. */
  private restoreLocator(locator: Locator) {
    if (locator.chapter !== this.chapter) return;
    if (this.settings.scrollMode) {
      this.goToFraction(locator.fraction ?? 0);
      return;
    }
    if (locator.page) this.goToPage(locator.page, false);
    else if (locator.fraction) this.goToFraction(locator.fraction);
  }

  /** Jump to a saved bookmark. */
  async goToBookmark(mark: Bookmark) {
    const parsed = parseLocator(mark.locator);
    await this.goToHref(mark.href, parsed);
  }

  /* ---------------------------------------------------------- highlights */

  private applyHighlights(doc: HTMLElement) {
    if (!this.annotations.length) return;
    const forChapter = this.annotations.filter((a) => {
      const href = this.book?.chapters[this.chapter]?.href;
      return a.href === href && a.quote.trim().length > 1;
    });
for (const note of forChapter) {
      const applied = wrapFirstOccurrence(doc, note.quote, note.color, String(note.id));
      if (!applied) this.unrendered.push(note.id);
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
      const doc = this.content.querySelector<HTMLElement>(".epub-doc");
      const applied = doc ? wrapFirstOccurrence(doc, quote, color, String(created.id)) : false;
      if (!applied) {
        // The quote could not be located in the rendered text. Say so instead of
        // letting the user conclude that highlighting is broken.
        this.cb.onNotify(
          "Saved, but the text could not be located on this page.",
        );
      }
      if (withNote) {
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

  /**
   * Capture the first line of visible text on the current page so the bookmark
   * list shows something recognisable instead of a bare chapter name.
   */
  private visibleExcerpt(): string {
    const { width, gap } = this.metrics();
    const left = this.page * (width + gap);
    const top = this.settings.marginY;
    const right = left + width;
    const bottom = top + this.viewport.clientHeight - this.settings.marginY;
    const doc = this.content.querySelector<HTMLElement>(".epub-doc");
    if (!doc) return "";
    for (const node of doc.querySelectorAll<HTMLElement>("p, h1, h2, h3, h4, li, blockquote")) {
      const rect = node.getBoundingClientRect();
      const base = this.content.getBoundingClientRect();
      const x = rect.left - base.left;
      const y = rect.top - base.top;
      // Any vertical overlap with the visible band counts.
      if (y + rect.height < top || y > bottom) continue;
      if (x + rect.width < left || x > right) continue;
      const text = (node.textContent ?? "").replace(/\s+/g, " ").trim();
      if (text.length > 1) return text.slice(0, 120);
    }
    return "";
  }

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
      const label = this.visibleExcerpt() || this.chapterLabel();
      const created = await api.addBookmark(this.book.id, href, locator, label);
      this.bookmarks.unshift(created);
      this.cb.onNotify("Bookmark added");
    }
    this.cb.onBookChange();
  }

  async renameBookmark(mark: Bookmark, label: string) {
    const trimmed = label.trim();
    if (!trimmed || trimmed === mark.label) return;
    mark.label = trimmed;
    // The label is stored in the annotation-adjacent table; reuse the notes
    // command surface by rewriting through a dedicated call below.
    await api.renameBookmark(mark.id, trimmed);
    this.cb.onNotify("Bookmark renamed");
  }

  /** The bookmark matching the current position, if any. */
  currentBookmark(): Bookmark | null {
    const href = this.book?.chapters[this.chapter]?.href;
    if (!href) return null;
    const locator = JSON.stringify(this.locator());
    return this.bookmarks.find((b) => b.href === href && b.locator === locator) ?? null;
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
      page: this.settings.scrollMode ? 0 : this.page,
      fraction: this.settings.scrollMode
        ? this.scrollFraction()
        : this.pages > 1
          ? this.page / (this.pages - 1)
          : 0,
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
    // Anchor on whichever position metric the current mode actually uses.
    // Scroll mode forces `pages = 1`, so the page-index anchor was always 0
    // and pressing + sent you back to the chapter heading.
    const anchor = this.settings.scrollMode
      ? this.scrollFraction()
      : this.pages > 1
        ? this.page / (this.pages - 1)
        : 0;
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

/**
 * Wrap the first occurrence of `needle` inside `root` with a `<mark>`.
 *
 * Three things this has to get right, all of which were wrong before:
 *  - a selection usually crosses element boundaries, so the match routinely
 *    spans several text nodes. Splitting each node it crosses is the only way
 *    to highlight a phrase containing `<em>` or `<a>`.
 *  - the selection string has its whitespace collapsed but the DOM does not,
 *    so both sides have to be normalised or the offsets do not line up.
 *  - splitting must never lose text: every node is sliced into
 *    before/mark/after and all three pieces are re-inserted.
 */
function wrapFirstOccurrence(
  root: HTMLElement,
  needle: string,
  color: string,
  id: string,
): boolean {
  const target = normaliseForMatch(needle);
  if (target.length < 2) return false;

  // Flatten to a string plus an index of where each node starts, joining with
  // a single space so that adjacent nodes do not fuse into false matches.
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  const nodes: Text[] = [];
  const starts: number[] = [];
  let probe = walker.nextNode() as Text | null;
  let flat = "";
  while (probe) {
    if (probe.data.length) {
      starts.push(flat.length);
      nodes.push(probe);
      flat += normaliseForMatch(probe.data);
      // Sentinel: a boundary can never fall inside a run of spaces, so a match
      // found here is guaranteed to be a real textual match, not a fusion.
      flat += " ";
    }
    probe = walker.nextNode() as Text | null;
  }
  if (!nodes.length) return false;

  const haystack = flat.toLowerCase();
  const pos = haystack.indexOf(target.toLowerCase());
  if (pos < 0) return false;
  const end = pos + target.length;

  // Map [pos, end) back onto the nodes it covers. A node can contribute a
  // prefix, the mark, and a suffix, in that order.
  const owner = (offset: number): number => {
    for (let i = nodes.length - 1; i >= 0; i -= 1) {
      if (starts[i] <= offset) return i;
    }
    return 0;
  };
  const firstNode = owner(pos);
  const lastNode = owner(end - 1);
  if (starts[lastNode] + nodes[lastNode].data.length <= end - 1) return false;

  // Walk backwards so mutating the DOM does not disturb the pending indices.
  for (let i = lastNode; i >= firstNode; i -= 1) {
    const text = nodes[i];
    const nodeStart = starts[i];
    const from = Math.max(0, pos - nodeStart);
    const to = Math.min(text.data.length, end - nodeStart);
    if (to <= from && !(i === firstNode && from === 0)) continue;

    const before = text.data.slice(0, from);
    const marked = text.data.slice(from, to);
    const after = text.data.slice(to);
    const parent = text.parentNode;
    if (!parent) return false;

    const pieces: Node[] = [];
    if (before) pieces.push(document.createTextNode(before));
    if (marked) {
      const mark = document.createElement("mark");
      mark.dataset.color = color;
      mark.dataset.id = id;
      mark.textContent = marked;
      pieces.push(mark);
    }
    if (after) pieces.push(document.createTextNode(after));

    parent.replaceChild(pieces[0], text);
    let anchor: Node = pieces[0];
    for (let k = 1; k < pieces.length; k += 1) {
      parent.insertBefore(pieces[k], anchor.nextSibling);
      anchor = pieces[k];
    }
  }
  return true;
}

/** Collapse every run of whitespace to one space, the way the DOM shows text. */
function normaliseForMatch(value: string): string {
  return value.replace(/\s+/g, " ");
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