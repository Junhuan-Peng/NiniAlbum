mod commands;
mod db;
mod error;
mod export;
mod models;
mod scan;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(db::AppState::default())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            commands::create_project,
            commands::replace_project,
            commands::open_project,
            commands::get_snapshot,
            commands::scan_project,
            commands::photo_root_has_changes,
            commands::get_photo_preview,
            commands::get_similarity_photos,
            commands::set_similarity_hidden,
            commands::create_tag,
            commands::rename_tag,
            commands::inspect_tag_delete,
            commands::delete_tag,
            commands::assign_tag,
            commands::set_photo_trashed,
            commands::save_sort,
            commands::delete_missing,
            commands::checkpoint,
            commands::plan_export,
            commands::export_photos,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Nini Albums");
}
