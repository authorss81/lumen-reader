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
suspicious names are never indexed at all, and no path derived from a book is
ever handed to the filesystem — every read goes through the zip name index.

**2. Decompression is capped.** 512 MiB for the archive on disk, 32 MiB per
entry, 20 000 entries, plus a compression-ratio check. There is deliberately
**no cumulative uncompressed budget**: the per-entry cap and the ratio guard are
what bound a hostile book.

**3. Scripts cannot run.** Every chapter body is sanitised with
[`ammonia`](https://github.com/rust-ammonia/illuminati) (an allow-list built on
`html5ever`). `<script>`, `<iframe>`, `<object>`, `<embed>`, `<form>` and
`<noscript>` are dropped along with their contents. `on*` handlers are stripped.
Links are rewritten from `href` to `data-href` so the document contains no live
navigation, and external links are never followed automatically.

**4. CSS cannot execute, and cannot escape its element.** `@import`,
`@charset`, `expression()`, `-moz-binding`, `behavior`, `javascript:` and
`vbscript:` are removed, and every `url()` except `data:image/*` becomes `none`.
The stylesheet is then passed through a guard that makes it structurally
impossible to close its own `<style>` element — without which a book's `.css`
file could have injected markup into the privileged webview *after* the
sanitizer ran. A regression test covers this.

**5. Images cannot track you.** Every `<img>` is resolved inside the archive and
inlined as a `data:` URL under a byte budget. A remote `src` simply fails to
resolve and is dropped — the reader makes **zero** network requests.

**6. A strict CSP** (`default-src 'self'`, `frame-src 'none'`,
`object-src 'none'`, `base-uri 'none'`, `form-action 'none'`) backs all of the
above up at the engine level.

**7. The `asset` protocol is disabled**, so no local file can be addressed by
the webview.

Known remaining gaps are tracked honestly in [docs/AUDIT.md](docs/AUDIT.md)
(CSS escape sequences such as `\75 rl(` and `image-set()` are not recognised by
the scrubber; CSP still blocks the resulting fetch).

Our own code contains no `unsafe` block. The dependency tree does — `zip`,
`html5ever` and rusqlite's C SQLite all use it.

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
cargo test --manifest-path src-tauri/Cargo.toml   # dev profile; --release will not link
npm run build                                      # tsc --noEmit + vite build
```
The Rust suite builds its own hostile specimen EPUB with `zip::ZipWriter`, so it
never skips and needs no fixture on disk. There are no JavaScript tests yet —
`reader.ts` is around 900 lines and currently uncovered; that is tracked in
[docs/AUDIT.md](docs/AUDIT.md) as T-07.

### Release
```bash
npm run tauri build
```
Produces `src-tauri/target/release/epub-reader.exe` and, with bundling
enabled, NSIS and MSI installers under `src-tauri/target/release/bundle/`.

The release profile uses `lto = true`, `codegen-units = 1`,
`opt-level = 3`, `strip = true` and **`panic = "abort"`**. The last one matters:
tests must run in the dev profile, so `cargo test --release` will not link.
Expect a cold release build to take on the order of ten minutes, and note that
the LTO link is memory-hungry — it is the slowest and most memory-intensive step
in the whole project.

**Builds belong in CI.** `.github/workflows/release.yml` runs the typecheck,
the Rust tests, and the bundle on `windows-latest` for free, and uploads the
`.exe`, the NSIS installer and the MSI. Push to `main` or tag `v*` to trigger
it, or use the **Build release** workflow in the Actions tab. CI is the
recommended path: it is faster and more predictable than a local release build,
and it keeps a few gigabytes of intermediates off your disk.

### Disk space

`src-tauri/target/` is the only thing that grows, and it is gitignored — no
build output is ever pushed. A full debug + release tree runs to several
gigabytes, essentially all of it throwaway intermediates:

| Path | Approx. size | Safe to delete? |
| --- | --- | --- |
| `src-tauri/target/debug` | 2–5 GB | Yes. Only `cargo test` needs it, and CI runs the tests. |
| `src-tauri/target/release/deps` | 1–2 GB | Yes, but the next local build then takes a full cold rebuild. |
| `src-tauri/target/release/build` | ~0.5 GB | Yes, same caveat. |
| `src-tauri/target/release/epub-reader.exe` | ~8 MB | No — this is the app. |
| `src-tauri/target/release/bundle/` | ~7 MB | No — the installers. |
| `node_modules` | ~70 MB | Only if you are not developing. |

Deleting the debug profile is the single biggest win and costs nothing:
```powershell
Remove-Item src-tauri\target\debug -Recurse -Force
```

---

## Project layout

```
src/                     frontend (TypeScript, no framework)
  api.ts                 typed wrappers over the Tauri commands
  library.ts             shelf UI, import, overlays, context menus
  reader.ts              pagination engine, scroll mode, selection, highlights
  settings.ts            reading preferences sheet
  main.ts                wiring, keyboard map, panels
  styles.css             design tokens and layout

src-tauri/src/
  lib.rs                 plugin wiring, single instance, shell-open queue
  epub.rs                ZIP → container → OPF/NCX → sanitised chapter
  model.rs               shared types and safety limits
  store.rs               SQLite schema and queries
  commands.rs            the Tauri command surface
  tests.rs               parser and sanitizer tests (specimen built in-process)
src-tauri/nsis/
  installer.nsh          .epub file association, removed on uninstall

docs/AUDIT.md            known findings, ranked, with fix status
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