# Lumen Reader — audit findings

Produced by parallel review agents (EPUB/security, reader layout, UX + Windows,
visual design, features, tests/docs) plus manual reproduction.
Status legend: **FIXED** = in a commit on `main`; **OPEN** = not yet addressed.

---

## Part 1 — Bugs the user actually hit

### F-01 · Double-clicking an `.epub` opens the home screen instead of the book — **FIXED**
`src/main.ts` `drainPendingOpens()` matched the imported book by comparing
`book.path` against the raw command-line argument. `probe()` in
`commands.rs` stores the **canonicalised** path, and on Windows
`std::fs::canonicalize` returns a `\\?\` verbatim path. The two never matched,
so `first` was `undefined` and the only feedback was the toast
"Could not open …". The book *was* imported correctly — the UI just never
opened it.

Fix: `importPaths` now returns the `ImportReport` and the pending-open handler
uses `report.added[0]` directly. No path comparison at all.

### F-02 · Continuous scroll pins text to the extreme corner — **FIXED**
`styles.css` had
`.epub-viewport.is-scrolling .epub-content { position: relative; top: 0 !important; left: 0 !important; }`.
Those `!important` values beat the inline `style.top`/`style.left` that
`relayout()` sets from the margin settings, so all text hugged the top-left of
the window. `padding-bottom: 40vh` was also window-relative rather than
reader-relative.

Fix: drop the `top/left !important` overrides; scroll mode now applies
`marginX` symmetrically and `marginY` as padding, with the trailing
breathing space computed from the viewport height.

### F-03 · Selecting text flips the page, so the rest of the line "disappears" — **FIXED**
`bindDragPaging()` in `reader.ts` fired `next()`/`prev()` on any pointer drag
with `|dx| > 70 && |dx| > |dy|`. Selecting a sentence within one line produces
exactly that gesture (`dx ≈ 200, dy ≈ 10`), so every ordinary text selection
turned the page — the selected line's remaining words appeared to be deleted.
Because highlights are created from a selection, this made bookmarking look
broken.

Fix: bail out when a non-empty selection exists at `pointerup`, raise the
threshold, and require the drag to end outside the content column.

### F-04 · Highlighting silently fails when the phrase spans two text nodes — **FIXED**
`wrapFirstOccurrence()` only wrapped matches contained in a **single** text
node and returned `false` otherwise. Any selection crossing an `<em>`,
`<span>` or `<a>` therefore produced no highlight — while the annotation row
had already been written to SQLite. The user sees "nothing happened" and
highlights the same phrase again, creating duplicates.

It also measured the wrap length from the **whitespace-collapsed** needle
against the **uncollapsed** source text, so partial matches wrapped the wrong
span.

Fix: walk the text nodes accumulating length and build the `<mark>` across
node boundaries, splitting each node it crosses. Whitespace is normalised on
both sides before matching.

---

## Part 2 — Security (ranked)

### S-01 · CRITICAL — CSS `<link>` path allows HTML injection into the app — **FIXED**
`epub.rs` sanitised only the document body with `ammonia`. The harvested
stylesheet was then interpolated into the returned HTML **after** the final
gate:

```rust
"<style data-epub=\"css\">\n{css}\n</style>\n<div class=\"epub-doc\">…"
```

`scrub_css()` never removed the literal sequence `</style`. A book whose
`.css` file contains `</style><img src=x onerror=…>` therefore closed the
style element and injected markup into the privileged webview, ahead of
`.epub-doc` where the click interceptor is not bound. CSP blocks inline
script, so this is HTML injection / navigation rather than immediate RCE — but
it fully breaks the documented "ammonia is the last gate" invariant.

Fix: the assembled CSS is rejected if it contains `</style` (after comment
stripping and case folding), and a regression test covers it.

### S-02 · HIGH — `build_excerpt` can panic and abort the process — **FIXED**
`epub.rs` derived a slice offset from `str::to_lowercase()`, which is **not
length-preserving** (`U+0130` → 3 bytes from 2). A chapter containing that
character panics on `&text[m_start..m_end]`. `Cargo.toml` sets
`panic = "abort"`, so this is a hard process kill, not a rejected command.

Reproduced: text `"\u{0130}\u{0130}ab"`, query `"ab"`.

Fix: search the case-folded string, then map the offset back with a character
walk that accumulates folded byte lengths.

### S-03 · HIGH — `book.title` reaches `innerHTML` unescaped — **FIXED**
`library.ts` escaped the *path* in the remove-confirmation body but
interpolated the raw `book.title` — which is `<dc:title>` straight out of the
EPUB — then assigned it to `body.innerHTML`. A book titled
`<img src=x onerror=…>` executes markup when you right-click → Remove.

Fix: `escapeHtml(book.title)`. This was the only `innerHTML` sink in the app
that missed escaping.

### S-04 · MEDIUM — CSS escapes and `image-set()` bypass `scrub_css` — **FIXED**
All three `url()` patterns require the literal ASCII `url(`. CSS escapes are
decoded by the engine, so `\75 rl(https://…)` and `image-set("https://…")`
survive scrubbing. CSP still blocks the fetch, so the impact is
defacement-class, not exfiltration.

### S-05 · MEDIUM — SVG `<a xlink:href="data:…">` survives the allow-list — **FIXED**
The `<a>` handler removes `href` but not `xlink:href`, and `xlink:href` is in
the generic attribute list while `url_schemes(["data"])` permits `data:`. Not
currently exploitable — the reader calls `preventDefault()` on any `closest("a")`
— but it rests on browser behaviour rather than the code.

### S-06 · LOW — "512 MiB total uncompressed" is not enforced — **OPEN**
`MAX_TOTAL_UNCOMPRESSED` is compared against the file's **compressed** on-disk
length. There is no cumulative accounting across reads. The real protections
are the 32 MiB per-entry cap and the ratio check. The README overstated this.

### S-07 · LOW — percent-decode runs twice on every href — **FIXED**
`resolve_href()` decodes internally *and* four call sites pre-decode. A zip
entry genuinely named `a%20b.png` becomes unreachable. Not a traversal hole.

---

## Part 3 — Correctness and compatibility

### C-01 · HIGH — UTF-16 / Latin-1 books are unopenable — **FIXED**
`decode_markup()` ignored the BOM and the XML declaration, only scanning for
a `charset` substring in the first 4 KiB. A UTF-16LE EPUB 2 book decoded to
mojibake, `Document::parse` failed, and **the whole book was rejected** — not
just one chapter.

Fix: BOM first (UTF-8/16LE/16BE), then an XML-declaration `encoding=`, then a
real `<meta charset>` element, then UTF-8.

### C-02 · HIGH — spine items pointing at missing files become dead chapters — **FIXED**
`bytes` fell back to `0` for both "no such entry" and "failed to read", and the
chapter was pushed unconditionally. The reader would page into a chapter that
throws.

Fix: entries whose target is absent or unreadable are skipped.

### C-03 · MEDIUM — `dc:*` metadata truncated at the first inline tag — **FIXED**
`node.text()` returns only the first child text node, so
`<dc:title>Mr <em>Bradbury</em></dc:title>` became `"Mr "`, and EPUB 2
`<dc:description><p>An <em>excellent</em> book</p></dc:description>` became
`"An "`. The reader already had `collect_text()` for the TOC; the metadata loop
was not using it.

Fix: metadata now uses `collect_text()`.

### C-04 · MEDIUM — only the first `rootfile` is tried — **FIXED**
One malformed first rootfile makes a multi-rootfile book permanently
unopenable.

### C-05 · MEDIUM — `linear="no"` spine items are paged like normal content — **FIXED**
Footnotes and endnote sections are interleaved into the sequential reading
order. `Chapter.linear` is parsed and never used.

### C-06 · MEDIUM — images with no usable extension are dropped — **FIXED**
`mime_for()` is extension-only, so `OEBPS/images/img001` is rejected even
though the OPF manifest carries the authoritative media type. The `<img src>`
is removed before the failure, so the image vanishes rather than falling back.

### C-07 · LOW — EPUB 2 `<guide>` and NCX `playOrder` are not parsed — **OPEN**

---

## Part 4 — Performance

### P-01 · HIGH — every spine document is decompressed just to read its length — **FIXED**
`parse_manifest()` inflated each spine item into a `Vec<u8>` purely to call
`.len()`. The size is free from the central directory via
`archive.by_index(i).size()`, which does not decompress. Because every command
re-opens the book, a single page turn decompressed the whole book.

### P-02 · HIGH — the manifest cache is consulted after the work it avoids — **FIXED**
`with_epub()` called `Epub::open()` — which parses container.xml, the OPF, the
nav and the NCX — and only then overwrote the manifest from cache. The
optimisation did nothing.

Fix: `Epub::open()` builds the name index only; manifest parsing moved to
`ensure_manifest()`, called on a cache miss. Cache key is now `(mtime, size)`.

### P-03 · MEDIUM — search re-derives every chapter's text per keystroke — **OPEN**
~12 full-document allocations and one inflation per spine item, per debounced
keystroke, with no text cache.

### P-04 · MEDIUM — `search_in_book`, `import_books` and `get_cover` run on the main thread — **FIXED**
Tauri executes synchronous commands on the main thread, so search freezes the
window — and freezes the loading overlay with it.

---

## Part 5 — UX and Windows integration

### X-01 · HIGH — "Remove from library" silently does nothing — **FIXED**
`api.removeBook` and `commands::remove_book` were fully implemented but
**nothing called them**. `forget()` filtered the in-memory array, then
`refreshLibrary()` re-read SQLite and the book reappeared — while the dialog
said "Removed from library".

### X-02 · HIGH — `confirmModal` leaks a capture-phase keydown listener — **FIXED**
Each call added `document.addEventListener("keydown", …, { capture: true })`
and never removed it. After one use, Escape stopped reaching `bindKeys`
entirely (the stale handler calls `stopPropagation()`), so it stopped closing
panels, leaving the reader and closing the settings sheet. Once X-01 is fixed,
Enter would have re-fired every historical `onConfirm`.

Fix: an `AbortController` per modal, aborted on close.

### X-03 · HIGH — closing Settings with × or Escape orphans a full-screen scrim — **FIXED**
`sheet.remove()` never removed its `.scrim`, leaving a
`position: fixed; inset: 0; z-index: 38` overlay over everything. It covered
the topbar (z-index 30), swallowed every click and the wheel, and looked
exactly like a hung app. The Reset handler's
`document.querySelector(".scrim")` matched `#scrim` from `index.html` instead.

Fix: one `closeSheet()` helper removes both; the scrim is tracked and
registered with `closeOverlays()`.

### X-04 · HIGH — the side panel covers the topbar buttons — **FIXED**
`.panel` is `position: fixed; top: 0; z-index: 40` over a `.topbar` at
`z-index: 30`, so all five topbar icons — including Settings and the panel's
own close — were unclickable while a panel was open.

### X-05 · HIGH — `prompt()` is dead in WebView2, so rename does nothing — **FIXED**
Four call sites (edit title, edit author, rename bookmark ×2) rely on
`window.prompt`, which wry does not enable. Each already handles `null` by
returning early, so the UI appears to work and does nothing.

Fix: an in-app `promptModal()` reusing the existing `.modal-box` shell.

### X-06 · MEDIUM — "Match Windows" theme is broken at boot — **FIXED**
`boot()` called `applyTheme(settings.theme)` — the raw stored value — instead
of `applyTheme(effectiveTheme(settings))`, and `watchSystemTheme()` was
exported but never called. So the setting only worked until restart.

### X-07 · MEDIUM — Escape while typing a note discards it — **FIXED**
Escape is handled before the `isTyping()` guard. Closing the panel removes the
focused `<textarea>` without firing `change`, so the note is lost silently.

### X-08 · MEDIUM — `+`/`-` in scroll mode jumps to the top of the chapter — **FIXED**
`nudgeFont()` anchors on `page / (pages - 1)`, and scroll mode forces
`pages = 1`, so the anchor is always 0.

### X-09 · MEDIUM — no window state, and the title never changes — **FIXED (title)**
Every launch is 1280×840 centred on the primary monitor. The window title stays
"Lumen Reader" while reading a book.

### X-10 · MEDIUM — no state for a moved or deleted book file — **OPEN**
The user gets a 3.4-second toast containing a raw Rust error string, the card
still looks healthy, and there is no recovery path.

### X-11 · LOW — import has no spinner, and each failure overwrites the last — **FIXED**
A single `#toast` node means N failures show exactly one.

### X-12 · LOW — `data/book` metadata, `page_list` and `landmarks` are parsed and never shown — **PARTIAL**

---

## Part 6 — Visual design

**All items in this part are now fixed.** See the closing notes at the end.

### D-01 · Contrast — **FIXED**
`--fg-faint` fails WCAG AA in all four themes (2.44–3.77:1); `--fg-muted` fails
in sepia; the danger colour `#d1483a` fails in three. Affected: every empty
state, all secondary metadata, all placeholders.

### D-02 · The dark themes have no elevation — **FIXED**
In the black theme `--bg-sunken` is literally `#000000`, identical to `--bg`,
so inputs, code blocks, settings segments and cover fallbacks are invisible.

### D-03 · The reader has no line-length cap — **FIXED**
Column width is `viewport.clientWidth - marginX * 2`. On a 2560px display that
is ~190 characters per line.

### D-04 · The reader's measure, weight scale and spacing scale are ad hoc — **FIXED**
Twelve font sizes (including a `13.2px` used once), nine weights (seven
non-standard, and Segoe UI has no 520/540/570/620/640), twelve gap values,
six off-grid.

### D-05 · Covers are cropped with `object-fit: cover` — **FIXED**
Landscape and square covers lose most of the image, usually including the
title.

### D-06 · Four settings-drawer layout bugs — **FIXED**
Duplicate value readouts on every slider; the three-option Typeface segment
wraps inside a two-column grid; the four-column grid stretches two-item rows;
theme-swatch labels are near-white on the light and sepia previews.

### D-07 · No focus ring on eleven interactive components — **FIXED**

---

## Part 7 — Tests and docs

### T-01 · HIGH — the integration test silently skips everywhere — **FIXED**
`parses_and_renders_the_specimen_book` returns early if the specimen EPUB is
absent, and nothing — not the repo, not CI — creates it. It has never actually
run. The README claimed "the test suite asserts each of these against a
deliberately hostile specimen book".

Fix: the specimen is now built in-process by the test via `zip::ZipWriter`, so
it always exists. No external fixture, no skip.

### T-02 · README overstatements — **FIXED**
"The specimen EPUB is generated by the build script" was false; there is no
build script. "512 MiB total uncompressed" was false. "ammonia is the only
thing that can emit final HTML" was false (S-01). "Not a single `unsafe`
block" is true of our code but not of the 501-crate dependency tree.
`panic = "abort"` was undocumented, and it means `cargo test --release` fails.

### T-03 · No CI on pull requests — **FIXED**
Triggers are `push: main`, tag `v*`, and manual. A regression lands on main and
builds a release before anything catches it.

### T-04 · Workflow grants `contents: write` to every step — **FIXED**
All four actions are pinned to floating tags, so a moved tag would execute with
write access.

### T-05 · `url = "2"` is entirely unused — **OPEN**
Drags `idna` → `icu4x` (~30 crates) into an LTO'd binary and lengthens the
cold build.

### T-06 · `package.json` is still named after the temp scaffold — **OPEN**
`"name": "c-usersuserappdatalocaltempopencodescaffoldepubreader"`,
`"version": "0.1.0"` vs `1.0.0` in `tauri.conf.json`.

### T-07 · No JS/TS tests at all — **OPEN**
`reader.ts` is ~880 lines with zero coverage, including the column-pitch
arithmetic — the exact class of bug that shipped broken in commit `bb00a8f`.

---

## Verified sound (no action)

- No path derived from an EPUB ever reaches `std::fs`; every read goes through
  the zip name index. Traversal, absolute paths and symlink entries are inert.
- Zip symlinks are not followed — a symlink entry's content is just a string.
- Truncated zips and zero-byte chapters fail cleanly; no panic.
- The nav → NCX → per-spine fallback chain works, including malformed
  `nav.xhtml`.
- `rewrite_tags` is O(n); the regex crate builds a DFA for the attribute
  sub-pattern.
- No secrets; largest tracked file is `Cargo.lock`; both lockfiles committed.
- Per-user NSIS install needs no elevation and removes every key it creates.
---

## Addendum — visual and interaction pass

Closed after the first round of fixes:

- The settings drawer was unclickable. The scrim sat at `z-index: 38` while the
  drawer had been moved to `29` to stop it covering the topbar, so the scrim was
  painted over the drawer. Stacking is now reader < scrim `20` < drawer `29` <
  topbar `30`.
- Page theme swatch labels were invisible on Light and Sepia: the ink was
  hardcoded near-white for all four because the guard compared against
  `"#ffffff"`, which no preview matched. Each swatch now carries its own legible
  ink, and the previews use the real `--page-bg` / `--bg-elev` values.
- The typeface row wrapped "Mono" onto its own line because a fixed 4-column
  track met a 2-column grid. Track count now follows item count.
- Every slider printed its value twice — once in the field label and once in a
  readout the slider drew itself. The slider updates the label instead.
- Highlighting from the middle of a word reported "the text could not be
  located". The wrapper searched a whitespace-collapsed copy of the text and
  then sliced the *raw* text with those offsets; whenever collapsing changed the
  length the offsets landed in the wrong place. It no longer translates offsets
  between two differently-shaped strings: pass one searches each text node's own
  raw data, and pass two maps back with a character walk.
- All of Part 6 (contrast, dark elevation, measure cap, type scale, covers,
  focus rings) is fixed, plus the smaller items the audits noted: the invisible
  cover-menu hit target, the 158px-wide "no match" note, the invisible reader
  chrome in sepia, the dark-theme highlight colours, and the card grid rhythm.

## Still genuinely open

- **S-06** `MAX_TOTAL_UNCOMPRESSED` was renamed to `MAX_ARCHIVE_BYTES` to stop
  the name implying a cumulative budget that is not enforced. The name is now
  honest, but there is still no cumulative expansion budget.
- **C-07** the EPUB 2 `<guide>` and NCX `playOrder` are not parsed. Books that
  rely on `<guide>` for their reading order fall back to the spine, which is
  usually but not always right.
- **P-03** search re-derives every chapter's text on each keystroke. Debounced
  at 220 ms, so it is not noticeable on a short book, but a long one will lag.
- **X-10** no state for a book whose file has moved or been deleted. The row
  stays in the library and fails on open, with no way to relink it.
- **X-12** `dc:*` metadata and `landmarks` are parsed and still never shown.
  `page_list` is now rendered in the contents panel.
- **T-05** `url = "2"` in `tauri.conf.json` is unused.
- **T-06** `package.json` is still named after the temp scaffold.
- **T-07** no JavaScript or TypeScript tests. `reader.ts` is around 1100 lines and
  the column-pitch arithmetic had already shipped broken once. This is the
  largest remaining risk: the front end has no automated coverage at all, and
  three of the bugs fixed in this pass (settings rebinding, the hidden scroll
  chrome, and highlights with no stored position) were all front-end-only and
  would not have been caught by the Rust suite.
