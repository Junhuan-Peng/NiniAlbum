use std::path::PathBuf;

use tauri::{AppHandle, Emitter, Manager, State};

use crate::{
    baidu,
    db::{self, AppState},
    error::{AppError, CommandResult},
    export,
    models::{
        BaiduDirectoryListing, BaiduStatus, DeleteImpact, ExportPlan, ExportTask, PhotoQuery,
        ProjectInfo, ProjectSnapshot, ScanSummary,
    },
    scan,
    tasks::ExportTaskManager,
};

#[tauri::command]
pub fn create_project(
    state: State<'_, AppState>,
    database_path: String,
    photo_root: String,
) -> Result<ProjectInfo, String> {
    let database_path = PathBuf::from(database_path);
    db::create_project(&database_path, &PathBuf::from(photo_root)).command()?;
    let info = db::open_project(&database_path).command()?;
    state.set_project(database_path);
    Ok(info)
}

#[tauri::command]
pub fn replace_project(
    state: State<'_, AppState>,
    database_path: String,
    photo_root: String,
) -> Result<ProjectInfo, String> {
    let database_path = PathBuf::from(database_path);
    db::replace_project(&database_path, &PathBuf::from(photo_root)).command()?;
    let info = db::open_project(&database_path).command()?;
    state.set_project(database_path);
    Ok(info)
}

#[tauri::command]
pub fn open_project(
    state: State<'_, AppState>,
    database_path: String,
) -> Result<ProjectInfo, String> {
    let database_path = PathBuf::from(database_path);
    let info = db::open_project(&database_path).command()?;
    state.set_project(database_path);
    Ok(info)
}

#[tauri::command]
pub fn get_snapshot(
    state: State<'_, AppState>,
    query: Option<PhotoQuery>,
) -> Result<ProjectSnapshot, String> {
    db::snapshot(&state.project_path().command()?, &query.unwrap_or_default()).command()
}

#[tauri::command]
pub async fn scan_project(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<ScanSummary, String> {
    let path = state.project_path().command()?;
    let progress = std::sync::Arc::new(move |update| {
        let _ = app.emit("scan-progress", update);
    });
    tauri::async_runtime::spawn_blocking(move || {
        scan::scan_project_with_progress(&path, progress).command()
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn photo_root_has_changes(state: State<'_, AppState>) -> Result<bool, String> {
    let path = state.project_path().command()?;
    tauri::async_runtime::spawn_blocking(move || scan::photo_root_has_changes(&path).command())
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn get_photo_preview(
    state: State<'_, AppState>,
    photo_id: i64,
) -> Result<String, String> {
    let path = state.project_path().command()?;
    tauri::async_runtime::spawn_blocking(move || scan::photo_preview(&path, photo_id).command())
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub fn get_similarity_photos(
    state: State<'_, AppState>,
    photo_id: i64,
) -> Result<Vec<crate::models::PhotoEntry>, String> {
    let connection = db::connect(&state.project_path().command()?).command()?;
    db::similarity_photos(&connection, photo_id).command()
}

#[tauri::command]
pub fn set_similarity_hidden(
    state: State<'_, AppState>,
    photo_id: i64,
    hidden: bool,
) -> Result<(), String> {
    let connection = db::connect(&state.project_path().command()?).command()?;
    db::set_similarity_hidden(&connection, photo_id, hidden).command()
}

#[tauri::command]
pub fn create_tag(
    state: State<'_, AppState>,
    parent_id: Option<i64>,
    name: String,
) -> Result<i64, String> {
    let mut connection = db::connect(&state.project_path().command()?).command()?;
    db::create_tag_path(&mut connection, parent_id, &name).command()
}

#[tauri::command]
pub fn rename_tag(state: State<'_, AppState>, id: i64, name: String) -> Result<(), String> {
    let connection = db::connect(&state.project_path().command()?).command()?;
    db::rename_tag(&connection, id, &name).command()
}

#[tauri::command]
pub fn inspect_tag_delete(state: State<'_, AppState>, id: i64) -> Result<DeleteImpact, String> {
    let connection = db::connect(&state.project_path().command()?).command()?;
    db::delete_impact(&connection, id).command()
}

#[tauri::command]
pub fn delete_tag(state: State<'_, AppState>, id: i64, unassign: bool) -> Result<(), String> {
    let mut connection = db::connect(&state.project_path().command()?).command()?;
    db::delete_tag(&mut connection, id, unassign).command()
}

#[tauri::command]
pub fn assign_tag(
    state: State<'_, AppState>,
    photo_id: i64,
    tag_id: Option<i64>,
) -> Result<(), String> {
    let connection = db::connect(&state.project_path().command()?).command()?;
    db::assign_tag(&connection, photo_id, tag_id).command()
}

#[tauri::command]
pub fn assign_tags(
    state: State<'_, AppState>,
    photo_ids: Vec<i64>,
    tag_id: Option<i64>,
) -> Result<usize, String> {
    let mut connection = db::connect(&state.project_path().command()?).command()?;
    db::assign_tags(&mut connection, &photo_ids, tag_id).command()
}

#[tauri::command]
pub fn create_manual_similarity_group(
    state: State<'_, AppState>,
    photo_ids: Vec<i64>,
) -> Result<String, String> {
    let mut connection = db::connect(&state.project_path().command()?).command()?;
    db::create_manual_similarity_group(&mut connection, &photo_ids).command()
}

#[tauri::command]
pub fn remove_from_similarity_group(
    state: State<'_, AppState>,
    photo_id: i64,
) -> Result<(), String> {
    let connection = db::connect(&state.project_path().command()?).command()?;
    db::remove_from_similarity_group(&connection, photo_id).command()
}

#[tauri::command]
pub fn dissolve_similarity_group(
    state: State<'_, AppState>,
    photo_id: i64,
) -> Result<usize, String> {
    let connection = db::connect(&state.project_path().command()?).command()?;
    db::dissolve_similarity_group(&connection, photo_id).command()
}

#[tauri::command]
pub fn set_tag_shortcut(
    state: State<'_, AppState>,
    tag_id: i64,
    shortcut_key: Option<String>,
) -> Result<(), String> {
    let connection = db::connect(&state.project_path().command()?).command()?;
    db::set_tag_shortcut(&connection, tag_id, shortcut_key.as_deref()).command()
}

#[tauri::command]
pub fn set_photo_trashed(
    state: State<'_, AppState>,
    photo_id: i64,
    trashed: bool,
) -> Result<(), String> {
    let connection = db::connect(&state.project_path().command()?).command()?;
    db::set_photo_trashed(&connection, photo_id, trashed).command()
}

#[tauri::command]
pub fn save_sort(
    state: State<'_, AppState>,
    field: String,
    direction: String,
) -> Result<(), String> {
    let connection = db::connect(&state.project_path().command()?).command()?;
    db::set_sort(&connection, &field, &direction).command()
}

#[tauri::command]
pub fn delete_missing(state: State<'_, AppState>, ids: Vec<i64>) -> Result<usize, String> {
    let mut connection = db::connect(&state.project_path().command()?).command()?;
    db::delete_missing(&mut connection, &ids).command()
}

#[tauri::command]
pub fn checkpoint(state: State<'_, AppState>) -> Result<(), String> {
    let connection = db::connect(&state.project_path().command()?).command()?;
    db::checkpoint(&connection).command()
}

#[tauri::command]
pub fn plan_export(state: State<'_, AppState>, export_root: String) -> Result<ExportPlan, String> {
    export::plan_export(
        &state.project_path().command()?,
        &PathBuf::from(export_root),
    )
    .command()
}

fn emit_export_task(app: &AppHandle, task: &ExportTask) {
    let _ = app.emit("export-task-update", task);
}

#[tauri::command]
pub fn start_local_export(
    app: AppHandle,
    state: State<'_, AppState>,
    tasks: State<'_, ExportTaskManager>,
    export_root: String,
    strategy: String,
) -> Result<ExportTask, String> {
    let database_path = state.project_path().command()?;
    let (task, control) = tasks.create("local", export_root.clone()).command()?;
    emit_export_task(&app, &task);
    let task_id = task.id.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let manager = app.state::<ExportTaskManager>();
        let result = export::export_photos_controlled(
            &database_path,
            &PathBuf::from(export_root),
            &strategy,
            |completed, total, file| {
                if let Some(snapshot) = manager.progress(&task_id, completed, total, file) {
                    emit_export_task(&app, &snapshot);
                }
            },
            || control.checkpoint(),
        );
        let snapshot = match result {
            Ok(summary) => manager.complete(&task_id, summary),
            Err(AppError::Cancelled) => manager.cancelled(&task_id),
            Err(error) => manager.fail(&task_id, error.to_string()),
        };
        if let Some(snapshot) = snapshot {
            emit_export_task(&app, &snapshot);
        }
    });
    Ok(task)
}

#[tauri::command]
pub fn baidu_status() -> Result<BaiduStatus, String> {
    baidu::status().command()
}

#[tauri::command]
pub fn open_baidu_authorization() -> Result<String, String> {
    baidu::open_authorization().command()
}

#[tauri::command]
pub fn disconnect_baidu() -> Result<(), String> {
    baidu::disconnect().command()
}

#[tauri::command]
pub async fn list_baidu_folders(
    directory: Option<String>,
) -> Result<BaiduDirectoryListing, String> {
    tauri::async_runtime::spawn_blocking(move || baidu::list_folders(directory).command())
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn create_baidu_folder(parent: String, name: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || baidu::create_folder(parent, name).command())
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn plan_baidu_export(
    state: State<'_, AppState>,
    root: String,
) -> Result<ExportPlan, String> {
    let database_path = state.project_path().command()?;
    tauri::async_runtime::spawn_blocking(move || {
        baidu::plan_export(&database_path, &root).command()
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub fn start_baidu_export(
    app: AppHandle,
    state: State<'_, AppState>,
    tasks: State<'_, ExportTaskManager>,
    root: String,
    strategy: String,
) -> Result<ExportTask, String> {
    let database_path = state.project_path().command()?;
    let (task, control) = tasks.create("baidu", root.clone()).command()?;
    emit_export_task(&app, &task);
    let task_id = task.id.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let manager = app.state::<ExportTaskManager>();
        let result = baidu::export_photos(
            &database_path,
            &root,
            &strategy,
            |completed, total, file| {
                if let Some(snapshot) = manager.progress(&task_id, completed, total, file) {
                    emit_export_task(&app, &snapshot);
                }
            },
            || control.checkpoint(),
        );
        let snapshot = match result {
            Ok(summary) => manager.complete(&task_id, summary),
            Err(AppError::Cancelled) => manager.cancelled(&task_id),
            Err(error) => manager.fail(&task_id, error.to_string()),
        };
        if let Some(snapshot) = snapshot {
            emit_export_task(&app, &snapshot);
        }
    });
    Ok(task)
}

#[tauri::command]
pub fn list_export_tasks(tasks: State<'_, ExportTaskManager>) -> Vec<ExportTask> {
    tasks.list()
}

#[tauri::command]
pub fn pause_export_task(
    app: AppHandle,
    tasks: State<'_, ExportTaskManager>,
    task_id: String,
) -> Result<ExportTask, String> {
    let task = tasks.pause(&task_id).command()?;
    emit_export_task(&app, &task);
    Ok(task)
}

#[tauri::command]
pub fn resume_export_task(
    app: AppHandle,
    tasks: State<'_, ExportTaskManager>,
    task_id: String,
) -> Result<ExportTask, String> {
    let task = tasks.resume(&task_id).command()?;
    emit_export_task(&app, &task);
    Ok(task)
}

#[tauri::command]
pub fn cancel_export_task(
    app: AppHandle,
    tasks: State<'_, ExportTaskManager>,
    task_id: String,
) -> Result<ExportTask, String> {
    let task = tasks.cancel(&task_id).command()?;
    emit_export_task(&app, &task);
    Ok(task)
}

#[tauri::command]
pub fn clear_finished_export_tasks(tasks: State<'_, ExportTaskManager>) -> Vec<ExportTask> {
    tasks.clear_finished();
    tasks.list()
}
