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
      const note = el("p", "panel-empty", "No books match that filter.");
      this.grid.appendChild(note);
      return;
    }

    const frag = document.createDocumentFragment();
    for (const book of list) frag.appendChild(this.card(book));
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
    if (cached) {
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

  private editDetails(book: BookMeta) {
    const title = prompt("Title", book.title);
    if (title === null) return;
    const author = prompt("Author", book.author);
    if (author === null) return;
    const trimmed = title.trim() || book.title;
    void api
      .updateBook(book.id, trimmed, author.trim())
      .then(() => {
        book.title = trimmed;
        book.author = author.trim();
        this.render();
        this.cb.onNotify("Details updated");
      })
      .catch((e) => this.cb.onNotify(String(e)));
  }

  private confirmRemove(book: BookMeta) {
    confirmModal({
      title: "Remove from library?",
      body: `“${book.title}” will be removed from Lumen. The ${escapeHtml(
        book.path.split(/[\\/]/).pop() || "file",
      )} file on disk is not deleted, along with your bookmarks and notes.`,
      confirmLabel: "Remove",
      danger: true,
      onConfirm: () => {
        this.forget(book.id);
        this.cb.onNotify("Removed from library");
      },
    });
  }

  /* ------------------------------------------------------------ import */

async importViaDialog() {
    const paths = await pickEpubs();
    if (paths.length) await this.importPaths(paths);
  }

  async importPaths(paths: string[]) {
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
      for (const failure of report.failed) {
        this.cb.onNotify(
          `${failure.path.split(/[\\/]/).pop()}: ${failure.error}`,
        );
      }
      await this.cb.onChanged();
    } catch (error) {
      this.cb.onNotify(String(error));
    }
  }
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

export function closeOverlays() {
  activeMenu?.remove();
  activeMenu = null;
  activeModal?.remove();
  activeModal = null;
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

  const done = () => closeOverlays();
  cancel.addEventListener("click", done);
  scrim.addEventListener("click", (event) => {
    if (event.target === scrim) done();
  });
  document.addEventListener(
    "keydown",
    (event) => {
      if (event.key === "Escape" || event.key === "Enter") {
        event.preventDefault();
        event.stopPropagation();
        done();
        if (event.key === "Enter") options.onConfirm();
      }
    },
    { capture: true },
  );
  ok.focus();
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