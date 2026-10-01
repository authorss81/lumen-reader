use serde::{Deserialize, Serialize};

/// Hard limits applied to every untrusted EPUB we open. These exist to stop
/// decompression bombs, absurdly large assets and pathological documents.
pub mod limits {
    /// Maximum number of entries we will even look at inside the zip.
    pub const MAX_ENTRIES: usize = 20_000;
    /// Maximum size of a single uncompressed file we are willing to buffer (32 MiB).
    pub const MAX_ENTRY_BYTES: u64 = 32 * 1024 * 1024;
    /// Maximum cumulative uncompressed bytes we will read from one book (512 MiB).
    pub const MAX_TOTAL_UNCOMPRESSED: u64 = 512 * 1024 * 1024;
    /// Refuse any entry whose compression ratio looks like a zip bomb (1:200).
    pub const MAX_COMPRESSION_RATIO: u64 = 200;
    /// Maximum combined size of images inlined into a single chapter (24 MiB).
    pub const MAX_INLINE_ASSET_BYTES: u64 = 24 * 1024 * 1024;
    /// Maximum number of images inlined into a single chapter.
    pub const MAX_INLINE_ASSETS: usize = 512;
    /// Maximum size of the book manifest / container / ncx documents.
    pub const MAX_XML_BYTES: u64 = 8 * 1024 * 1024;
    /// Maximum number of spine documents we will expose.
    pub const MAX_SPINE_ITEMS: usize = 30_000;
    /// Maximum depth of the navigation tree.
    pub const MAX_TOC_DEPTH: usize = 32;
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct BookMeta {
    pub id: String,
    pub path: String,
    pub title: String,
    pub author: String,
    pub language: String,
    pub publisher: String,
    pub description: String,
    pub identifier: String,
    pub rights: String,
    pub cover_href: Option<String>,
    pub file_size: i64,
    pub mtime: i64,
    pub added_at: i64,
    pub last_opened: Option<i64>,
    /// 0.0 - 1.0 across the whole book.
    pub progress: f64,
    /// JSON payload describing where reading stopped.
    pub locator: String,
    /// Number of spine documents.
    pub chapters: usize,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Chapter {
    /// Zip-relative path of the spine document.
    pub href: String,
    /// Best-effort title derived from the document heading or nav label.
    pub title: String,
    /// Index into the reading order.
    pub index: usize,
    pub bytes: u64,
    /// `false` when the spine marks the item `linear="no"`.
    pub linear: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct TocNode {
    pub title: String,
    /// Zip-relative path plus optional `#fragment`.
    pub href: String,
    pub depth: usize,
    pub children: Vec<TocNode>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct Manifest {
    pub title: String,
    pub author: String,
    pub language: String,
    pub publisher: String,
    pub description: String,
    pub identifier: String,
    pub rights: String,
    pub cover_href: Option<String>,
    pub toc: Vec<TocNode>,
    pub page_list: Vec<TocNode>,
    pub landmarks: Vec<TocNode>,
    pub chapters: Vec<Chapter>,
    pub opf_path: String,
}

/// A chapter payload handed to the webview.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChapterContent {
    pub href: String,
    pub title: String,
    pub index: usize,
    /// Sanitized HTML, fully self-contained (inline CSS + data: images).
    pub html: String,
    /// True when this was the last document in the reading order.
    pub is_last: bool,
    /// True when the previous document in the reading order exists.
    pub has_prev: bool,
    pub has_next: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SearchHit {
    pub href: String,
    pub title: String,
    pub chapter_index: usize,
    /// Plain-text excerpt with the query already highlighted with `[[ ]]` markers.
    pub excerpt: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Annotation {
    pub id: i64,
    pub book_id: String,
    pub href: String,
    pub selector: String,
    pub quote: String,
    pub note: String,
    pub color: String,
    pub created_at: i64,
    pub modified_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Bookmark {
    pub id: i64,
    pub book_id: String,
    pub href: String,
    pub locator: String,
    pub label: String,
    pub created_at: i64,
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Deterministic 64-bit FNV-1a, rendered as hex. Stable across builds and
/// platforms so a book keeps the same id between sessions.
pub fn stable_id(input: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in input.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}