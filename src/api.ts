import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";

export interface BookMeta {
  id: string;
  path: string;
  title: string;
  author: string;
  language: string;
  publisher: string;
  description: string;
  identifier: string;
  rights: string;
  cover_href: string | null;
  file_size: number;
  mtime: number;
  added_at: number;
  last_opened: number | null;
  progress: number;
  locator: string;
  chapters: number;
}

export interface TocNode {
  title: string;
  href: string;
  depth: number;
  children: TocNode[];
}

export interface Chapter {
  href: string;
  title: string;
  index: number;
  bytes: number;
  linear: boolean;
}

export interface BookDetail extends Omit<BookMeta, "chapters"> {
  toc: TocNode[];
  page_list: TocNode[];
  landmarks: TocNode[];
  chapters: Chapter[];
  cover: string | null;
}

export interface ChapterContent {
  href: string;
  title: string;
  index: number;
  html: string;
  is_last: boolean;
  has_prev: boolean;
  has_next: boolean;
}

export interface SearchHit {
  href: string;
  title: string;
  chapter_index: number;
  excerpt: string;
}

export interface Annotation {
  id: number;
  book_id: string;
  href: string;
  selector: string;
  quote: string;
  note: string;
  color: string;
  created_at: number;
  modified_at: number;
}

export interface Bookmark {
  id: number;
  book_id: string;
  href: string;
  locator: string;
  label: string;
  created_at: number;
}

export interface ImportFailure {
  path: string;
  error: string;
}

export interface ImportReport {
  added: BookMeta[];
  failed: ImportFailure[];
}

export interface Locator {
  chapter: number;
  page: number;
  fraction: number;
  fragment?: string;
}

export const api = {
  importBooks: (paths: string[]) => invoke<ImportReport>("import_books", { paths }),
  listBooks: () => invoke<BookMeta[]>("list_books"),
  openBook: (id: string) => invoke<BookDetail>("open_book", { id }),
  loadChapter: (id: string, index: number) =>
    invoke<ChapterContent>("load_chapter", { id, index }),
  searchInBook: (id: string, query: string, limit = 300) =>
    invoke<SearchHit[]>("search_in_book", { id, query, limit }),
  getCover: (id: string) => invoke<string | null>("get_cover", { id }),
  removeBook: (id: string) => invoke<void>("remove_book", { id }),
  updateBook: (id: string, title: string, author: string) =>
    invoke<void>("update_book", { id, title, author }),
  saveProgress: (id: string, progress: number, locator: string) =>
    invoke<void>("save_progress", { id, progress, locator }),
  revealInExplorer: (path: string) => invoke<void>("reveal_in_explorer", { path }),

  listAnnotations: (bookId: string, href?: string) =>
    invoke<Annotation[]>("list_annotations", { bookId, href }),
  addAnnotation: (payload: {
    book_id: string;
    href: string;
    selector: string;
    quote: string;
    note: string;
    color: string;
  }) => invoke<Annotation>("add_annotation", { payload }),
  updateAnnotation: (payload: {
    id: number;
    note?: string;
    color?: string;
    quote?: string;
  }) => invoke<void>("update_annotation", { payload }),
  deleteAnnotation: (id: number) => invoke<void>("delete_annotation", { id }),

  listBookmarks: (bookId: string) => invoke<Bookmark[]>("list_bookmarks", { bookId }),
  addBookmark: (bookId: string, href: string, locator: string, label: string) =>
    invoke<Bookmark>("add_bookmark", { bookId, href, locator, label }),
  deleteBookmark: (id: number) => invoke<void>("delete_bookmark", { id }),
  renameBookmark: (id: number, label: string) =>
    invoke<void>("rename_bookmark", { id, label }),

  getSettings: () => invoke<Record<string, unknown>>("get_settings"),
  setSetting: (key: string, value: unknown) =>
    invoke<void>("set_setting", { key, value }),
  libraryPath: () => invoke<string>("library_path"),
  /** Drains EPUB paths handed over by the shell (file association / relaunch). */
  takePendingOpens: () => invoke<string[]>("take_pending_opens"),
};

export async function pickEpubs(): Promise<string[]> {
  const selected = await open({
    multiple: true,
    filters: [{ name: "EPUB books", extensions: ["epub"] }],
  });
  if (!selected) return [];
  return Array.isArray(selected) ? selected : [selected];
}

/* ---------------------------------------------------------------- utils */

export function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  className?: string,
  text?: string,
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

export function icon(path: string): SVGElement {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("aria-hidden", "true");
  const p = document.createElementNS("http://www.w3.org/2000/svg", "path");
  p.setAttribute("d", path);
  svg.appendChild(p);
  return svg;
}

export function escapeHtml(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

export function formatDate(epochSeconds: number): string {
  if (!epochSeconds) return "—";
  return new Date(epochSeconds * 1000).toLocaleDateString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
  });
}

export function relativeTime(epochSeconds: number | null): string {
  if (!epochSeconds) return "Not started";
  const diff = Date.now() / 1000 - epochSeconds;
  if (diff < 60) return "Just now";
  if (diff < 3600) return `${Math.floor(diff / 60)} min ago`;
  if (diff < 86400) return `${Math.floor(diff / 3600)} h ago`;
  if (diff < 604800) return `${Math.floor(diff / 86400)} d ago`;
  return formatDate(epochSeconds);
}

export function formatBytes(bytes: number): string {
  if (!bytes) return "—";
  const units = ["B", "KB", "MB", "GB"];
  let value = bytes;
  let i = 0;
  while (value >= 1024 && i < units.length - 1) {
    value /= 1024;
    i += 1;
  }
  return `${value.toFixed(value >= 10 || i === 0 ? 0 : 1)} ${units[i]}`;
}