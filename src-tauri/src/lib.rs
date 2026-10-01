mod commands;
mod epub;
mod model;
mod store;
#[cfg(test)]
mod tests;

use commands::AppState;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let store = store::Store::open(&data_dir.join("library.sqlite3"))?;
            app.manage(AppState::new(store));
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
            commands::get_settings,
            commands::set_setting,
            commands::library_path,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}