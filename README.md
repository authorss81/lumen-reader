# Lumen Reader

A fast, secure EPUB 3 reader for Windows, written in **Rust** with a **Tauri 2**
webview frontend.

No accounts, no telemetry, no network calls. Your library, your notes, your
progress — all stored locally in a single SQLite file.

---

## Why Rust

An EPUB is untrusted input: a ZIP archive containing XML and HTML that ends up
rendered by a browser engine. That makes the reader itself an attack surface, so
the language choice matters more than usual.

| Approach | RAM | Memory safety | Verdict |
| --- | --- | --- | --- |
| **Rust + Tauri 2** | ~40 MB | Enforced by the compiler | **This project** |
| Rust + egui | ~15 MB | Enforced | Great, but no HTML/CSS fidelity |
| C++ / Qt (Calibre, KOReader) | ~60 MB | Manual, `unsafe` | Large attack surface |
| TypeScript + Electron | 150 MB+ | Managed | Heavy for a reading app |
| Python + Qt | 120 MB+ | Managed | Slow startup |

There is not a single `unsafe` block in this codebase.

---

## Features

**Library**
- Import via file picker, drag-and-drop onto the window, or by double-clicking
  any `.epub` in Explorer — installing the app registers it as the default
  handler for the format
- Covers decoded on demand and cached in memory
- Filter by title/author/publisher, sort by recent, added, title, author, progress
- Per-book context menu: edit metadata, show in Explorer, remove (files are never deleted)

**Reading**
- Two reading styles: **paginated** (CSS multi-column, one screen, no scrollbar)
  or **continuous scroll** for people who prefer a normal document flow
- Mouse wheel and click-drag page in paginated mode, with a threshold so a
  trackpad flick turns exactly one page
- Navigation via click zones, keyboard, slider, or full table of contents
- Cross-document and `#fragment` links, external links routed through an explicit menu
- Four themes (Light, Sepia, Dark, Black), optional **match Windows** light/dark
  preference, three typefaces, adjustable size, line height, letter spacing,
  justification and margins
- Fullscreen (`F`), page slider, percentage + page counter

**Notes and bookmarks**
- Select text to highlight in four colours, with optional note
- Bookmarks (`B`) capture the **exact page**, not just the chapter, and are
  labelled with the first line of text on that page
- Rename any bookmark (double-click or the pencil button), jump to it from
  anywhere, and delete it
- The bookmark list shows chapter, page number and date, and marks which
  bookmark matches where you are reading right now
- Per-book progress saved automatically, resumes exactly where you left off

**Search**
- Full-text across the entire spine, results with highlighted excerpts and
  click-to-jump
- The side panel's filter box narrows the contents list, bookmarks and notes

### Keyboard shortcuts

| Key | Action |
| --- | --- |
| `←` `→` `Space` `PageUp` `PageDown` `J` `K` | Previous / next page |
| `T` | Table of contents |
| `S` | Search this book |
| `B` | Toggle bookmark |
| `N` | Highlights and notes |
| `F` | Fullscreen |
| `+` `-` | Font size |
| `Esc` | Close panel, or return to the library |

---

## Windows integration

- Registers `.epub` as the default handler on install (per-user, no elevation)
  and removes every key it created on uninstall
- Single-instance: launching a second book raises the existing window and opens
  it there, instead of starting a second copy
- Native window frame, own icon, Start menu entry, own taskbar button
- `withGlobalTauri` is off, so no dev globals are injected into the webview
- Nothing is phoned home: the app makes zero network requests

---

## Security model

EPUB content is hostile by default. Every layer below is enforced in Rust, not
in the webview.

**1. Path traversal is impossible.** Hrefs are resolved through a segment
normaliser that refuses `..` past the archive root, backslashes, NUL bytes,
absolute paths and percent-encoded traversal (`%2e%2e`). Zip entries with
suspicious names are never indexed at all.

**2. Decompression bombs are capped.** Hard limits: 512 MiB total uncompressed,
32 MiB per entry, 20 000 entries, plus a compression-ratio check on small
entries.

**3. Scripts cannot run.** Every chapter is sanitised with
[`ammonia`](https://github.com/rust-ammonia/illuminati) (an allow-list built on
`html5ever`), applied *last* as the final gate. `<script>`, `<iframe>`,
`<object>`, `<embed>`, `<form>` and `<noscript>` are dropped along with their
contents. `on*` handlers are stripped. Links are rewritten from `href` to
`data-href` so the document contains no live navigation, and external links are
never followed automatically.

**4. CSS cannot execute or phone home.** `@import`, `@charset`,
`expression()`, `-moz-binding`, `behavior`, `javascript:` and `vbscript:` are
removed, and every `url()` except `data:image/*` becomes `none`. Stylesheet
content is scrubbed in two independent places so the guarantee does not depend
on call ordering.

**5. Images cannot track you.** Every `<img>` is resolved inside the archive and
inlined as a `data:` URL under a byte budget. A remote `src` simply fails to
resolve and is dropped — the reader makes **zero** network requests.

**6. A strict CSP** (`default-src 'self'`, `frame-src 'none'`,
`object-src 'none'`, `base-uri 'none'`, `form-action 'none'`) backs all of the
above up at the engine level.

**7. The `asset` protocol is disabled**, so no local file can be addressed by
the webview.

The test suite asserts each of these against a deliberately hostile specimen
book.

---

## Building from source

### Prerequisites
- [Rust](https://rustup.rs) (`rustup` — MSVC toolchain)
- [Node.js](https://nodejs.org) 20+
- Visual Studio Build Tools with the C++ workload (linker for MSVC targets)
- WebView2 runtime (preinstalled on Windows 11 / current Windows 10)

### Development
```bash
npm install
npm run tauri dev
```

### Tests
```bash
cargo test --manifest-path src-tauri/Cargo.toml
npm run build            # tsc --noEmit + vite build
```

### Release
```bash
npm run tauri build
```
Produces `src-tauri/target/release/epub-reader.exe` and, with bundling
enabled, NSIS and MSI installers under `src-tauri/target/release/bundle/`.

The release profile uses `lto = true`, `codegen-units = 1`,
`opt-level = 3` and `strip = true`. Expect a slow first link (~3–6 minutes) on a
laptop-class CPU; subsequent builds are cached. CI does this for you via the
**Build release** workflow — see `.github/workflows/release.yml`.

---

## Project layout

```
src/                     frontend (TypeScript, no framework)
  api.ts                 typed wrappers over the Tauri commands
  library.ts             shelf UI, import, context menus
  reader.ts              pagination engine, selection, highlights, search
  settings.ts            reading preferences sheet
  main.ts                wiring, keyboard map, panels
  styles.css             design tokens and layout

src-tauri/src/
  epub.rs                ZIP → container → OPF/NCX → sanitised chapter
  model.rs               shared types and safety limits
  store.rs               SQLite schema and queries
  commands.rs            the Tauri command surface
  tests.rs               parser and sanitizer tests
```

### How a chapter is rendered

```
EPUB zip
  └─ container.xml            → OEBPS/content.opf
       ├─ <metadata>          → title, author, language, cover
       ├─ <manifest>           → id → href map, nav + ncx discovery
       └─ <spine>              → ordered chapter list
  └─ chapter.xhtml
       1. decode (BOM / <meta charset>)
       2. harvest <style> bodies and <link rel=stylesheet>
       3. inline images as data: URLs (budgeted)
       4. rewrite <a href> → <a data-href>, scrub style attributes
       5. ammonia allow-list   ← the only thing that can emit final HTML
       6. attach scrubbed CSS and return a self-contained document
```

---

## Licence

MIT — see [LICENSE](LICENSE). The specimen EPUB used by the tests is generated
by the build script and is public domain.