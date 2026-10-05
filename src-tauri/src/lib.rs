mod baidu;
mod commands;
mod db;
mod error;
mod export;
mod models;
mod scan;
mod tasks;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(db::AppState::default())
        .manage(tasks::ExportTaskManager::default())
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
            commands::assign_tags,
            commands::create_manual_similarity_group,
            commands::remove_from_similarity_group,
            commands::dissolve_similarity_group,
            commands::set_tag_shortcut,
            commands::set_photo_trashed,
            commands::save_sort,
            commands::delete_missing,
            commands::checkpoint,
            commands::plan_export,
            commands::start_local_export,
            commands::baidu_status,
            commands::open_baidu_authorization,
            commands::disconnect_baidu,
            commands::list_baidu_folders,
            commands::create_baidu_folder,
            commands::plan_baidu_export,
            commands::start_baidu_export,
            commands::list_export_tasks,
            commands::pause_export_task,
            commands::resume_export_task,
            commands::cancel_export_task,
            commands::clear_finished_export_tasks,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Nini Albums");
}
