use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::{Manager, State};

use crate::epub::{self, Epub};
use crate::model::{self, Annotation, BookMeta, Bookmark, ChapterContent, Manifest, SearchHit};
use crate::store::Store;

const MANIFEST_CACHE_MAX: usize = 16;

pub struct AppState {
    store: Mutex<Store>,
    /// Parsed OPF/NCX caches keyed by book path. Re-parsing XML on every chapter
    /// request would dominate response time, and this keeps the hot path cheap.
    manifest_cache: Mutex<HashMap<String, (i64, Arc<Manifest>)>>,
    cache_order: Mutex<Vec<String>>,
    /// EPUB paths handed over by the shell (file association, or a second
    /// launch while we are already running). Drained by the webview.
    pending_opens: Mutex<Vec<String>>,
}

impl AppState {
    pub fn new(store: Store) -> Self {
        Self {
            store: Mutex::new(store),
            manifest_cache: Mutex::new(HashMap::new()),
            cache_order: Mutex::new(Vec::new()),
            pending_opens: Mutex::new(Vec::new()),
        }
    }

    pub fn queue_opens(&self, paths: Vec<String>) {
        if let Ok(mut pending) = self.pending_opens.lock() {
            for path in paths {
                if !pending.contains(&path) {
                    pending.push(path);
                }
            }
        }
    }

    fn take_pending_opens(&self) -> CmdResult<Vec<String>> {
        let mut pending = self.pending_opens.lock().map_err(err)?;
        Ok(std::mem::take(&mut *pending))
    }

    fn touch_cache(&self, path: &str) {
        if let Ok(mut order) = self.cache_order.lock() {
            if let Some(pos) = order.iter().position(|p| p == path) {
                order.remove(pos);
            }
            order.push(path.to_string());
            while order.len() > MANIFEST_CACHE_MAX {
                if let Some(evicted) = order.first().cloned() {
                    order.remove(0);
                    if let Ok(mut cache) = self.manifest_cache.lock() {
                        cache.remove(&evicted);
                    }
                }
            }
        }
    }

    fn store(&self) -> std::sync::MutexGuard<'_, Store> {
        self.store.lock().expect("store mutex poisoned")
    }
}

type CmdResult<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/// Open the EPUB behind a stored book id, reusing the cached manifest when the
/// file on disk has not changed.
fn with_epub<F, R>(state: &AppState, book_id: &str, f: F) -> CmdResult<R>
where
    F: FnOnce(&mut Epub) -> CmdResult<R>,
{
    let book = state
        .store()
        .get_book(book_id)
        .map_err(err)?
        .ok_or_else(|| format!("book {book_id} is not in the library"))?;
    let path = PathBuf::from(&book.path);
    let mtime = std::fs::metadata(&path).map(|m| mtime_of(&m)).unwrap_or(0);

    let cached = {
        let cache = state.manifest_cache.lock().map_err(err)?;
        cache
            .get(&book.path)
            .filter(|entry| entry.0 == mtime)
            .map(|entry| entry.1.clone())
    };

    let mut file = Epub::open(&path).map_err(err)?;
    if let Some(manifest) = cached {
        if manifest.chapters.len() == file.manifest.chapters.len() {
            file.manifest = manifest.as_ref().clone();
        }
    } else {
        let manifest = Arc::new(file.manifest.clone());
        {
            let mut cache = state.manifest_cache.lock().map_err(err)?;
            cache.insert(book.path.clone(), (mtime, manifest));
        }
        state.touch_cache(&book.path);
    }
    f(&mut file)
}

fn mtime_of(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// library
// ---------------------------------------------------------------------------

fn probe(path: &Path) -> CmdResult<BookMeta> {
    let meta = std::fs::metadata(path).map_err(err)?;
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let path_str = canonical.to_string_lossy().to_string();

    let m = Epub::open(&canonical).map_err(err)?.manifest.clone();
    let file_size = meta.len() as i64;
    let mtime = mtime_of(&meta);

    let fallback_title = canonical
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "Untitled".into());

    Ok(BookMeta {
        id: model::stable_id(&path_str),
        path: path_str,
        title: if m.title.trim().is_empty() { fallback_title } else { m.title },
        author: m.author,
        language: m.language,
        publisher: m.publisher,
        description: m.description,
        identifier: m.identifier,
        rights: m.rights,
        cover_href: m.cover_href,
        file_size,
        mtime,
        added_at: model::now(),
        last_opened: None,
        progress: 0.0,
        locator: String::new(),
        chapters: m.chapters.len(),
    })
}

#[derive(Serialize)]
pub struct ImportReport {
    pub added: Vec<BookMeta>,
    pub failed: Vec<ImportFailure>,
}

#[derive(Serialize)]
pub struct ImportFailure {
    pub path: String,
    pub error: String,
}

#[tauri::command]
pub fn import_books(state: State<'_, AppState>, paths: Vec<String>) -> CmdResult<ImportReport> {
    let mut added = Vec::new();
    let mut failed = Vec::new();

    for raw in paths {
        let path = PathBuf::from(&raw);
        match probe(&path) {
            Ok(book) => {
                // Preserve existing reading progress when re-importing.
                let merged = {
                    let store = state.store();
                    match store.get_book(&book.id) {
                        Ok(Some(existing)) => BookMeta {
                            progress: existing.progress,
                            locator: existing.locator,
                            last_opened: existing.last_opened,
                            added_at: existing.added_at,
                            ..book
                        },
                        _ => book,
                    }
                };
                let stored = merged.clone();
                if let Err(e) = state.store().upsert_book(&stored) {
                    failed.push(ImportFailure { path: raw, error: err(e) });
                    continue;
                }
                added.push(merged);
            }
            Err(error) => failed.push(ImportFailure { path: raw, error }),
        }
    }
    Ok(ImportReport { added, failed })
}

#[tauri::command]
pub fn list_books(state: State<'_, AppState>) -> CmdResult<Vec<BookMeta>> {
    state.store().list_books().map_err(err)
}

#[derive(Serialize)]
pub struct BookDetail {
    #[serde(flatten)]
    pub book: BookMeta,
    pub toc: Vec<model::TocNode>,
    pub page_list: Vec<model::TocNode>,
    pub landmarks: Vec<model::TocNode>,
    pub chapters: Vec<model::Chapter>,
    pub cover: Option<String>,
}

#[tauri::command]
pub fn open_book(state: State<'_, AppState>, id: String) -> CmdResult<BookDetail> {
    let book = {
        let store = state.store();
        let Some(mut book) = store.get_book(&id).map_err(err)? else {
            return Err(format!("book {id} is not in the library"));
        };
        store.touch(&id).map_err(err)?;
        book.chapters = 0;
        book
    };

    let (toc, page_list, landmarks, chapters, cover) =
        with_epub(&state, &id, |file| {
            let cover = file.cover_data_url().ok().flatten();
            Ok((
                file.manifest.toc.clone(),
                file.manifest.page_list.clone(),
                file.manifest.landmarks.clone(),
                file.manifest.chapters.clone(),
                cover,
            ))
        })?;

    Ok(BookDetail {
        book: BookMeta { chapters: chapters.len(), ..book },
        toc,
        page_list,
        landmarks,
        chapters,
        cover,
    })
}

#[tauri::command]
pub fn load_chapter(state: State<'_, AppState>, id: String, index: usize) -> CmdResult<ChapterContent> {
    with_epub(&state, &id, |file| {
        let mut content = file.chapter(index).map_err(err)?;
        if content.title.is_empty() {
            if let Some(chapter) = file.manifest.chapters.get(index) {
                content.title = chapter.title.clone();
            }
        }
        Ok(content)
    })
}

#[tauri::command]
pub fn search_in_book(
    state: State<'_, AppState>,
    id: String,
    query: String,
    limit: Option<usize>,
) -> CmdResult<Vec<SearchHit>> {
    let limit = limit.unwrap_or(200).clamp(1, 2000);
    with_epub(&state, &id, |file| Ok(epub::search_book(file, &query, limit)))
}

#[tauri::command]
pub fn get_cover(state: State<'_, AppState>, id: String) -> CmdResult<Option<String>> {
    with_epub(&state, &id, |file| Ok(file.cover_data_url().unwrap_or(None)))
}

#[tauri::command]
pub fn remove_book(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    if let Some(book) = state.store().get_book(&id).map_err(err)? {
        state.store().remove_book(&id).map_err(err)?;
        if let Ok(mut cache) = state.manifest_cache.lock() {
            cache.remove(&book.path);
        }
        if let Ok(mut order) = state.cache_order.lock() {
            order.retain(|p| p != &book.path);
        }
    }
    Ok(())
}

#[tauri::command]
pub fn update_book(
    state: State<'_, AppState>,
    id: String,
    title: String,
    author: String,
) -> CmdResult<()> {
    let (progress, locator) = state
        .store()
        .get_book(&id)
        .map_err(err)?
        .map(|b| (b.progress, b.locator))
        .unwrap_or((0.0, String::new()));
    state
        .store()
        .update_details(&id, &title, &author, progress, &locator)
        .map_err(err)
}

#[tauri::command]
pub fn save_progress(
    state: State<'_, AppState>,
    id: String,
    progress: f64,
    locator: String,
) -> CmdResult<()> {
    state.store().set_progress(&id, progress, &locator).map_err(err)
}

#[tauri::command]
pub fn reveal_in_explorer(path: String) -> CmdResult<()> {
    let file = PathBuf::from(&path);
    if !file.exists() {
        return Err("file no longer exists".into());
    }
    std::process::Command::new("explorer")
        .arg("/select,")
        .arg(&file)
        .spawn()
        .map_err(err)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// annotations & bookmarks
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn list_annotations(
    state: State<'_, AppState>,
    book_id: String,
    href: Option<String>,
) -> CmdResult<Vec<Annotation>> {
    state
        .store()
        .list_annotations(&book_id, href.as_deref())
        .map_err(err)
}

#[derive(Deserialize)]
pub struct NewAnnotation {
    pub book_id: String,
    pub href: String,
    pub selector: String,
    pub quote: String,
    pub note: String,
    pub color: String,
}

#[tauri::command]
pub fn add_annotation(state: State<'_, AppState>, payload: NewAnnotation) -> CmdResult<Annotation> {
    state
        .store()
        .add_annotation(
            &payload.book_id,
            &payload.href,
            &payload.selector,
            &payload.quote,
            &payload.note,
            &payload.color,
        )
        .map_err(err)
}

#[derive(Deserialize)]
pub struct AnnotationPatch {
    pub id: i64,
    pub note: Option<String>,
    pub color: Option<String>,
    pub quote: Option<String>,
}

#[tauri::command]
pub fn update_annotation(state: State<'_, AppState>, payload: AnnotationPatch) -> CmdResult<()> {
    state
        .store()
        .update_annotation(
            payload.id,
            payload.note.as_deref(),
            payload.color.as_deref(),
            payload.quote.as_deref(),
        )
        .map_err(err)
}

#[tauri::command]
pub fn delete_annotation(state: State<'_, AppState>, id: i64) -> CmdResult<()> {
    state.store().delete_annotation(id).map_err(err)
}

#[tauri::command]
pub fn list_bookmarks(state: State<'_, AppState>, book_id: String) -> CmdResult<Vec<Bookmark>> {
    state.store().list_bookmarks(&book_id).map_err(err)
}

#[tauri::command]
pub fn add_bookmark(
    state: State<'_, AppState>,
    book_id: String,
    href: String,
    locator: String,
    label: String,
) -> CmdResult<Bookmark> {
    state
        .store()
        .add_bookmark(&book_id, &href, &locator, &label)
        .map_err(err)
}

#[tauri::command]
pub fn delete_bookmark(state: State<'_, AppState>, id: i64) -> CmdResult<()> {
    state.store().delete_bookmark(id).map_err(err)
}

// ---------------------------------------------------------------------------
// settings
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> CmdResult<serde_json::Value> {
    state.store().get_settings().map_err(err)
}

#[tauri::command]
pub fn set_setting(
    state: State<'_, AppState>,
    key: String,
    value: serde_json::Value,
) -> CmdResult<()> {
    state.store().set_setting(&key, &value).map_err(err)
}

#[tauri::command]
pub fn library_path(app: tauri::AppHandle) -> CmdResult<String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(err)?
        .join("library");
    std::fs::create_dir_all(&dir).map_err(err)?;
    Ok(dir.to_string_lossy().to_string())
}

/// Returns and clears any EPUB paths the shell asked us to open. The webview
/// calls this on boot and whenever the window regains focus.
#[tauri::command]
pub fn take_pending_opens(state: State<'_, AppState>) -> CmdResult<Vec<String>> {
    state.take_pending_opens()
}