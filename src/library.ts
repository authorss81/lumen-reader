import {
  api,
  el,
  escapeHtml,
  formatBytes,
  formatDate,
  icon,
  pickEpubs,
  relativeTime,
  type BookMeta,
} from "./api";

const ICON_MORE = "M12 6.5h.01M12 12h.01M12 17.5h.01";
const ICON_PEN = "M4 20h4L19 9a2.1 2.1 0 0 0-3-3L5 17v3z";

export interface LibraryCallbacks {
  onOpen: (book: BookMeta) => void;
  onImport: () => void;
  onChanged: () => void;
  onNotify: (message: string) => void;
}

const coverCache = new Map<string, string | null>();

export class Library {
  private books: BookMeta[] = [];
  private filter = "";
  private sort: keyof typeof SORTS = "recent";
  private coverObserver: IntersectionObserver;

constructor(
    private grid: HTMLElement,
    private empty: HTMLElement,
    filterInput: HTMLInputElement,
    sortSelect: HTMLSelectElement,
    private cb: LibraryCallbacks,
  ) {
    this.coverObserver = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          if (!entry.isIntersecting) continue;
          this.coverObserver.unobserve(entry.target);
          void this.attachCover(entry.target as HTMLElement);
        }
      },
      { rootMargin: "320px" },
    );

    filterInput.addEventListener("input", () => {
      this.filter = filterInput.value.trim().toLowerCase();
      this.render();
    });
    sortSelect.addEventListener("change", () => {
      this.sort = sortSelect.value as keyof typeof SORTS;
      this.render();
    });
  }

  setBooks(books: BookMeta[]) {
    this.books = books;
    this.render();
  }

  /** Refresh one card in place (after progress or metadata changes). */
  patch(book: BookMeta) {
    this.books = this.books.map((b) => (b.id === book.id ? { ...b, ...book } : b));
    this.render();
  }

forget(bookId: string) {
    coverCache.delete(bookId);
    this.books = this.books.filter((b) => b.id !== bookId);
    this.render();
    void this.cb.onChanged();
  }

  private get visible(): BookMeta[] {
    const needle = this.filter;
    const list = needle
      ? this.books.filter(
          (b) =>
            b.title.toLowerCase().includes(needle) ||
            b.author.toLowerCase().includes(needle) ||
            (b.publisher || "").toLowerCase().includes(needle),
        )
      : this.books.slice();
    return SORTS[this.sort](list);
  }

  render() {
    const list = this.visible;
    this.grid.replaceChildren();
    this.empty.hidden = this.books.length > 0;

const noMatch = this.books.length > 0 && list.length === 0;
    if (noMatch) {
      this.grid.appendChild(
        el("p", "grid-empty", `No books match “${this.filter}”.`),
      );
      return;
    }

    const frag = document.createDocumentFragment();
    list.forEach((book, index) => {
      const card = this.card(book);
      // Caps the entrance stagger at 10 so late cards do not wait 900ms.
      card.style.setProperty("--i", String(Math.min(index, 10)));
      frag.appendChild(card);
    });
    this.grid.appendChild(frag);
  }

  private card(book: BookMeta): HTMLElement {
    const card = el("article", "card");
    card.tabIndex = 0;
    card.dataset.id = book.id;
    card.title = book.description || book.title;

    const cover = el("div", "card-cover");
    cover.dataset.id = book.id;
const cached = coverCache.get(book.id);
    if (cached !== undefined) {
      if (cached) this.paintCover(cover, cached);
      else this.paintFallback(cover, book);
    } else {
      cover.appendChild(el("div", "fallback", book.title));
      this.coverObserver.observe(cover);
    }

    if (book.progress > 0.001) {
      const bar = el("div", "card-progress");
      const fill = el("span");
      fill.style.width = `${Math.min(100, book.progress * 100).toFixed(1)}%`;
      bar.appendChild(fill);
      cover.appendChild(bar);
    }

    const menu = el("div", "card-menu");
    const menuBtn = el("button", "icon-btn");
    menuBtn.type = "button";
    menuBtn.title = "More actions";
    menuBtn.appendChild(icon(ICON_MORE));
    menuBtn.addEventListener("click", (event) => {
      event.stopPropagation();
      this.showMenu(event, book);
    });
    menu.appendChild(menuBtn);
    cover.appendChild(menu);

    const body = el("div", "card-body");
    body.appendChild(el("div", "card-title", book.title));
    body.appendChild(
      el("div", "card-author", book.author || "Unknown author"),
    );
    card.append(cover, body);

    card.addEventListener("click", () => this.cb.onOpen(book));
    card.addEventListener("keydown", (event) => {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        this.cb.onOpen(book);
      }
      if (event.key === "ContextMenu") {
        event.preventDefault();
        const rect = card.getBoundingClientRect();
        this.showMenu(
          { clientX: rect.left + rect.width - 30, clientY: rect.bottom } as MouseEvent,
          book,
        );
      }
    });
    card.addEventListener("contextmenu", (event) => {
      event.preventDefault();
      this.showMenu(event, book);
    });
    return card;
  }

private paintCover(host: HTMLElement, dataUrl: string) {
    host.replaceChildren();
    const img = document.createElement("img");
    img.loading = "lazy";
    img.decoding = "async";
    img.alt = "";
    img.src = dataUrl;
    // Fade in once decoded, so the shelf does not strobe as 20 covers resolve.
    img.addEventListener(
      "load",
      () => {
        img.dataset.loaded = "1";
      },
      { once: true },
    );
    if (img.complete) img.dataset.loaded = "1";
    const bar = host.querySelector(".card-progress");
    host.appendChild(img);
    if (bar) host.appendChild(bar);
  }

  private paintFallback(host: HTMLElement, book: BookMeta) {
    host.replaceChildren();
    host.appendChild(el("div", "fallback", book.title));
    const bar = host.querySelector(".card-progress");
    if (bar) host.appendChild(bar);
  }

  private async attachCover(host: HTMLElement) {
    const id = host.dataset.id;
    if (!id || coverCache.has(id)) return;
    try {
      const dataUrl = await api.getCover(id);
      coverCache.set(id, dataUrl);
      const card = host.closest(".card") as HTMLElement | null;
      if (!card || card.dataset.id !== id) return;
      if (dataUrl) this.paintCover(host, dataUrl);
      else this.paintFallback(host, this.books.find((b) => b.id === id)!);
    } catch {
      coverCache.set(id, null);
    }
  }

  private showMenu(event: MouseEvent, book: BookMeta) {
    openContextMenu(event.clientX, event.clientY, [
      {
        label: "Read",
        iconPath: "M4 5a2 2 0 012-2h13v18H6a2 2 0 01-2-2z",
        action: () => this.cb.onOpen(book),
      },
      {
        label: "Edit title & author",
        iconPath: ICON_PEN,
        action: () => this.editDetails(book),
      },
      {
        label: "Show in folder",
        iconPath: "M3 7a2 2 0 012-2h4l2 2h8a2 2 0 012 2v8a2 2 0 01-2 2H5a2 2 0 01-2-2z",
        action: () => void api.revealInExplorer(book.path),
      },
      { separator: true },
      {
        label: "Remove from library",
        iconPath: "M5 7h14M10 7V5h4v2M7 7l1 13h8l1-13",
        danger: true,
        action: () => this.confirmRemove(book),
      },
    ]);
  }

private async editDetails(book: BookMeta) {
    const title = await promptModal({
      title: "Edit book details",
      label: "Title",
      value: book.title,
      onConfirm: (value) => {
        const next = value.trim() || book.title;
        book.title = next;
        this.render();
      },
    });
    if (title === null) return;
    const author = await promptModal({
      title: "Edit book details",
      label: "Author",
      value: book.author,
      onConfirm: (value) => {
        book.author = value.trim();
        this.render();
      },
    });
    if (author === null) return;
    void api
      .updateBook(book.id, book.title, book.author)
      .then(() => this.cb.onNotify("Details updated"))
      .catch((error) => this.cb.onNotify(String(error)));
  }

  private confirmRemove(book: BookMeta) {
    confirmModal({
      title: "Remove from library?",
      // The title comes from the EPUB's own metadata, so it is attacker
      // controlled and must be escaped before reaching innerHTML.
      body:
        `“${escapeHtml(book.title)}” will be removed from Lumen, along with its ` +
        `bookmarks and highlights. The file <code>${escapeHtml(
          book.path.split(/[\\/]/).pop() || "…",
        )}</code> is not deleted from disk.`,
      confirmLabel: "Remove",
      danger: true,
      onConfirm: () => {
        // The row has to actually leave SQLite. `forget()` alone only filtered
        // the in-memory array, so `refreshLibrary()` brought the book straight
        // back and the confirmation was a lie.
        void api
          .removeBook(book.id)
          .then(() => this.forget(book.id))
          .catch((error) => this.cb.onNotify(String(error)));
        this.cb.onNotify("Removed from library");
      },
    });
  }

  /* ------------------------------------------------------------ import */

async importViaDialog() {
    const paths = await pickEpubs();
    if (paths.length) await this.importPaths(paths);
  }

  /**
   * Import a list of paths and return what was actually added.
   *
   * The returned `BookMeta` carries the *canonicalised* path, which on Windows
   * differs from the path the caller passed in. Callers must therefore use the
   * returned rows rather than trying to match on the input paths.
   */
  async importPaths(paths: string[]): Promise<BookMeta[]> {
    try {
      const report = await api.importBooks(paths);
      for (const book of report.added) coverCache.delete(book.id);
      if (report.added.length) {
        this.cb.onNotify(
          report.added.length === 1
            ? `Added “${report.added[0].title}”`
            : `Added ${report.added.length} books`,
        );
      }
      if (report.failed.length) {
        this.cb.onNotify(
          report.failed.length === 1
            ? `Could not add ${basename(report.failed[0].path)}: ${report.failed[0].error}`
            : `${report.failed.length} files could not be added: ${report.failed
                .map((f) => basename(f.path))
                .join(", ")}`,
        );
      }
      await this.cb.onChanged();
      return report.added;
    } catch (error) {
      this.cb.onNotify(String(error));
      return [];
    }
  }
}

function basename(path: string): string {
  return path.split(/[\\/]/).pop() || path;
}

const SORTS: Record<string, (list: BookMeta[]) => BookMeta[]> = {
  recent: (list) =>
    list.sort((a, b) =>
      (b.last_opened ?? b.added_at) - (a.last_opened ?? a.added_at),
    ),
  added: (list) => list.sort((a, b) => b.added_at - a.added_at),
  title: (list) =>
    list.sort((a, b) => a.title.localeCompare(b.title, undefined, { sensitivity: "base" })),
  author: (list) =>
    list.sort((a, b) =>
      (a.author || "zzz").localeCompare(b.author || "zzz", undefined, {
        sensitivity: "base",
      }),
    ),
  progress: (list) => list.sort((a, b) => b.progress - a.progress),
};

/* -------------------------------------------------------------- overlays */

export interface MenuItem {
  label?: string;
  iconPath?: string;
  danger?: boolean;
  separator?: boolean;
  action?: () => void;
}

let activeMenu: HTMLElement | null = null;
let activeModal: HTMLElement | null = null;
let activeScrim: HTMLElement | null = null;

/**
 * Tear down whichever overlay is open. Every overlay registers here so that
 * Escape, the close buttons and a fresh open cannot leave a scrim behind — a
 * stranded full-screen scrim silently swallows every click and the wheel, and
 * looks exactly like a hung app.
 */
export function closeOverlays() {
  activeMenu?.remove();
  activeMenu = null;
  activeModal?.remove();
  activeModal = null;
  if (activeScrim) {
    activeScrim.remove();
    activeScrim = null;
  }
  document.querySelectorAll(".scrim.overlay-owned").forEach((node) => node.remove());
}

/** Build a themed scrim that is tracked by `closeOverlays`. */
export function overlayScrim(onClick: () => void): HTMLElement {
  const scrim = el("div", "scrim overlay-owned");
  scrim.addEventListener("click", onClick);
  activeScrim = scrim;
  return scrim;
}

export function openContextMenu(
  x: number,
  y: number,
  items: MenuItem[],
) {
  closeOverlays();
  const menu = el("div", "context-menu");
for (const item of items) {
    if (item.separator) {
      menu.appendChild(document.createElement("hr"));
      continue;
    }
    const btn = el("button", item.danger ? "danger" : undefined, item.label ?? "");
    btn.type = "button";
    if (item.iconPath) btn.appendChild(icon(item.iconPath));
    btn.addEventListener("click", () => {
      closeOverlays();
      item.action?.();
    });
    menu.appendChild(btn);
  }
  document.body.appendChild(menu);
  const rect = menu.getBoundingClientRect();
  menu.style.left = `${Math.min(x, window.innerWidth - rect.width - 8)}px`;
  menu.style.top = `${Math.min(y, window.innerHeight - rect.height - 8)}px`;
  activeMenu = menu;

  const dismiss = (event: Event) => {
    if (!menu.contains(event.target as Node)) {
      closeOverlays();
      window.removeEventListener("pointerdown", dismiss, true);
    }
  };
  setTimeout(() => window.addEventListener("pointerdown", dismiss, true), 0);
}

export function confirmModal(options: {
  title: string;
  body: string;
  confirmLabel?: string;
  danger?: boolean;
  onConfirm: () => void;
}) {
  closeOverlays();
  const scrim = el("div", "modal");
  const box = el("div", "modal-box");
  box.appendChild(el("h3", undefined, options.title));
  const body = el("p");
  body.innerHTML = options.body;
  box.appendChild(body);
  const actions = el("div", "modal-actions");
  const cancel = el("button", "btn", "Cancel");
  cancel.type = "button";
  const ok = el("button", `btn ${options.danger ? "btn-danger" : "btn-primary"}`, options.confirmLabel ?? "OK");
  ok.type = "button";
  actions.append(cancel, ok);
  box.appendChild(actions);
  scrim.appendChild(box);
  document.body.appendChild(scrim);
  activeModal = scrim;

  // One capture-phase listener per modal, torn down on close. Without this the
  // handler leaked: after a single use of this dialog, Escape stopped reaching
  // the rest of the app (it called stopPropagation), and Enter re-fired every
  // historical onConfirm.
  const abort = new AbortController();
  const done = () => {
    abort.abort();
    closeOverlays();
  };

  cancel.addEventListener("click", done);
  ok.addEventListener("click", () => {
    done();
    options.onConfirm();
  });
  scrim.addEventListener("click", (event) => {
    if (event.target === scrim) done();
  });
  document.addEventListener(
    "keydown",
    (event) => {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        done();
      } else if (event.key === "Enter") {
        event.preventDefault();
        done();
        options.onConfirm();
      }
    },
    { capture: true, signal: abort.signal },
  );
  ok.focus();
}

/**
 * In-app replacement for `window.prompt`, which wry does not enable — the
 * native version resolves to null and shows nothing, so every rename and
 * metadata edit silently did nothing.
 */
export function promptModal(options: {
  title: string;
  label: string;
  value: string;
  confirmLabel?: string;
  onConfirm: (value: string) => void;
}): Promise<string | null> {
  return new Promise((resolve) => {
    closeOverlays();
    const scrim = el("div", "modal");
    const box = el("div", "modal-box");
    box.appendChild(el("h3", undefined, options.title));

    const field = el("div", "field");
    field.appendChild(el("div", "field-label", options.label));
    const input = document.createElement("input");
    input.className = "input";
    input.value = options.value;
    input.spellcheck = false;
    field.appendChild(input);
    box.appendChild(field);

    const actions = el("div", "modal-actions");
    const cancel = el("button", "btn", "Cancel");
    cancel.type = "button";
    const ok = el("button", "btn btn-primary", options.confirmLabel ?? "Save");
    ok.type = "button";
    actions.append(cancel, ok);
    box.appendChild(actions);
    scrim.appendChild(box);
    document.body.appendChild(scrim);
    activeModal = scrim;

    const abort = new AbortController();
    let done = false;
    const finish = (result: string | null) => {
      if (done) return;
      done = true;
      abort.abort();
      closeOverlays();
      if (result !== null) options.onConfirm(result);
      resolve(result);
    };

    cancel.addEventListener("click", () => finish(null));
    ok.addEventListener("click", () => finish(input.value));
    scrim.addEventListener("click", (event) => {
      if (event.target === scrim) finish(null);
    });
    input.addEventListener("keydown", (event) => {
      if (event.key === "Enter") {
        event.preventDefault();
        finish(input.value);
      }
    });
    document.addEventListener(
      "keydown",
      (event) => {
        if (event.key === "Escape") {
          event.preventDefault();
          event.stopPropagation();
          finish(null);
        }
      },
      { capture: true, signal: abort.signal },
    );
    input.focus();
    input.select();
  });
}

export { coverCache };

/** Small helper for the book-details popover used by the card tooltip. */
export function bookSummary(book: BookMeta): string {
  return [
    book.author,
    book.publisher,
    book.chapters ? `${book.chapters} sections` : null,
    formatBytes(book.file_size),
    book.last_opened ? `Last read ${relativeTime(book.last_opened)}` : null,
    `Added ${formatDate(book.added_at)}`,
  ]
    .filter(Boolean)
    .join("\n");
}