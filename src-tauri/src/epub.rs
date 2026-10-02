use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::OnceLock;

use base64::Engine as _;
use regex::Regex;
use roxmltree::{Document, Node};

use crate::model::{limits, Chapter, ChapterContent, Manifest, SearchHit, TocNode};

pub type Result<T> = std::result::Result<T, EpubError>;

#[derive(Debug)]
pub enum EpubError {
    Io(String),
    Zip(String),
    Xml(String),
    Limit(String),
    NotFound(String),
}

impl std::fmt::Display for EpubError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EpubError::Io(m) => write!(f, "io error: {m}"),
            EpubError::Zip(m) => write!(f, "invalid epub archive: {m}"),
            EpubError::Xml(m) => write!(f, "malformed markup: {m}"),
            EpubError::Limit(m) => write!(f, "safety limit exceeded: {m}"),
            EpubError::NotFound(m) => write!(f, "not found: {m}"),
        }
    }
}
impl std::error::Error for EpubError {}

impl From<std::io::Error> for EpubError {
    fn from(e: std::io::Error) -> Self {
        EpubError::Io(e.to_string())
    }
}
impl From<zip::result::ZipError> for EpubError {
    fn from(e: zip::result::ZipError) -> Self {
        EpubError::Zip(e.to_string())
    }
}
impl From<roxmltree::Error> for EpubError {
    fn from(e: roxmltree::Error) -> Self {
        EpubError::Xml(e.to_string())
    }
}

// ---------------------------------------------------------------------------
// small helpers
// ---------------------------------------------------------------------------

fn mime_for(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "xhtml" | "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" => "text/javascript",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "ogg" => "audio/ogg",
        "ncx" => "application/x-dtbncx+xml",
        "opf" => "application/oebps-package+xml",
        _ => "application/octet-stream",
    }
}

/// Identify an image or font from its leading bytes, for the many books that
/// give assets a meaningless name and rely on the manifest to say what they are.
///
/// Only the formats a webview renders unconditionally are recognised. Anything
/// else stays unknown so the asset is skipped rather than inlined with a lying
/// content type.
fn sniff_mime(bytes: &[u8]) -> Option<&'static str> {
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";
    const GIF87: &[u8] = b"GIF87a";
    const GIF89: &[u8] = b"GIF89a";
    const RIFF: &[u8] = b"RIFF";
    const WEBP: &[u8] = b"WEBP";
    const TTFF: &[u8] = b"\x00\x01\x00\x00";
    const OTOTO: &[u8] = b"OTTO";
    if bytes.starts_with(PNG) {
        Some("image/png")
    } else if bytes.starts_with(GIF87) || bytes.starts_with(GIF89) {
        Some("image/gif")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(RIFF) && bytes.len() > 12 && &bytes[8..12] == WEBP {
        Some("image/webp")
    } else if bytes.starts_with(b"BM") {
        Some("image/bmp")
    } else if bytes.starts_with(OTOTO) {
        Some("font/otf")
    } else if bytes.starts_with(TTFF) {
        Some("font/ttf")
    } else if bytes.len() > 4 && (&bytes[..4] == b"wOFF" || &bytes[..4] == b"wOF2") {
        Some(if &bytes[..4] == b"wOFF" { "font/woff" } else { "font/woff2" })
    } else {
        None
    }
}

/// Dublin Core metadata may appear under several namespace URIs (EPUB 2
/// elements, EPUB 3 terms, or none at all), so match on a substring.
fn is_dublin_core(namespace: Option<&str>) -> bool {
    match namespace {
        None => true,
        Some(ns) => {
            let ns = ns.to_ascii_lowercase();
            ns.contains("purl.org/dc")
                || ns.contains("dublincore")
                || ns.contains("/dc/")
                || ns == "dc"
        }
    }
}

/// Normalise an EPUB href against the directory of the document that
/// references it. Returns `None` for anything that tries to escape the archive
/// (absolute paths, `..` traversal, remote URLs).
pub fn resolve_href(base_dir: &str, href: &str) -> Option<String> {
    let href = href.split('#').next().unwrap_or("");
    let href = percent_encoding::percent_decode_str(href).decode_utf8().ok()?;
    let href = href.trim();
    if href.is_empty() {
        return None;
    }
    let lower = href.to_ascii_lowercase();
    if lower.starts_with("http:")
        || lower.starts_with("https:")
        || lower.starts_with("data:")
        || lower.starts_with("file:")
        || lower.starts_with("javascript:")
        || lower.starts_with("//")
    {
        return None;
    }
    if href.contains('\\') || href.contains('\0') {
        return None;
    }

    let mut segments: Vec<&str> = Vec::new();
    // A leading slash means "container root", not "next to this document".
    if !href.starts_with('/') {
        for part in base_dir.split('/') {
            if !part.is_empty() && part != "." {
                segments.push(part);
            }
        }
    }
    for part in href.trim_start_matches('/').split('/') {
        match part {
            "" | "." => {}
            ".." => {
                // Refuse to climb above the archive root.
                if segments.pop().is_none() {
                    return None;
                }
            }
            other => segments.push(other),
        }
    }
    if segments.is_empty() {
        return None;
    }
    Some(segments.join("/"))
}

/// Directory part of a zip path ("a/b/c.xhtml" -> "a/b").
fn dir_of(path: &str) -> String {
    match path.rfind('/') {
        Some(i) => path[..i].to_string(),
        None => String::new(),
    }
}

fn percent_decode(s: &str) -> String {
    percent_encoding::percent_decode_str(s)
        .decode_utf8_lossy()
        .to_string()
}

/// Decode markup bytes. The BOM is authoritative, then an XML declaration,
/// then a real `<meta charset>`, then UTF-8.
///
/// Previously only a bare `charset` substring was searched for in the first
/// 4 KiB. A UTF-16LE EPUB 2 book decoded to mojibake, `Document::parse` failed,
/// and the whole book was rejected rather than one chapter.
fn decode_markup(bytes: &[u8]) -> String {
    // BOMs first: they are unambiguous and cost nothing to check.
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(&bytes[3..]).into_owned();
    }
    if bytes.starts_with(&[0xFF, 0xFE]) {
        return decode_with(bytes, encoding_rs::UTF_16LE);
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        return decode_with(bytes, encoding_rs::UTF_16BE);
    }
    if bytes.starts_with(&[0x00, 0x00, 0xFE, 0xFF])
        || bytes.starts_with(&[0xFF, 0xFE, 0x00, 0x00])
    {
        // UTF-32; not supported by encoding_rs, but lossy UTF-8 still beats
        // handing the raw bytes to the XML parser.
        return String::from_utf8_lossy(bytes).into_owned();
    }

    const PROBE: usize = 4096;
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(PROBE)]).to_string();

    // 1. An XML declaration's encoding="...", which EPUB 2 relies on heavily.
    if let Some(label) = capture_quoted(head.to_ascii_lowercase().as_str(), "encoding") {
        if let Some(enc) = encoding_for(&label) {
            return decode_with(bytes, enc);
        }
    }

    // 2. A <meta charset> or a meta http-equiv content-type, but only when it
    //    appears inside an actual <meta ...> tag, so the word "charset" in body
    //    text or a comment cannot select a bogus encoding.
    let lower = head.to_ascii_lowercase();
    if let Some(meta) = lower.find("<meta") {
        let chunk_end = lower[meta..].find('>').map(|i| meta + i).unwrap_or(lower.len());
        let tag = &lower[meta..chunk_end];
        if let Some(label) = capture_quoted(tag, "charset") {
            if let Some(enc) = encoding_for(&label) {
                return decode_with(bytes, enc);
            }
        }
        if tag.contains("http-equiv") {
            if let Some(label) = capture_quoted(tag, "content") {
                if let Some(pos) = label.to_ascii_lowercase().find("charset=") {
                    let label = &label[pos + 8..];
                    let label: String = label
                        .chars()
                        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                        .collect();
                    if let Some(enc) = encoding_for(&label) {
                        return decode_with(bytes, enc);
                    }
                }
            }
        }
    }

    String::from_utf8_lossy(bytes).into_owned()
}

fn decode_with(bytes: &[u8], enc: &'static encoding_rs::Encoding) -> String {
    let (text, _, _) = enc.decode(bytes);
    text.into_owned()
}

fn encoding_for(label: &str) -> Option<&'static encoding_rs::Encoding> {
    let label = label.trim().trim_matches(['"', '\'']);
    if label.is_empty() {
        return None;
    }
    encoding_rs::Encoding::for_label(label.as_bytes())
}

/// Pull the value out of `name="value"` or `name='value'` inside `haystack`.
/// Byte length of `c` after Unicode lower-casing. This is not always equal to
/// `c.len_utf8()`: `U+0130` is 2 bytes and folds to 3.
fn folded_len(c: char) -> usize {
    c.to_lowercase().map(|lc| lc.len_utf8()).sum()
}

fn capture_quoted(haystack: &str, name: &str) -> Option<String> {
    let mut from = 0usize;
    while let Some(rel) = haystack[from..].find(name) {
        let start = from + rel + name.len();
        let rest = &haystack[start..];
        // The `=` is optional: `encoding="..."` has one, `<meta charset>` does
        // not, and the previous version only accepted the no-`=` form - so
        // Latin-1 books silently fell through to UTF-8.
        let trimmed = rest.trim_start().trim_start_matches('=').trim_start();
        if let Some(q @ ('"' | '\'')) = trimmed.chars().next() {
            let value: String = trimmed[1..].chars().take_while(|c| *c != q).collect();
            return Some(value);
        }
        from = start;
    }
    None
}

// ---------------------------------------------------------------------------
// CSS scrubbing
// ---------------------------------------------------------------------------

fn re_at_import() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?is)@import[^;{}]*;?").unwrap())
}
fn re_css_comment() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?s)/\*(.*?)\*/").unwrap())
}
/// Neutralise any attempt to terminate the `<style>` element we are about to
/// emit. CSS comments are collapsed first so a sequence split across a comment
/// cannot slip through, then every `</` becomes `<\\/`. That escape is invalid
/// inside CSS, so it renders as literal text instead of closing the element.
fn guard_style_block(css: &str) -> String {
    let squashed = re_css_comment().replace_all(css, " ");
    let mut out = String::with_capacity(squashed.len() + 16);
    let mut prev_slash = false;
    for ch in squashed.chars() {
        if ch == '<' {
            out.push('<');
            out.push('\\');
            prev_slash = false;
            continue;
        }
        if prev_slash {
            out.push(ch);
            prev_slash = false;
            continue;
        }
        prev_slash = ch == '\\';
        out.push(ch);
    }
    out
}
fn re_url_dq() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?is)url\(\s*"([^"]*)"\s*\)"#).unwrap())
}
fn re_url_sq() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?is)url\(\s*'([^']*)'\s*\)").unwrap())
}
fn re_url_bare() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?is)url\(\s*([^)\s][^)]*?)\s*\)").unwrap())
}
fn re_dangerous_css() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?is)(expression\s*\(|-moz-binding|behavior\s*:|@charset|javascript\s*:|vbscript\s*:|-webkit-image-set|-moz-image-set|image-set\s*\()",
        )
        .unwrap()
    })
}

/// Neutralise every construct in a stylesheet that could execute code or
/// reach the network. Only `data:` images survive.
///
/// CSS escapes are decoded by the engine before a token is interpreted, so
/// `\75 rl(https://…)` *is* a `url()` to the browser while matching none of the
/// literal patterns. The stylesheet is therefore unescaped first, so `url`
/// always appears spelled out before the patterns run.
pub fn scrub_css(css: &str) -> String {
    let mut out = decode_css_escapes(css);
    out = re_css_comment().replace_all(&out, "").into_owned();
    out = re_at_import().replace_all(&out, "").into_owned();
    out = re_dangerous_css().replace_all(&out, "").into_owned();
    // The `regex` crate has no backreferences, so quoted and bare forms are
    // handled by three separate passes.
    let rewrite = |text: &mut String, re: &Regex| {
        *text = re
            .replace_all(text, |caps: &regex::Captures<'_>| {
                let target = caps.get(1).map(|m| m.as_str().trim()).unwrap_or("");
                if target.to_ascii_lowercase().starts_with("data:image/") {
                    format!("url({target})")
                } else {
                    "none".to_string()
                }
            })
            .into_owned();
    };
    rewrite(&mut out, re_url_dq());
    rewrite(&mut out, re_url_sq());
    rewrite(&mut out, re_url_bare());
    out
}

/// Replace CSS escape sequences (`\` plus up to six hex digits and an optional
/// terminating whitespace) with the character they denote. A backslash followed
/// by whitespace is a line continuation and is dropped. Anything else keeps its
/// backslash so ordinary CSS is not corrupted.
fn decode_css_escapes(css: &str) -> String {
    let chars: Vec<char> = css.chars().collect();
    let mut out = String::with_capacity(css.len());
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] != '\\' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        i += 1;
        if i < chars.len() && chars[i].is_whitespace() {
            i += 1;
            continue;
        }
        let mut hex = String::new();
        while i < chars.len() && hex.len() < 6 && chars[i].is_ascii_hexdigit() {
            hex.push(chars[i]);
            i += 1;
        }
        match u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
            Some(decoded) => {
                out.push(decoded);
                if i < chars.len() && chars[i].is_whitespace() {
                    i += 1;
                }
            }
            None => out.push('\\'),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// HTML tag rewriting
// ---------------------------------------------------------------------------

fn re_tag() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?is)<([a-z][a-z0-9:_-]*)((?:[^>"']|"[^"]*"|'[^']*')*)>"#).unwrap())
}
fn re_style_block() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?is)<style\b[^>]*>(.*?)</\s*style\s*>").unwrap())
}
fn re_attr() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r#"(?is)\b([a-zA-Z_:][-a-zA-Z0-9_:.]*)\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'`=<>]+))"#,
        )
        .unwrap()
    })
}

struct Attrs {
    pairs: Vec<(String, Option<String>)>,
    self_closing: bool,
}

fn parse_attrs(src: &str) -> Attrs {
    let mut pairs = Vec::new();
    let self_closing = src.trim_end().ends_with('/');
    for caps in re_attr().captures_iter(src) {
        let name = caps.get(1).unwrap().as_str().to_ascii_lowercase();
        let value = caps
            .get(2)
            .or_else(|| caps.get(3))
            .or_else(|| caps.get(4))
            .map(|m| m.as_str().to_string());
        pairs.push((name, value));
    }
    Attrs { pairs, self_closing }
}

fn render_tag(name: &str, attrs: &Attrs) -> String {
    let mut out = String::with_capacity(64);
    out.push('<');
    out.push_str(name);
    for (key, value) in &attrs.pairs {
        out.push(' ');
        out.push_str(key);
        if let Some(v) = value {
            out.push_str("=\"");
            for ch in v.chars() {
                match ch {
                    '&' => out.push_str("&amp;"),
                    '"' => out.push_str("&quot;"),
                    '<' => out.push_str("&lt;"),
                    '>' => out.push_str("&gt;"),
                    _ => out.push(ch),
                }
            }
            out.push('"');
        }
    }
    if attrs.self_closing {
        out.push_str(" /");
    }
    out.push('>');
    out
}

/// Walk every tag in `html` and let `handler` rewrite it. Returns the new
/// document. This is intentionally a *preserving* rewrite: it never invents
/// attributes and never reorders content, so we do not corrupt well-formed
/// markup before sanitization.
fn rewrite_tags<F>(html: &str, mut handler: F) -> String
where
    F: FnMut(&str, &mut Attrs) -> Option<String>,
{
    let mut out = String::with_capacity(html.len() + 1024);
    let mut last = 0usize;
    for caps in re_tag().captures_iter(html) {
        let whole = caps.get(0).unwrap();
        out.push_str(&html[last..whole.start()]);
        last = whole.end();

        let name = caps.get(1).unwrap().as_str().to_ascii_lowercase();
        let body = caps.get(2).map(|m| m.as_str()).unwrap_or("");
        let mut attrs = parse_attrs(body);
        match handler(&name, &mut attrs) {
            Some(forced) => out.push_str(&forced),
            None => out.push_str(&render_tag(&name, &attrs)),
        }
    }
    out.push_str(&html[last..]);
    out
}

fn attr_get<'a>(attrs: &'a Attrs, key: &str) -> Option<&'a str> {
    attrs
        .pairs
        .iter()
        .find(|(k, _)| k == key)
        .and_then(|(_, v)| v.as_deref())
}

fn attr_set(attrs: &mut Attrs, key: &str, value: &str) {
    if let Some(slot) = attrs.pairs.iter_mut().find(|(k, _)| k == key) {
        slot.1 = Some(value.to_string());
    } else {
        attrs.pairs.push((key.to_string(), Some(value.to_string())));
    }
}

fn attr_remove(attrs: &mut Attrs, key: &str) {
    attrs.pairs.retain(|(k, _)| k != key);
}

// ---------------------------------------------------------------------------
// The archive
// ---------------------------------------------------------------------------

pub struct Epub {
    archive: zip::ZipArchive<File>,
    names: HashMap<String, usize>,
    manifest: Manifest,
    /// Path of the OPF, discovered cheaply from container.xml.
    opf_path: String,
    /// Set once the OPF/NCX/nav have been parsed.
    manifest_ready: bool,
}

impl Epub {
    /// Open the archive and index its entries. Parsing the package document is
    /// deferred to `ensure_manifest`, so a cache hit costs nothing.
    pub fn open(path: &Path) -> Result<Self> {
        let file =
            File::open(path).map_err(|e| EpubError::Io(format!("{}: {e}", path.display())))?;
        let file_len = file.metadata()?.len();
        if file_len == 0 {
            return Err(EpubError::Zip("file is empty".into()));
        }
        if file_len > limits::MAX_ARCHIVE_BYTES {
            return Err(EpubError::Limit("file larger than 512 MiB".into()));
        }
        let mut archive = zip::ZipArchive::new(file)?;
        if archive.len() > limits::MAX_ENTRIES {
            return Err(EpubError::Limit(format!(
                "archive has {} entries (max {})",
                archive.len(),
                limits::MAX_ENTRIES
            )));
        }

        let mut names = HashMap::with_capacity(archive.len());
        for i in 0..archive.len() {
            let entry = archive.by_index(i)?;
            if entry.is_dir() {
                continue;
            }
            let raw = entry.name().to_string();
            // Defensive: never index entries with traversal or absolute names.
            if raw.starts_with('/') || raw.contains("..") || raw.contains('\\') || raw.contains('\0') {
                continue;
            }
            names.entry(raw).or_insert(i);
        }
        if !names.contains_key("mimetype") && !names.contains_key("META-INF/container.xml") {
            return Err(EpubError::Zip("not an EPUB (no mimetype or container.xml)".into()));
        }

        let opf_path = Self::find_opf(&mut archive, &names)?;

        Ok(Self {
            archive,
            names,
            manifest: Manifest {
                opf_path: opf_path.clone(),
                ..Default::default()
            },
            opf_path,
            manifest_ready: false,
        })
    }

    /// Parse the package document if that has not happened yet. Every entry
    /// point that needs the spine or the navigation calls this first.
    pub fn ensure_manifest(&mut self) -> Result<()> {
        if !self.manifest_ready {
            let manifest = Self::parse_manifest(&mut self.archive, &self.names, &self.opf_path)?;
            if manifest.chapters.is_empty() {
                return Err(EpubError::Zip("spine contains no readable documents".into()));
            }
            self.manifest = manifest;
            self.manifest_ready = true;
        }
        Ok(())
    }

    /// The package document. Only meaningful after `ensure_manifest`.
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// Install a manifest that was parsed earlier, so `ensure_manifest` becomes
    /// a no-op. This is what makes a warm cache actually cheap.
    pub fn adopt_manifest(&mut self, manifest: Manifest) {
        self.manifest = manifest;
        self.manifest_ready = true;
    }

    /// Read the manifest, opening the file if necessary. The common entry point
    /// for a one-shot command.
    pub fn open_with_manifest(path: &Path) -> Result<Self> {
        let mut epub = Self::open(path)?;
        epub.ensure_manifest()?;
        Ok(epub)
    }

    // -- raw reads --------------------------------------------------------

    fn find_opf(archive: &mut zip::ZipArchive<File>, names: &HashMap<String, usize>) -> Result<String> {
        let path = names
            .get("META-INF/container.xml")
            .copied()
            .ok_or_else(|| EpubError::NotFound("META-INF/container.xml".into()))?;
        let bytes = Self::read_index(archive, path, limits::MAX_XML_BYTES)?;
        let text = decode_markup(&bytes);
        let doc = Document::parse(&text).map_err(|e| EpubError::Xml(format!("container.xml: {e}")))?;

        // EPUB 3.0.1 allows several <rootfile> elements and says consumers
        // SHOULD read all of them; the media-type attribute picks the
        // authoritative one. Previously only the first was considered, so an
        // ebook that listed a legacy NCX rootfile second could resolve to the
        // wrong package document, or to none at all.
        let mut fallback: Option<String> = None;
        for node in doc.descendants() {
            if node.tag_name().name() != "rootfile" {
                continue;
            }
            let Some(full) = node.attribute("full-path") else {
                continue;
            };
            let Some(resolved) = resolve_href("", full) else {
                continue;
            };
            // Skip anything that is not actually present in the archive.
            if !names.contains_key(&resolved) {
                continue;
            }
            let media = node.attribute("media-type").unwrap_or("").to_ascii_lowercase();
            if media == "application/oebps-package+xml" {
                return Ok(resolved);
            }
            if media.is_empty() {
                fallback.get_or_insert(resolved);
            } else if fallback.is_none() {
                fallback = Some(resolved);
            }
        }
        fallback.ok_or_else(|| EpubError::NotFound("rootfile in container.xml".into()))
    }

    /// The next chapter to read, starting the search at `from`.
    ///
    /// A spine item marked `linear="no"` is a footnote, a figure or a
    /// standalone page. It stays in the reading order so the table of contents
    /// can still reach it, but stepping forward through the book must skip over
    /// it, which is what the attribute exists to express.
    pub fn next_linear(&self, from: usize) -> Option<usize> {
        self.manifest
            .chapters
            .iter()
            .enumerate()
            .skip(from)
            .find(|(_, chapter)| chapter.linear)
            .map(|(i, _)| i)
    }

    /// The previous linear chapter before `before`.
    pub fn prev_linear(&self, before: usize) -> Option<usize> {
        self.manifest
            .chapters
            .iter()
            .enumerate()
            .take(before)
            .filter(|(_, chapter)| chapter.linear)
            .next_back()
            .map(|(i, _)| i)
    }

    fn read_index(
        archive: &mut zip::ZipArchive<File>,
        index: usize,
        max_bytes: u64,
    ) -> Result<Vec<u8>> {
        let mut entry = archive.by_index(index)?;
        let size = entry.size();
        if size > max_bytes {
            return Err(EpubError::Limit(format!(
                "entry is {} bytes (max {max_bytes})",
                size
            )));
        }
        // Zip-bomb guard: absurd ratios on small entries are never legitimate.
        if size > 0 && size < 4 * 1024 * 1024 {
            let compressed = entry.compressed_size().max(1);
            if size / compressed > limits::MAX_COMPRESSION_RATIO * 5 {
                return Err(EpubError::Limit(format!(
                    "suspicious compression ratio on {size} bytes"
                )));
            }
        }
        let mut buf = Vec::with_capacity(size as usize);
        entry.by_ref().take(max_bytes).read_to_end(&mut buf)?;
        Ok(buf)
    }

    /// Read a zip-relative path, honouring all safety limits.
    pub fn read(&mut self, path: &str, max_bytes: u64) -> Result<Vec<u8>> {
        let index = *self
            .names
            .get(path)
            .ok_or_else(|| EpubError::NotFound(path.to_string()))?;
        Self::read_index(&mut self.archive, index, max_bytes)
    }

    pub fn exists(&self, path: &str) -> bool {
        self.names.contains_key(path)
    }

    // -- manifest / OPF ---------------------------------------------------

    fn parse_manifest(
        archive: &mut zip::ZipArchive<File>,
        names: &HashMap<String, usize>,
        opf_path: &str,
    ) -> Result<Manifest> {
        let index = *names
            .get(opf_path)
            .ok_or_else(|| EpubError::NotFound(opf_path.to_string()))?;
        let bytes = Self::read_index(archive, index, limits::MAX_XML_BYTES)?;
        let text = decode_markup(&bytes);
        let doc = Document::parse(&text).map_err(|e| EpubError::Xml(format!("{opf_path}: {e}")))?;

        let base = dir_of(opf_path);
        let mut manifest = Manifest {
            opf_path: opf_path.to_string(),
            ..Default::default()
        };

        // ---- metadata (dc:*), matched by local name so odd prefixes work.
        let mut cover_meta_id: Option<String> = None;
        for node in doc.descendants() {
            match node.tag_name().name() {
                "title" if is_dublin_core(node.tag_name().namespace()) => {
                    if manifest.title.is_empty() {
                        manifest.title = collect_text(&node).trim().to_string();
                    }
                }
                "creator" if is_dublin_core(node.tag_name().namespace()) => {
                    let value = collect_text(&node).trim().to_string();
                    if !value.is_empty() {
                        if manifest.author.is_empty() {
                            manifest.author = value.clone();
                        } else {
                            manifest.author.push_str(", ");
                            manifest.author.push_str(&value);
                        }
                    }
                }
                "language" if is_dublin_core(node.tag_name().namespace()) => {
                    manifest.language = collect_text(&node).trim().to_string();
                }
                "publisher" if is_dublin_core(node.tag_name().namespace()) => {
                    manifest.publisher = collect_text(&node).trim().to_string();
                }
                "description" if is_dublin_core(node.tag_name().namespace()) => {
                    manifest.description = collapse(&collect_text(&node));
                }
                "identifier" if is_dublin_core(node.tag_name().namespace()) => {
                    manifest.identifier = collect_text(&node).trim().to_string();
                }
                "rights" if is_dublin_core(node.tag_name().namespace()) => {
                    manifest.rights = collect_text(&node).trim().to_string();
                }
                "meta" => {
                    let name = node.attribute("name").unwrap_or("");
                    if name.eq_ignore_ascii_case("cover") {
                        if let Some(content) = node.attribute("content") {
                            cover_meta_id = Some(content.to_string());
                        }
                    }
                }
                _ => {}
            }
        }

        // ---- manifest items
struct Item {
            href: String,
            media_type: String,
        }
        let mut items: HashMap<String, Item> = HashMap::new();
        let mut nav_href: Option<String> = None;
        let mut ncx_href: Option<String> = None;
        let mut cover_item: Option<String> = None;

        for node in doc.descendants() {
            if node.tag_name().name() != "item" {
                continue;
            }
            let id = node.attribute("id").unwrap_or("").to_string();
            let href_raw = node.attribute("href").unwrap_or("");
            let Some(href) = resolve_href(&base, href_raw) else {
                continue;
            };
            let media_type = node
                .attribute("media-type")
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            let properties: Vec<String> = node
                .attribute("properties")
                .unwrap_or("")
                .split_whitespace()
                .map(|p| p.to_ascii_lowercase())
                .collect();

            if properties.iter().any(|p| p == "cover-image") {
                cover_item = Some(href.clone());
            }
            if properties.iter().any(|p| p == "nav") {
                nav_href = Some(href.clone());
            }
            if media_type == "application/x-dtbncx+xml" {
                ncx_href = Some(href.clone());
            }
            if !id.is_empty() {
                items.insert(id, Item { href, media_type });
            }
        }

        // ---- spine
for node in doc.descendants() {
            if node.tag_name().name() != "itemref" {
                continue;
            }
            let Some(idref) = node.attribute("idref") else {
                continue;
            };
            let Some(item) = items.get(idref) else {
                continue;
            };
            if !item.media_type.contains("html") && !item.href.ends_with(".xhtml") {
                continue;
            }
            if manifest.chapters.len() >= limits::MAX_SPINE_ITEMS {
                break;
            }
            // A spine entry that points at nothing, or at something we cannot
            // read within the limits, used to become a chapter that threw the
            // moment the reader paged into it.
            let Some(index) = names.get(&item.href).copied() else {
                continue;
            };
            let Ok(entry) = archive.by_index(index) else {
                continue;
            };
            let size = entry.size();
            if size > limits::MAX_ENTRY_BYTES {
                continue;
            }
            let linear = node
                .attribute("linear")
                .map(|v| !v.eq_ignore_ascii_case("no"))
                .unwrap_or(true);
            manifest.chapters.push(Chapter {
                href: item.href.clone(),
                title: String::new(),
                index: manifest.chapters.len(),
                // Read from the central directory. Inflating the document just
                // to call .len() meant every chapter of the book was
                // decompressed on every single command, including one page turn.
                bytes: size,
                linear,
            });
        }

        // ---- cover resolution order: cover-image, meta[name=cover], guess
        manifest.cover_href = cover_item
            .or_else(|| {
                cover_meta_id
                    .as_ref()
                    .and_then(|id| items.get(id))
                    .map(|i| i.href.clone())
            })
            .or_else(|| {
                items
                    .values()
                    .find(|i| {
                        i.media_type.starts_with("image/")
                            && i.href.to_ascii_lowercase().contains("cover")
                    })
                    .map(|i| i.href.clone())
            })
            .filter(|href| mime_for(href).starts_with("image/"));

        // ---- navigation
        if let Some(nav) = nav_href.clone() {
            if names.contains_key(&nav) {
                if let Ok(bytes) = Self::read_index(archive, names[&nav], limits::MAX_XML_BYTES) {
                    let text = decode_markup(&bytes);
                    let dir = dir_of(&nav);
                    parse_nav_document(&text, &dir, &mut manifest);
                }
            }
        }
        if manifest.toc.is_empty() {
            if let Some(ncx) = ncx_href.or_else(|| {
                items
                    .values()
                    .find(|i| i.href.to_ascii_lowercase().ends_with(".ncx"))
                    .map(|i| i.href.clone())
            }) {
                if names.contains_key(&ncx) {
                    if let Ok(bytes) = Self::read_index(archive, names[&ncx], limits::MAX_XML_BYTES) {
                        let text = decode_markup(&bytes);
                        let dir = dir_of(&ncx);
                        manifest.toc = parse_ncx(&text, &dir);
                    }
                }
            }
        }
        if manifest.toc.is_empty() {
            // Last resort: one entry per spine item.
            manifest.toc = manifest
                .chapters
                .iter()
                .enumerate()
                .map(|(i, c)| TocNode {
                    title: format!("Chapter {}", i + 1),
                    href: c.href.clone(),
                    depth: 0,
                    children: Vec::new(),
                })
                .collect();
        }

        Ok(manifest)
    }

    // -- assets -----------------------------------------------------------

    fn read_asset_as_data_url(&mut self, base: &str, href: &str, budget: &mut AssetBudget) -> Result<String> {
        let Some(path) = resolve_href(base, href) else {
            return Err(EpubError::NotFound(href.to_string()));
        };
        if !self.names.contains_key(&path) {
            return Err(EpubError::NotFound(path));
        }
        // The extension is only a hint. Plenty of real books ship artwork as
        // `img001` or `cover.dat` and declare the true type only in the OPF
        // manifest, so a name we cannot map must not be discarded outright -
        // sniff the bytes instead.
        let declared = mime_for(&path);
        let bytes = self.read(&path, budget.remaining())?;
        budget.consume(bytes.len() as u64);
        let mime = if declared == "application/octet-stream" {
            sniff_mime(&bytes)
                .ok_or_else(|| EpubError::NotFound(path.clone()))?
        } else {
            declared
        };
        if !(mime.starts_with("image/") || mime.starts_with("font/")) {
            return Err(EpubError::NotFound(path));
        }
        let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
        Ok(format!("data:{mime};base64,{encoded}"))
    }

    // -- chapters ---------------------------------------------------------

    /// Build a fully self-contained, sanitized HTML document for `index`.
pub fn chapter(&mut self, index: usize) -> Result<ChapterContent> {
        self.ensure_manifest()?;
        let chapter = self
            .manifest
            .chapters
            .get(index)
            .cloned()
            .ok_or_else(|| EpubError::NotFound(format!("chapter {index}")))?;
        let (title, html) = self.render_document(&chapter.href)?;

        Ok(ChapterContent {
            href: chapter.href.clone(),
            title: if title.is_empty() { chapter.title.clone() } else { title },
            index,
            html,
            is_last: self.next_linear(index + 1).is_none(),
            has_prev: self.prev_linear(index).is_some(),
            has_next: self.next_linear(index + 1).is_some(),
        })
    }

    fn render_document(&mut self, href: &str) -> Result<(String, String)> {
        let bytes = self.read(href, limits::MAX_ENTRY_BYTES)?;
        let source = decode_markup(&bytes);
        let base = dir_of(href);

// 1. Harvest inline <style> bodies and drop the elements. This must happen
        //    *before* the tag rewriting pass, otherwise the CSS text would be
        //    left behind as visible document text.
        let mut inline_styles: Vec<String> = Vec::new();
        let mut body = String::with_capacity(source.len());
        {
            let mut cursor = 0usize;
            for caps in re_style_block().captures_iter(&source) {
                let whole = caps.get(0).unwrap();
                inline_styles.push(caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string());
                body.push_str(&source[cursor..whole.start()]);
                cursor = whole.end();
            }
            body.push_str(&source[cursor..]);
        }

        // 2. Collect stylesheet links and drop link/meta/base elements.
        let mut stylesheet_links: Vec<String> = Vec::new();
        let body = rewrite_tags(&body, |name, attrs| {
            match name {
                "link" => {
                    let rel = attr_get(attrs, "rel").unwrap_or("").to_ascii_lowercase();
                    if rel.contains("stylesheet") {
                        if let Some(h) = attr_get(attrs, "href") {
                            stylesheet_links.push(h.to_string());
                        }
                    }
                    Some(String::new())
                }
                "style" | "base" | "meta" => Some(String::new()),
                _ => None,
            }
        });

        // 3. Inline every referenced stylesheet (scoped is unnecessary: the
        //    CSS is fully scrubbed and only ever loaded into our own frame).
        let mut css = String::new();
        for link in stylesheet_links.iter().take(32) {
            if let Some(path) = resolve_href(&base, link) {
                if self.exists(&path) {
                    if let Ok(css_bytes) = self.read(&path, 2 * 1024 * 1024) {
                        css.push_str(&scrub_css(&decode_markup(&css_bytes)));
                        css.push('\n');
                    }
                }
            }
        }
for block in inline_styles.iter().take(64) {
            css.push_str(&scrub_css(block));
            css.push('\n');
        }
        css.truncate(1_500_000);
        // The stylesheet is interpolated into the returned HTML *after* the
        // ammonia gate, so it must be incapable of closing its own <style>
        // element. `scrub_css` only understands CSS grammar and lets `</style`
        // through, which would otherwise let a book's .css file inject markup
        // into the privileged webview.
        css = guard_style_block(&css);

        // 4. Inline images / fonts as data URLs, rewrite links, scrub styles.
        let mut budget = AssetBudget::new(limits::MAX_INLINE_ASSET_BYTES);
        let body = rewrite_tags(&body, |name, attrs| {
            match name {
                "img" => {
                    let src = attr_get(attrs, "src").unwrap_or("").to_string();
                    attr_remove(attrs, "src");
                    if !src.is_empty() && budget.can_add() {
                        if let Ok(data) = self.read_asset_as_data_url(&base, &src, &mut budget) {
                            attr_set(attrs, "src", &data);
                        }
                    }
                    None
                }
                "image" => {
                    let raw = attr_get(attrs, "xlink:href")
                        .or_else(|| attr_get(attrs, "href"))
                        .map(|s| s.to_string());
                    attr_remove(attrs, "href");
                    attr_remove(attrs, "xlink:href");
                    if let Some(src) = raw {
                        if budget.can_add() {
                            if let Ok(data) = self.read_asset_as_data_url(&base, &src, &mut budget) {
                                attr_set(attrs, "xlink:href", &data);
                            }
                        }
                    }
                    None
                }
                "a" => {
                    let href = attr_get(attrs, "href").unwrap_or("").to_string();
                    // An SVG anchor can carry xlink:href instead, and that
                    // attribute is on the generic allow-list. Leaving it would
                    // let xlink:href="data:text/html,..." survive the final
                    // gate, since data is an allowed URL scheme.
                    let svg_href = attr_get(attrs, "xlink:href").unwrap_or("").to_string();
                    attr_remove(attrs, "href");
                    attr_remove(attrs, "xlink:href");
                    attr_remove(attrs, "target");
                    let source = if href.is_empty() { svg_href } else { href };
                    if !source.is_empty() {
                        let decoded = percent_decode(&source);
                        let external = decoded.to_ascii_lowercase().starts_with("http")
                            || decoded.to_ascii_lowercase().starts_with("mailto:");
                        if external {
                            attr_set(attrs, "data-external", &decoded);
                        } else {
                            let resolved = resolve_href(&base, &decoded);
                            let fragment = decoded.split('#').nth(1).map(|f| f.to_string());
                            match (resolved, fragment) {
                                (Some(path), Some(frag)) => {
                                    attr_set(attrs, "data-href", &format!("{path}#{frag}"));
                                    attr_set(attrs, "data-kind", "fragment");
                                }
                                (Some(path), None) => {
                                    attr_set(attrs, "data-href", &path);
                                    attr_set(attrs, "data-kind", "document");
                                }
                                _ => {}
                            }
                        }
                    }
                    None
                }
                "embed" | "object" | "iframe" | "audio" | "video" | "source" | "track"
                | "form" | "input" | "button" | "select" | "textarea" | "noscript" => {
                    Some(String::new())
                }
                _ => {
                    if let Some(style) = attr_get(attrs, "style") {
                        attr_set(attrs, "style", &scrub_css(style));
                    }
                    None
                }
            }
        });

        // 5. Final gate: ammonia allow-list. Anything we missed cannot survive.
        let clean = sanitize_html(&body);

        let title = extract_title(&clean).unwrap_or_default();
        let html = format!(
            "<style data-epub=\"css\">\n{css}\n</style>\n<div class=\"epub-doc\" epub-href=\"{}\">{}</div>",
            escape_attr(href),
            clean
        );
        Ok((title, html))
    }

    /// Raw (unsanitized) plain text of every spine document, used for search.
pub fn chapter_text(&mut self, index: usize) -> Result<(String, String)> {
        self.ensure_manifest()?;
        let chapter = self
            .manifest
            .chapters
            .get(index)
            .cloned()
            .ok_or_else(|| EpubError::NotFound(format!("chapter {index}")))?;
        let bytes = self.read(&chapter.href, limits::MAX_ENTRY_BYTES)?;
        let source = decode_markup(&bytes);
        Ok((chapter.href.clone(), html_to_text(&source)))
    }

pub fn cover_data_url(&mut self) -> Result<Option<String>> {
        self.ensure_manifest()?;
        let Some(href) = self.manifest.cover_href.clone() else {
            return Ok(None);
        };
        let mut budget = AssetBudget::new(limits::MAX_INLINE_ASSET_BYTES);
        Ok(self.read_asset_as_data_url("", &href, &mut budget).ok())
    }
}

struct AssetBudget {
    remaining: u64,
    count: usize,
}

impl AssetBudget {
    fn new(total: u64) -> Self {
        Self { remaining: total, count: 0 }
    }
    fn remaining(&self) -> u64 {
        self.remaining.max(1)
    }
    fn can_add(&self) -> bool {
        self.count < limits::MAX_INLINE_ASSETS && self.remaining > 0
    }
    fn consume(&mut self, n: u64) {
        self.remaining = self.remaining.saturating_sub(n);
        self.count += 1;
    }
}

// ---------------------------------------------------------------------------
// navigation parsing
// ---------------------------------------------------------------------------

/// roxmltree's `attribute()` matches the *local* name, so `epub:type` is
/// reached as `"type"`. Both the EPUB 3 vocabulary and the ARIA roles are
/// consulted, since real books use either.
fn node_has_prop(node: &Node<'_, '_>, want: &str) -> bool {
    for name in ["type", "role", "property"] {
        if let Some(value) = node.attribute(name) {
            if value
                .split_whitespace()
                .any(|token| token.eq_ignore_ascii_case(want))
            {
                return true;
            }
        }
    }
    false
}

fn parse_nav_document(text: &str, base: &str, manifest: &mut Manifest) {
    let Ok(doc) = Document::parse(text) else {
        // Fall back to regex scraping for nav documents that are not well-formed.
        manifest.toc = scrape_nav_regex(text, base);
        return;
    };
    let mut toc = Vec::new();
    let mut pages = Vec::new();
    let mut landmarks = Vec::new();
    for nav in doc.descendants().filter(|n| n.tag_name().name() == "nav") {
        let mut list: Vec<TocNode> = Vec::new();
        let mut depth = 0usize;
        collect_nav_list(nav.children(), base, &mut list, &mut depth);
        if list.is_empty() {
            continue;
        }
        if node_has_prop(&nav, "toc") || node_has_prop(&nav, "doc-toc") {
            toc.extend(list);
        } else if node_has_prop(&nav, "page-list") || node_has_prop(&nav, "doc-pagelist") {
            pages.extend(list);
        } else if node_has_prop(&nav, "landmarks") || node_has_prop(&nav, "doc-landmarks") {
            landmarks.extend(list);
        }
    }
    if !toc.is_empty() {
        manifest.toc = toc;
    }
    manifest.page_list = pages;
    manifest.landmarks = landmarks;
}

fn collect_nav_list<'a, I>(nodes: I, base: &str, out: &mut Vec<TocNode>, depth: &mut usize)
where
    I: Iterator<Item = Node<'a, 'a>>,
{
    for node in nodes {
        match node.tag_name().name() {
            "li" => {
                if *depth >= limits::MAX_TOC_DEPTH {
                    return;
                }
                let mut title = String::new();
                let mut href = None;
                let mut children: Vec<TocNode> = Vec::new();
                for child in node.children() {
                    match child.tag_name().name() {
                        "a" | "span" => {
                            if title.is_empty() {
                                title = collect_text(&child);
                            }
                            if href.is_none() {
                                if let Some(h) = child.attribute("href") {
                                    href = Some(h.to_string());
                                }
                            }
                        }
                        "ol" | "ul" => {
                            *depth += 1;
                            collect_nav_list(child.children(), base, &mut children, depth);
                            *depth -= 1;
                        }
                        _ => {}
                    }
                }
                if let Some(h) = href {
                    if let Some(resolved) = resolve_href(base, &h) {
                        out.push(TocNode {
                            title: if title.is_empty() { "Untitled".into() } else { title },
                            href: resolved,
                            depth: *depth,
                            children,
                        });
                    }
                }
            }
            "ol" | "ul" => collect_nav_list(node.children(), base, out, depth),
            _ => {}
        }
    }
}

fn scrape_nav_regex(text: &str, base: &str) -> Vec<TocNode> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r#"(?is)<a\s[^>]*href\s*=\s*["']([^"']+)["'][^>]*>(.*?)</a>"#).unwrap()
    });
    let mut out = Vec::new();
    for caps in re.captures_iter(text) {
        let href = &caps[1];
        let title = strip_tags(&caps[2]);
        if let Some(resolved) = resolve_href(base, href) {
            out.push(TocNode {
                title: if title.is_empty() { "Untitled".into() } else { title },
                href: resolved,
                depth: 0,
                children: Vec::new(),
            });
        }
    }
    out
}

fn parse_ncx(text: &str, base: &str) -> Vec<TocNode> {
    let Ok(doc) = Document::parse(text) else {
        return scrape_nav_regex(text, base);
    };
    let mut out = Vec::new();
    for nav_map in doc.descendants().filter(|n| n.tag_name().name() == "navMap") {
        let mut depth = 0usize;
        collect_ncx(nav_map.children(), base, &mut out, &mut depth);
    }
    out
}

fn collect_ncx<'a, I>(nodes: I, base: &str, out: &mut Vec<TocNode>, depth: &mut usize)
where
    I: Iterator<Item = Node<'a, 'a>>,
{
    for node in nodes {
        if node.tag_name().name() != "navPoint" {
            continue;
        }
        if *depth >= limits::MAX_TOC_DEPTH {
            break;
        }
        let mut title = String::new();
        let mut href: Option<String> = None;
        for child in node.children() {
            match child.tag_name().name() {
                "navLabel" => title = collect_text(&child),
                "content" => {
                    if href.is_none() {
                        href = child.attribute("src").map(|s| s.to_string());
                    }
                }
                _ => {}
            }
        }
        *depth += 1;
        let mut children = Vec::new();
        collect_ncx(node.children(), base, &mut children, depth);
        *depth -= 1;
        if let Some(h) = href {
            if let Some(resolved) = resolve_href(base, &h) {
                out.push(TocNode {
                    title: if title.is_empty() { "Untitled".into() } else { title },
                    href: resolved,
                    depth: *depth,
                    children,
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------
// text handling
// ---------------------------------------------------------------------------

fn collect_text(node: &Node<'_, '_>) -> String {
    let parts: Vec<String> = node
        .descendants()
        .filter(|n| n.is_text())
        .filter_map(|n| n.text())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    parts.join(" ")
}

fn collapse(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut last_space = true;
    for ch in input.chars() {
        if ch.is_whitespace() {
            if !last_space {
                out.push(' ');
            }
            last_space = true;
        } else {
            out.push(ch);
            last_space = false;
        }
    }
    out.trim().to_string()
}

pub fn strip_tags(html: &str) -> String {
    // Drop script/style bodies first so their contents never become text.
    // (No backreferences: the `regex` crate does not support them.)
    static RE: OnceLock<Vec<Regex>> = OnceLock::new();
    let regexes = RE.get_or_init(|| {
        vec![
            Regex::new(r"(?is)<script\b[^>]*>.*?</\s*script\s*>").unwrap(),
            Regex::new(r"(?is)<style\b[^>]*>.*?</\s*style\s*>").unwrap(),
            Regex::new(r"(?is)<!--.*?-->").unwrap(),
        ]
    });
    let mut cleaned = html.to_string();
    for re in regexes {
        cleaned = re.replace_all(&cleaned, " ").into_owned();
    }

    let mut out = String::with_capacity(cleaned.len());
    let mut in_tag = false;
    for ch in cleaned.chars() {
        if in_tag {
            if ch == '>' {
                in_tag = false;
                out.push(' ');
            }
            continue;
        }
        if ch == '<' {
            in_tag = true;
            continue;
        }
        out.push(ch);
    }
    decode_entities(&out)
}

pub(crate) fn decode_entities(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'&' {
            if let Some(end) = input[i..].find(';').filter(|e| *e <= 12) {
                let name = &input[i + 1..i + end];
                let replacement = match name {
                    "amp" => Some("&".to_string()),
                    "lt" => Some("<".to_string()),
                    "gt" => Some(">".to_string()),
                    "quot" => Some("\"".to_string()),
                    "apos" | "#39" => Some("'".to_string()),
                    "nbsp" => Some("\u{a0}".to_string()),
                    _ => name
                        .strip_prefix('#')
                        .and_then(|n| {
                            if let Some(hex) = n.strip_prefix('x').or_else(|| n.strip_prefix('X')) {
                                u32::from_str_radix(hex, 16).ok()
                            } else {
                                n.parse::<u32>().ok()
                            }
                        })
                        .and_then(char::from_u32)
                        .map(|c| c.to_string()),
                };
                if let Some(r) = replacement {
                    out.push_str(&r);
                    i += end + 1;
                    continue;
                }
            }
        }
        let ch_len = utf8_len(bytes[i]);
        out.push_str(&input[i..i + ch_len]);
        i += ch_len;
    }
    out
}

fn utf8_len(byte: u8) -> usize {
    match byte {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        _ => 4,
    }
}

pub fn html_to_text(html: &str) -> String {
    // Drop non-content elements entirely first. Block elements become spaces
    // so adjacent runs do not merge into a single word.
    static RE: OnceLock<Vec<Regex>> = OnceLock::new();
    let regexes = RE.get_or_init(|| {
        vec![
            Regex::new(r"(?is)<script\b[^>]*>.*?</\s*script\s*>").unwrap(),
            Regex::new(r"(?is)<style\b[^>]*>.*?</\s*style\s*>").unwrap(),
            Regex::new(r"(?is)<head\b[^>]*>.*?</\s*head\s*>").unwrap(),
            Regex::new(r"(?is)<noscript\b[^>]*>.*?</\s*noscript\s*>").unwrap(),
            Regex::new(r"(?is)<!--.*?-->").unwrap(),
            Regex::new(
                r"(?is)</(p|div|h[1-6]|li|tr|td|th|blockquote|section|article|figcaption)\s*>",
            )
            .unwrap(),
            Regex::new(r"(?is)<br\s*/?>").unwrap(),
            Regex::new(r"(?is)<hr\s*/?>").unwrap(),
        ]
    });
    let mut cleaned = html.to_string();
    for re in regexes {
        cleaned = re.replace_all(&cleaned, " ").into_owned();
    }
    collapse(&strip_tags(&cleaned))
}

fn escape_attr(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn extract_title(html: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"(?is)<h[1-3][^>]*>(.*?)</h[1-3]>").unwrap());
    for caps in re.captures_iter(html) {
        let title = collapse(&strip_tags(caps.get(1).map(|m| m.as_str()).unwrap_or("")));
        if !title.is_empty() {
            return Some(title);
        }
    }
    static TITLE_RE: OnceLock<Regex> = OnceLock::new();
    let re = TITLE_RE.get_or_init(|| Regex::new(r"(?is)<title[^>]*>(.*?)</title>").unwrap());
    for caps in re.captures_iter(html) {
        let title = collapse(&strip_tags(caps.get(1).map(|m| m.as_str()).unwrap_or("")));
        if !title.is_empty() {
            return Some(title);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// HTML sanitization (ammonia allow-list)
// ---------------------------------------------------------------------------

fn sanitize_html(html: &str) -> String {
    use std::collections::HashSet;

    let tags: HashSet<&str> = [
        "a", "abbr", "address", "article", "aside", "b", "bdi", "bdo", "blockquote", "br",
        "caption", "cite", "code", "col", "colgroup", "data", "dd", "del", "details", "dfn",
        "div", "dl", "dt", "em", "figcaption", "figure", "footer", "h1", "h2", "h3", "h4", "h5",
        "h6", "header", "hgroup", "hr", "i", "img", "ins", "kbd", "li", "main", "mark", "nav",
        "ol", "p", "pre", "q", "rp", "rt", "ruby", "s", "samp", "section", "small", "span",
        "strong", "sub", "summary", "sup", "table", "tbody", "td", "tfoot", "th", "thead",
        "time", "tr", "u", "ul", "var", "wbr",
        // SVG subset for inline cover art and maths-free decorations.
        "svg", "g", "path", "circle", "ellipse", "line", "polygon", "polyline", "rect", "text",
        "tspan", "defs", "use", "symbol", "clippath", "mask", "lineargradient", "radialgradient",
        "stop", "pattern", "image", "title", "desc",
        // MathML subset, needed for technical books.
        "math", "mrow", "mi", "mn", "mo", "msub", "msup", "msubsup", "mfrac", "msqrt", "mtext",
        "mspace", "mtable", "mtr", "mtd", "munder", "mover", "munderover", "mstyle", "mpadded",
    ]
    .into_iter()
    .collect();

    let mut builder = ammonia::Builder::new();
    builder
        .tags(tags)
        .add_generic_attributes(&[
            "class", "id", "title", "lang", "dir", "style", "role", "aria-label",
            "aria-hidden", "aria-level", "aria-describedby", "xml:lang", "alt", "epub:type",
            "data-href", "data-kind", "data-external", "data-epub", "epub-href", "span",
            "col", "rowspan", "colspan", "width", "height", "viewbox", "viewBox",
            "preserveAspectRatio", "version", "xmlns", "fill", "stroke", "stroke-width",
            "transform", "cx", "cy", "r", "rx", "ry", "x", "y", "x1", "y1", "x2", "y2",
            "d", "points", "offset", "stop-color", "stop-opacity", "font-size", "text-anchor",
            "xlink:href", "href", "datetime", "value", "scope", "span", "clip", "mask",
        ])
        .add_tag_attributes("img", &["src", "width", "height", "decoding", "loading"])
        .add_tag_attributes("td", &["colspan", "rowspan", "align", "headers"])
        .add_tag_attributes("th", &["colspan", "rowspan", "scope", "align", "headers"])
        .add_tag_attributes("col", &["span", "width", "align"])
        .add_tag_attributes("colgroup", &["span", "align"])
        .add_tag_attributes("ol", &["start", "type"])
        .add_tag_attributes("ul", &["type"])
        .add_tag_attributes("table", &["summary", "border", "cellpadding", "cellspacing"])
        .add_tag_attributes("time", &["datetime"])
        .add_tag_attributes("data", &["value"])
// Only inline images may keep a URL; everything else loses its href.
        .url_schemes(["data"].into_iter().collect())
        .url_relative(ammonia::UrlRelative::Deny)
        .link_rel(None)
        // Defence in depth: scrub `style` here too, so the guarantee does not
        // depend on the caller having pre-processed the fragment.
        .attribute_filter(|element, attribute, value| {
            if attribute.eq_ignore_ascii_case("style") {
                let scrubbed = scrub_css(value);
                return Some(std::borrow::Cow::Owned(scrubbed));
            }
            if attribute.starts_with("on") || attribute.eq_ignore_ascii_case("srcdoc") {
                return None;
            }
            let _ = element;
            Some(std::borrow::Cow::Borrowed(value))
        })
        .clean_content_tags(
            ["script", "style", "iframe", "object", "embed", "form", "noscript"]
                .into_iter()
                .collect(),
        );

    builder.clean(html).to_string()
}

/// Thin wrappers so the sanitizer, the style guard, the decoder and the excerpt
/// builder can be exercised directly from tests.
#[cfg(test)]
pub fn sanitize_html_for_test(html: &str) -> String {
    sanitize_html(html)
}
#[cfg(test)]
pub fn guard_style_for_test(css: &str) -> String {
    guard_style_block(css)
}
#[cfg(test)]
pub fn decode_for_test(bytes: &[u8]) -> String {
    decode_markup(bytes)
}
#[cfg(test)]
pub fn excerpt_for_test(text: &str, start: usize, end: usize, needle: &str) -> String {
    build_excerpt(text, start, end, needle)
}
#[cfg(test)]
pub fn decode_css_escapes_for_test(css: &str) -> String {
    decode_css_escapes(css)
}

// ---------------------------------------------------------------------------
// search
// ---------------------------------------------------------------------------

pub fn search_book(epub: &mut Epub, query: &str, max_hits: usize) -> Vec<SearchHit> {
    if epub.ensure_manifest().is_err() {
        return Vec::new();
    }
    let query = query.trim();
    if query.len() < 2 {
        return Vec::new();
    }
    let needle = query.to_lowercase();
    let mut hits = Vec::new();
    let chapter_count = epub.manifest.chapters.len();

    for index in 0..chapter_count {
        if hits.len() >= max_hits {
            break;
        }
        let Ok((href, text)) = epub.chapter_text(index) else {
            continue;
        };
        let haystack = text.to_lowercase();
        let title = epub.manifest.chapters[index].title.clone();

        let mut from = 0usize;
        let mut found_here = 0usize;
        while let Some(pos) = haystack[from..].find(&needle) {
            let start = from + pos;
            let end = start + needle.len();
            hits.push(SearchHit {
                href: href.clone(),
                title: title.clone(),
                chapter_index: index,
                excerpt: build_excerpt(&text, start, end, &needle),
            });
            found_here += 1;
            if hits.len() >= max_hits || found_here >= 8 {
                break;
            }
            from = end.max(start + 1);
        }
    }
    hits
}

fn build_excerpt(text: &str, start: usize, end: usize, needle: &str) -> String {
    // Snap to word boundaries so the excerpt reads naturally.
    let mut lo = start.saturating_sub(90);
    let mut hi = (end + 90).min(text.len());
    while lo > 0 && !text.is_char_boundary(lo) {
        lo -= 1;
    }
    while hi < text.len() && !text.is_char_boundary(hi) {
        hi += 1;
    }
    let mut prefix = String::new();
    let mut suffix = String::new();
    if lo > 0 {
        prefix.push('…');
    }
    if hi < text.len() {
        suffix.push('…');
    }
    let middle = &text[lo..hi];

    // Case folding is NOT length preserving (U+0130 folds to 3 bytes from 2), so
    // an offset taken from `middle.to_lowercase()` cannot index back into
    // `middle`. Map it forward with a character walk that accumulates folded
    // lengths. Getting this wrong panics, and `panic = "abort"` kills the
    // whole process rather than failing one command.
    let folded = middle.to_lowercase();
    let mut body = format!("{prefix}{middle}{suffix}");
    if let Some(rel) = folded.find(&needle.to_lowercase()) {
        let mut m_start = middle.len();
        let mut acc = 0usize;
        for (i, c) in middle.char_indices() {
            if acc >= rel {
                m_start = i;
                break;
            }
            acc += folded_len(c);
        }
        let m_end = (m_start + needle.len()).min(middle.len());
        body = format!(
            "{prefix}{}[[{}]]{}{suffix}",
            &middle[..m_start],
            &middle[m_start..m_end],
            &middle[m_end..]
        );
    }
    body
}