use std::path::PathBuf;

use tauri::{AppHandle, Emitter, State};

use crate::{
    db::{self, AppState},
    error::CommandResult,
    export,
    models::{
        DeleteImpact, ExportPlan, ExportSummary, PhotoQuery, ProjectInfo, ProjectSnapshot,
        ScanSummary,
    },
    scan,
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
    let connection = db::connect(&state.project_path().command()?).command()?;
    db::create_tag(&connection, parent_id, &name).command()
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

#[tauri::command]
pub async fn export_photos(
    state: State<'_, AppState>,
    export_root: String,
    strategy: String,
) -> Result<ExportSummary, String> {
    let database_path = state.project_path().command()?;
    tauri::async_runtime::spawn_blocking(move || {
        export::export_photos(&database_path, &PathBuf::from(export_root), &strategy).command()
    })
    .await
    .map_err(|error| error.to_string())?
}
