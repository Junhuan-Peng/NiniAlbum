use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use filetime::{set_file_mtime, FileTime};
use rusqlite::Connection;

use crate::{
    db,
    error::{AppError, AppResult},
    models::{ExportFailure, ExportPlan, ExportPreviewItem, ExportSummary},
};

#[derive(Debug, Clone)]
struct SourceFile {
    relative_path: String,
    original_name: String,
    modified_time: f64,
}

#[derive(Debug, Clone)]
struct ExportItem {
    id: i64,
    base_name: String,
    tag_path: Option<String>,
    jpeg: SourceFile,
    raw: SourceFile,
}

#[derive(Debug, Clone)]
pub(crate) struct ExportFile {
    pub source: PathBuf,
    pub name: String,
    pub modified_time: f64,
}

#[derive(Debug, Clone)]
pub(crate) struct ExportManifestItem {
    pub photo_id: i64,
    pub display_name: String,
    pub tag_components: Vec<String>,
    pub files: [ExportFile; 2],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictStrategy {
    Skip,
    Overwrite,
    Rename,
}

impl ConflictStrategy {
    fn parse(value: &str) -> AppResult<Self> {
        match value {
            "skip" => Ok(Self::Skip),
            "overwrite" => Ok(Self::Overwrite),
            "rename" => Ok(Self::Rename),
            _ => Err(AppError::InvalidInput(format!("未知导出冲突策略：{value}"))),
        }
    }
}

fn source_file(connection: &Connection, entry_id: i64, kind: &str) -> AppResult<SourceFile> {
    connection
        .query_row(
            "SELECT relative_path,original_file_name,modified_time FROM photo_files
         WHERE photo_entry_id=?1 AND file_kind=?2 AND is_missing=0 ORDER BY id LIMIT 1",
            rusqlite::params![entry_id, kind],
            |row| {
                Ok(SourceFile {
                    relative_path: row.get(0)?,
                    original_name: row.get(1)?,
                    modified_time: row.get(2)?,
                })
            },
        )
        .map_err(AppError::from)
}

fn export_items(connection: &Connection) -> AppResult<Vec<ExportItem>> {
    let mut statement = connection.prepare(
        "WITH RECURSIVE paths(id,path) AS (
           SELECT id,name FROM tag_tree WHERE parent_id IS NULL
           UNION ALL SELECT t.id,paths.path || '>' || t.name FROM tag_tree t JOIN paths ON t.parent_id=paths.id
         )
         SELECT e.id,e.display_base_name,paths.path FROM photo_entries e
         LEFT JOIN paths ON paths.id=e.tag_id
         WHERE e.is_missing=0 AND e.pair_status='complete'
           AND e.is_trashed=0 AND e.is_similarity_hidden=0 ORDER BY e.id")?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
        ))
    })?;
    let mut items = Vec::new();
    for row in rows {
        let (id, base_name, tag_path) = row?;
        items.push(ExportItem {
            id,
            base_name,
            tag_path,
            jpeg: source_file(connection, id, "jpeg")?,
            raw: source_file(connection, id, "arw")?,
        });
    }
    Ok(items)
}

fn safe_component(component: &str) -> String {
    let mut value: String = component
        .chars()
        .map(|character| {
            if character.is_control() || "<>:\"/\\|?*".contains(character) {
                '_'
            } else {
                character
            }
        })
        .collect();
    while value.ends_with([' ', '.']) {
        value.pop();
    }
    if value.is_empty() {
        value.push('_');
    }
    let upper = value.to_ascii_uppercase();
    let reserved = matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (upper.len() == 4
            && (upper.starts_with("COM") || upper.starts_with("LPT"))
            && upper.as_bytes()[3].is_ascii_digit()
            && upper.as_bytes()[3] != b'0');
    if reserved {
        value.insert(0, '_');
    }
    value
}

fn target_directory(root: &Path, tag_path: Option<&str>) -> PathBuf {
    tag_path.map_or_else(
        || root.to_path_buf(),
        |path| {
            path.split('>')
                .fold(root.to_path_buf(), |directory, component| {
                    directory.join(safe_component(component))
                })
        },
    )
}

fn original_targets(directory: &Path, item: &ExportItem) -> (PathBuf, PathBuf) {
    (
        directory.join(&item.jpeg.original_name),
        directory.join(&item.raw.original_name),
    )
}

fn extension(name: &str, fallback: &str) -> String {
    Path::new(name)
        .extension()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| fallback.into())
}

fn renamed_targets(directory: &Path, item: &ExportItem) -> (PathBuf, PathBuf) {
    let jpeg_ext = extension(&item.jpeg.original_name, "jpg");
    let raw_ext = extension(&item.raw.original_name, "arw");
    for index in 1_u64.. {
        let base = format!("{}_{}", item.base_name, index);
        let jpeg = directory.join(format!("{base}.{jpeg_ext}"));
        let raw = directory.join(format!("{base}.{raw_ext}"));
        if !jpeg.exists() && !raw.exists() {
            return (jpeg, raw);
        }
    }
    unreachable!("rename suffix iterator is unbounded")
}

fn has_conflict(targets: &(PathBuf, PathBuf)) -> bool {
    targets.0.exists() || targets.1.exists()
}

fn restore_mtime(path: &Path, seconds: f64) -> AppResult<()> {
    let whole = seconds.trunc() as i64;
    let nanos = ((seconds.fract().max(0.0)) * 1_000_000_000.0) as u32;
    set_file_mtime(path, FileTime::from_unix_time(whole, nanos))?;
    Ok(())
}

fn copy_pair_controlled<P, C>(
    root: &Path,
    destination: &(PathBuf, PathBuf),
    item: &ExportItem,
    completed: &mut usize,
    total: usize,
    progress: &mut P,
    checkpoint: &mut C,
) -> AppResult<()>
where
    P: FnMut(usize, usize, &str),
    C: FnMut() -> AppResult<()>,
{
    let jpeg_source = root.join(&item.jpeg.relative_path);
    let raw_source = root.join(&item.raw.relative_path);
    checkpoint()?;
    progress(*completed, total, &item.jpeg.original_name);
    fs::copy(&jpeg_source, &destination.0)?;
    *completed += 1;
    checkpoint().inspect_err(|_| {
        let _ = fs::remove_file(&destination.0);
    })?;
    progress(*completed, total, &item.raw.original_name);
    if let Err(error) = fs::copy(&raw_source, &destination.1) {
        let _ = fs::remove_file(&destination.0);
        return Err(error.into());
    }
    *completed += 1;
    restore_mtime(&destination.0, item.jpeg.modified_time)?;
    restore_mtime(&destination.1, item.raw.modified_time)?;
    Ok(())
}

#[derive(Debug, Default)]
struct ExportCounts {
    untagged: usize,
    incomplete: usize,
    missing: usize,
    hidden: usize,
    trashed: usize,
}

fn export_counts(connection: &Connection) -> AppResult<ExportCounts> {
    connection
        .query_row(
            "SELECT
         SUM(tag_id IS NULL AND pair_status='complete' AND is_missing=0 AND is_trashed=0 AND is_similarity_hidden=0),
         SUM(pair_status!='complete' AND is_missing=0 AND is_trashed=0 AND is_similarity_hidden=0),
         SUM(is_missing=1 AND is_trashed=0 AND is_similarity_hidden=0),
         SUM(is_similarity_hidden=1 AND is_trashed=0),
         SUM(is_trashed=1)
         FROM photo_entries",
            [],
            |row| {
                Ok(ExportCounts {
                    untagged: row.get::<_, Option<i64>>(0)?.unwrap_or(0) as usize,
                    incomplete: row.get::<_, Option<i64>>(1)?.unwrap_or(0) as usize,
                    missing: row.get::<_, Option<i64>>(2)?.unwrap_or(0) as usize,
                    hidden: row.get::<_, Option<i64>>(3)?.unwrap_or(0) as usize,
                    trashed: row.get::<_, Option<i64>>(4)?.unwrap_or(0) as usize,
                })
            },
        )
        .map_err(AppError::from)
}

fn export_preview_items(
    connection: &Connection,
    export_root: &Path,
    eligible: &[ExportItem],
) -> AppResult<Vec<ExportPreviewItem>> {
    let eligible_by_id = eligible
        .iter()
        .map(|item| (item.id, item))
        .collect::<BTreeMap<_, _>>();
    let mut statement = connection.prepare(
        "WITH RECURSIVE paths(id,path) AS (
           SELECT id,name FROM tag_tree WHERE parent_id IS NULL
           UNION ALL SELECT t.id,paths.path || '>' || t.name FROM tag_tree t JOIN paths ON t.parent_id=paths.id
         )
         SELECT e.id,e.display_base_name,paths.path,e.pair_status,e.is_missing,e.is_trashed
         FROM photo_entries e LEFT JOIN paths ON paths.id=e.tag_id ORDER BY e.id",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, i64>(4)? != 0,
            row.get::<_, i64>(5)? != 0,
        ))
    })?;
    let mut preview = Vec::new();
    for row in rows {
        let (id, name, tag_path, pair_status, missing, trashed) = row?;
        let (status, detail) = if trashed {
            ("trashed", "位于回收站，不导出".into())
        } else if missing {
            ("missing", "源文件丢失".into())
        } else if pair_status != "complete" {
            ("incomplete", format!("配对状态：{pair_status}"))
        } else if let Some(item) = eligible_by_id.get(&id) {
            let directory = target_directory(export_root, item.tag_path.as_deref());
            let targets = original_targets(&directory, item);
            let conflict = has_conflict(&targets);
            let relative = item.tag_path.as_deref().map_or_else(
                || {
                    format!(
                        "根目录 / {} + {}",
                        item.jpeg.original_name, item.raw.original_name
                    )
                },
                |path| {
                    format!(
                        "{} / {} + {}",
                        path.replace('>', " / "),
                        item.jpeg.original_name,
                        item.raw.original_name
                    )
                },
            );
            let status = if conflict {
                "conflict"
            } else if item.tag_path.is_none() {
                "untagged"
            } else {
                "ready"
            };
            (status, relative)
        } else {
            ("incomplete", "缺少可导出的 JPEG/ARW 文件".into())
        };
        preview.push(ExportPreviewItem {
            photo_id: id,
            name,
            tag_path,
            status: status.into(),
            detail,
        });
    }
    Ok(preview)
}

pub(crate) fn manifest(database_path: &Path) -> AppResult<Vec<ExportManifestItem>> {
    let connection = db::connect(database_path)?;
    let source_root = PathBuf::from(connection.query_row::<String, _, _>(
        "SELECT photo_root FROM project_settings WHERE id=1",
        [],
        |row| row.get(0),
    )?);
    export_items(&connection).map(|items| {
        items
            .into_iter()
            .map(|item| ExportManifestItem {
                photo_id: item.id,
                display_name: item.base_name,
                tag_components: item
                    .tag_path
                    .as_deref()
                    .map(|path| path.split('>').map(safe_component).collect())
                    .unwrap_or_default(),
                files: [
                    ExportFile {
                        source: source_root.join(item.jpeg.relative_path),
                        name: item.jpeg.original_name,
                        modified_time: item.jpeg.modified_time,
                    },
                    ExportFile {
                        source: source_root.join(item.raw.relative_path),
                        name: item.raw.original_name,
                        modified_time: item.raw.modified_time,
                    },
                ],
            })
            .collect()
    })
}

pub(crate) fn content_plan(database_path: &Path) -> AppResult<ExportPlan> {
    let connection = db::connect(database_path)?;
    let items = export_items(&connection)?;
    let counts = export_counts(&connection)?;
    let mut occupied = std::collections::HashSet::new();
    let conflicts = items
        .iter()
        .filter(|item| {
            let directory = item.tag_path.as_deref().unwrap_or("").to_lowercase();
            let keys = [
                format!("{directory}/{}", item.jpeg.original_name.to_lowercase()),
                format!("{directory}/{}", item.raw.original_name.to_lowercase()),
            ];
            let conflict = keys.iter().any(|key| occupied.contains(key));
            occupied.extend(keys);
            conflict
        })
        .count();
    Ok(ExportPlan {
        eligible: items.len(),
        conflicts,
        untagged: counts.untagged,
        skipped_incomplete: counts.incomplete,
        skipped_missing: counts.missing,
        skipped_hidden: counts.hidden,
        skipped_trashed: counts.trashed,
        items: Vec::new(),
    })
}

fn validate_empty_directory(export_root: &Path) -> AppResult<()> {
    if !export_root.is_dir() {
        return Err(AppError::InvalidInput(format!(
            "请选择一个已存在的空目录：{}",
            export_root.display()
        )));
    }
    if fs::read_dir(export_root)?.next().transpose()?.is_some() {
        return Err(AppError::InvalidInput(format!(
            "导出根目录必须为空：{}",
            export_root.display()
        )));
    }
    Ok(())
}

pub fn plan_export(database_path: &Path, export_root: &Path) -> AppResult<ExportPlan> {
    validate_empty_directory(export_root)?;
    let connection = db::connect(database_path)?;
    let items = export_items(&connection)?;
    let mut plan = content_plan(database_path)?;
    plan.items = export_preview_items(&connection, export_root, &items)?;
    Ok(plan)
}

#[cfg(test)]
pub fn export_photos(
    database_path: &Path,
    export_root: &Path,
    strategy: &str,
) -> AppResult<ExportSummary> {
    export_photos_controlled(
        database_path,
        export_root,
        strategy,
        |_, _, _| {},
        || Ok(()),
    )
}

pub fn export_photos_controlled<P, C>(
    database_path: &Path,
    export_root: &Path,
    strategy: &str,
    mut progress: P,
    mut checkpoint: C,
) -> AppResult<ExportSummary>
where
    P: FnMut(usize, usize, &str),
    C: FnMut() -> AppResult<()>,
{
    validate_empty_directory(export_root)?;
    let strategy = ConflictStrategy::parse(strategy)?;
    let connection = db::connect(database_path)?;
    let root = PathBuf::from(connection.query_row::<String, _, _>(
        "SELECT photo_root FROM project_settings WHERE id=1",
        [],
        |row| row.get(0),
    )?);
    let items = export_items(&connection)?;
    let total_files = items.len() * 2;
    let mut completed_files = 0;
    progress(0, total_files, "");
    let counts = export_counts(&connection)?;
    let mut summary = ExportSummary {
        untagged: counts.untagged,
        skipped_incomplete: counts.incomplete,
        skipped_missing: counts.missing,
        skipped_hidden: counts.hidden,
        skipped_trashed: counts.trashed,
        ..Default::default()
    };
    for item in items {
        checkpoint()?;
        let directory = target_directory(export_root, item.tag_path.as_deref());
        if let Err(error) = fs::create_dir_all(&directory) {
            summary.failed += 1;
            summary.failures.push(ExportFailure {
                photo_id: item.id,
                name: item.base_name.clone(),
                reason: error.to_string(),
            });
            completed_files += 2;
            progress(completed_files, total_files, "");
            continue;
        }
        let original = original_targets(&directory, &item);
        let conflict = has_conflict(&original);
        if conflict && strategy == ConflictStrategy::Skip {
            summary.skipped += 1;
            completed_files += 2;
            progress(completed_files, total_files, "");
            continue;
        }
        let destination = if conflict && strategy == ConflictStrategy::Rename {
            summary.renamed += 1;
            renamed_targets(&directory, &item)
        } else {
            original
        };
        match copy_pair_controlled(
            &root,
            &destination,
            &item,
            &mut completed_files,
            total_files,
            &mut progress,
            &mut checkpoint,
        ) {
            Ok(()) => summary.succeeded += 1,
            Err(AppError::Cancelled) => return Err(AppError::Cancelled),
            Err(error) => {
                summary.failed += 1;
                completed_files = ((completed_files / 2 + 1) * 2).min(total_files);
                summary.failures.push(ExportFailure {
                    photo_id: item.id,
                    name: item.base_name.clone(),
                    reason: error.to_string(),
                });
            }
        }
        progress(completed_files, total_files, "");
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, fs::File, io::Write};

    use super::*;

    #[test]
    fn sanitizes_cross_platform_components() {
        assert_eq!(safe_component("tea/sea"), "tea_sea");
        assert_eq!(safe_component("name. "), "name");
        assert_eq!(safe_component("CON"), "_CON");
    }

    #[test]
    fn paired_rename_uses_the_same_suffix() {
        let directory = tempfile::tempdir().unwrap();
        let item = ExportItem {
            id: 1,
            base_name: "DSC0001".into(),
            tag_path: Some("x".into()),
            jpeg: SourceFile {
                relative_path: "a".into(),
                original_name: "DSC0001.JPG".into(),
                modified_time: 0.0,
            },
            raw: SourceFile {
                relative_path: "b".into(),
                original_name: "DSC0001.ARW".into(),
                modified_time: 0.0,
            },
        };
        let (jpeg, raw) = renamed_targets(directory.path(), &item);
        assert_eq!(jpeg.file_name().unwrap(), "DSC0001_1.JPG");
        assert_eq!(raw.file_name().unwrap(), "DSC0001_1.ARW");
    }

    #[test]
    fn exports_untagged_and_excludes_hidden_and_trashed_photos() {
        let workspace = tempfile::tempdir().unwrap();
        let photo_root = workspace.path().join("photos");
        std::fs::create_dir(&photo_root).unwrap();
        for name in ["VISIBLE", "HIDDEN", "TRASHED"] {
            image::RgbImage::from_pixel(4, 4, image::Rgb([10, 20, 30]))
                .save(photo_root.join(format!("{name}.JPG")))
                .unwrap();
            File::create(photo_root.join(format!("{name}.ARW")))
                .unwrap()
                .write_all(name.as_bytes())
                .unwrap();
        }
        let database = workspace.path().join("project.sqlite");
        db::create_project(&database, &photo_root).unwrap();
        crate::scan::scan_project(&database).unwrap();
        let connection = db::connect(&database).unwrap();
        connection
            .execute(
                "UPDATE photo_entries SET is_similarity_hidden=1 WHERE display_base_name='HIDDEN'",
                [],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE photo_entries SET is_trashed=1 WHERE display_base_name='TRASHED'",
                [],
            )
            .unwrap();
        drop(connection);

        let export_root = workspace.path().join("export");
        std::fs::create_dir(&export_root).unwrap();
        let plan = plan_export(&database, &export_root).unwrap();
        assert_eq!(plan.eligible, 1);
        assert_eq!(plan.untagged, 1);
        assert_eq!(plan.skipped_hidden, 1);
        assert_eq!(plan.skipped_trashed, 1);

        let summary = export_photos(&database, &export_root, "skip").unwrap();
        assert_eq!(summary.succeeded, 1);
        assert!(export_root.join("VISIBLE.JPG").is_file());
        assert!(export_root.join("VISIBLE.ARW").is_file());
        assert!(!export_root.join("HIDDEN.JPG").exists());
        assert!(!export_root.join("TRASHED.JPG").exists());
    }

    #[test]
    fn rejects_a_nonempty_export_root() {
        let directory = tempfile::tempdir().unwrap();
        File::create(directory.path().join("existing.txt")).unwrap();
        assert!(validate_empty_directory(directory.path()).is_err());
    }

    #[test]
    fn cancellation_between_pair_files_removes_the_partial_copy() {
        let workspace = tempfile::tempdir().unwrap();
        let photo_root = workspace.path().join("photos");
        std::fs::create_dir(&photo_root).unwrap();
        image::RgbImage::from_pixel(4, 4, image::Rgb([10, 20, 30]))
            .save(photo_root.join("PAIR.JPG"))
            .unwrap();
        File::create(photo_root.join("PAIR.ARW"))
            .unwrap()
            .write_all(b"raw")
            .unwrap();
        let database = workspace.path().join("project.sqlite");
        db::create_project(&database, &photo_root).unwrap();
        crate::scan::scan_project(&database).unwrap();
        let export_root = workspace.path().join("export");
        std::fs::create_dir(&export_root).unwrap();
        let checkpoints = Cell::new(0);

        let result = export_photos_controlled(
            &database,
            &export_root,
            "skip",
            |_, _, _| {},
            || {
                let current = checkpoints.get();
                checkpoints.set(current + 1);
                if current == 2 {
                    Err(AppError::Cancelled)
                } else {
                    Ok(())
                }
            },
        );

        assert!(matches!(result, Err(AppError::Cancelled)));
        assert!(!export_root.join("PAIR.JPG").exists());
        assert!(!export_root.join("PAIR.ARW").exists());
    }
}
