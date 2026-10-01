//! Tests for the security-critical parts of the EPUB pipeline.
//!
//! Run with `cargo test --manifest-path src-tauri/Cargo.toml`.

use std::path::PathBuf;

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
    assert_eq!(resolve_href("OEBPS", "/root.xhtml").as_deref(), Some("root.xhtml"));
}

#[test]
fn resolve_href_rejects_traversal_and_remote() {
    // Climbing above the archive root must fail rather than escape.
    assert_eq!(resolve_href("OEBPS", "../../../etc/passwd"), None);
    assert_eq!(resolve_href("OEBPS/text", "../../../.."), None);
    // Remote and dangerous schemes are never resolved to a local path.
    assert_eq!(resolve_href("OEBPS", "http://evil.example/x.xhtml"), None);
    assert_eq!(resolve_href("OEBPS", "https://evil.example/x.xhtml"), None);
    assert_eq!(resolve_href("OEBPS", "//evil.example/x.xhtml"), None);
    assert_eq!(resolve_href("OEBPS", "javascript:alert(1)"), None);
    assert_eq!(resolve_href("OEBPS", "data:text/html,<script>"), None);
    assert_eq!(resolve_href("OEBPS", "file:///C:/Windows/System32"), None);
    // Backslashes and NUL bytes are rejected outright.
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
    assert!(!out.contains("@import"), "{out}");
    assert!(!out.contains("@charset"), "{out}");
    assert!(!out.contains("javascript:"), "{out}");
    assert!(!out.contains("expression("), "{out}");
    assert!(!out.contains("-moz-binding"), "{out}");
    assert!(!out.contains("behavior"), "{out}");
    assert!(!out.contains("evil.example"), "{out}");
    // Harmless declarations survive untouched.
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
        "<object", "<embed", "<form", "<input", "javascript:",
        "evil.example",
    ] {
        assert!(!out.contains(needle), "leaked {needle} in:\n{out}");
    }
    // Legitimate content is preserved.
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
    let text = html_to_text("<div><p>one</p><p>two</p></div>");
    assert_eq!(text, "one two");
    let text = html_to_text("<style>p{color:red}</style><p>visible</p>");
    assert!(text.contains("visible"));
    assert!(!text.contains("color:red"));
}

#[test]
fn stable_id_is_deterministic() {
    assert_eq!(stable_id("C:/books/a.epub"), stable_id("C:/books/a.epub"));
    assert_ne!(stable_id("C:/books/a.epub"), stable_id("C:/books/b.epub"));
}

// ---------------------------------------------------------- integration

/// Opens the generated specimen if it is present, exercising the real parser,
/// nav discovery, cover extraction, sanitisation and search.
fn specimen() -> Option<PathBuf> {
    let candidates = [
        std::env::temp_dir().join("opencode").join("The Anatomy of a Reader.epub"),
        PathBuf::from("testdata").join("The Anatomy of a Reader.epub"),
    ];
    candidates.into_iter().find(|p| p.exists())
}

#[test]
fn parses_and_renders_the_specimen_book() {
    let Some(path) = specimen() else {
        eprintln!("specimen not found; skipping");
        return;
    };
    let mut epub = Epub::open(&path).expect("specimen should parse");
    let m = epub.manifest.clone();

    assert_eq!(m.title, "The Anatomy of a Reader");
    assert_eq!(m.author, "A. Nonymous");
    assert_eq!(m.language, "en");
    assert_eq!(m.publisher, "Test Press");
    assert_eq!(m.chapters.len(), 5);
    // Cover discovered via properties="cover-image".
    assert_eq!(m.cover_href.as_deref(), Some("OEBPS/images/cover.png"));
    // Navigation comes from nav.xhtml, preserving the nested list structure.
    let titles: Vec<&str> = m.toc.iter().map(|n| n.title.as_str()).collect();
    assert_eq!(titles, vec!["Prologue", "Chapter One", "Chapter Two", "Appendix"]);
    let nested: Vec<&str> = m
        .toc
        .iter()
        .flat_map(|n| n.children.iter())
        .map(|n| n.title.as_str())
        .collect();
    assert!(nested.contains(&"A Mid-Chapter Anchor"), "{nested:?}");
    assert!(nested.contains(&"A Nested Section"), "{nested:?}");

    // Cover round-trips as a data URL.
    let cover = epub.cover_data_url().unwrap().expect("cover");
    assert!(cover.starts_with("data:image/png;base64,"), "{}", &cover[..40.min(cover.len())]);

    // Every spine document renders, and the hostile one is neutralised.
    for index in 0..m.chapters.len() {
        let content = epub.chapter(index).expect("chapter renders");
        assert!(content.html.contains("data-epub=\"css\""), "chapter {index} lacks css");
        assert!(!content.html.contains("<script"), "chapter {index} kept a script");
        assert!(!content.html.contains("evil.example"), "chapter {index} kept a remote url");
        assert!(!content.html.contains("javascript:"), "chapter {index} kept a js url");
        assert!(!content.html.contains("onload="), "chapter {index} kept onload");
        assert!(!content.html.contains("<iframe"), "chapter {index} kept an iframe");
    }

    // The hostile document's legitimate paragraph survives.
    let hostile = epub.chapter(3).unwrap();
    assert!(hostile.html.contains("This paragraph must survive"), "{}", hostile.html);

    // Internal links are rewritten to data-href instead of a live href.
    let ch2 = epub.chapter(1).unwrap();
    assert!(ch2.html.contains("data-href=\"OEBPS/ch3.xhtml#nested\""), "{}", ch2.html);
    assert!(ch2.html.contains("data-external=\"https://example.com/\""), "{}", ch2.html);
    // No live `href` attribute survives anywhere (note the leading space, so
    // `data-href="..."` does not count as a match).
    assert!(!ch2.html.contains(" href="), "{}", ch2.html);
    assert!(!ch2.html.contains(" src=\"http"), "{}", ch2.html);

    // Cover page image is inlined as a data URL.
    let ch1 = epub.chapter(0).unwrap();
    assert!(ch1.html.contains("data:image/png;base64,"), "cover image not inlined");

    // Search finds a phrase unique to one chapter and marks it in the excerpt.
    let hits = crate::epub::search_book(&mut epub, "end to end", 50);
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].chapter_index, 1);
    assert!(hits[0].excerpt.contains("[[end to end]]"), "{}", hits[0].excerpt);
    // A term present in several chapters is reported once per chapter, capped.
    let many = crate::epub::search_book(&mut epub, "bisect", 50);
    assert!(many.len() >= 4, "{}", many.len());
    // Single characters and misses return nothing.
    assert!(crate::epub::search_book(&mut epub, "z", 50).is_empty());
    assert!(crate::epub::search_book(&mut epub, "notinthisbook", 50).is_empty());
    assert!(crate::epub::search_book(&mut epub, "", 50).is_empty());
}

#[test]
fn rejects_non_epub_input() {
    let dir = std::env::temp_dir().join("opencode");
    let _ = std::fs::create_dir_all(&dir);
    let junk = dir.join("not-a-book.epub");
    std::fs::write(&junk, b"this is definitely not a zip file").unwrap();
    assert!(Epub::open(&junk).is_err());
}