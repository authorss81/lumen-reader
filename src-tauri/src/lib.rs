mod commands;
mod epub;
mod model;
mod store;
#[cfg(test)]
mod tests;

use commands::AppState;
use tauri::{AppHandle, Manager};

/// Keep only arguments that look like real EPUB files on disk. The single
/// instance plugin also hands us the launch arguments of the *first* process,
/// and a file association appends the path to the command line.
fn epub_paths_from<I: IntoIterator<Item = String>>(args: I) -> Vec<String> {
    args.into_iter()
        .filter(|arg| !arg.starts_with('-'))
        .filter(|arg| arg.to_ascii_lowercase().ends_with(".epub"))
        .filter(|arg| std::path::Path::new(arg).is_file())
        .collect()
}

/// Queue EPUB paths for the webview to import and open once it is listening.
fn queue_opens(app: &AppHandle, paths: Vec<String>) {
    if paths.is_empty() {
        return;
    }
    let state = app.state::<AppState>();
    state.queue_opens(paths);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Registered first so a second launch is intercepted before any other
        // plugin does work.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
            queue_opens(app, epub_paths_from(argv));
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
.setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let store = store::Store::open(&data_dir.join("library.sqlite3"))?;
            app.manage(AppState::new(store));
            // A file association launches us as `lumen-reader.exe book.epub`.
            queue_opens(app.handle(), epub_paths_from(std::env::args()));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::import_books,
            commands::list_books,
            commands::open_book,
            commands::load_chapter,
            commands::search_in_book,
            commands::get_cover,
            commands::remove_book,
            commands::update_book,
            commands::save_progress,
            commands::reveal_in_explorer,
            commands::list_annotations,
            commands::add_annotation,
            commands::update_annotation,
            commands::delete_annotation,
            commands::list_bookmarks,
            commands::add_bookmark,
            commands::delete_bookmark,
            commands::rename_bookmark,
            commands::get_settings,
            commands::set_setting,
            commands::library_path,
            commands::take_pending_opens,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}