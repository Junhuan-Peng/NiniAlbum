use std::{
    fs,
    path::{Path, PathBuf},
};

use filetime::{set_file_mtime, FileTime};
use rusqlite::Connection;

use crate::{
    db,
    error::{AppError, AppResult},
    models::{ExportFailure, ExportPlan, ExportSummary},
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
    tag_path: String,
    jpeg: SourceFile,
    raw: SourceFile,
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
         JOIN paths ON paths.id=e.tag_id
         WHERE e.is_missing=0 AND e.pair_status='complete' AND e.is_trashed=0 ORDER BY e.id")?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
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

fn target_directory(root: &Path, tag_path: &str) -> PathBuf {
    tag_path
        .split('>')
        .fold(root.to_path_buf(), |path, component| {
            path.join(safe_component(component))
        })
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

fn copy_pair(root: &Path, destination: &(PathBuf, PathBuf), item: &ExportItem) -> AppResult<()> {
    let jpeg_source = root.join(&item.jpeg.relative_path);
    let raw_source = root.join(&item.raw.relative_path);
    fs::copy(&jpeg_source, &destination.0)?;
    if let Err(error) = fs::copy(&raw_source, &destination.1) {
        let _ = fs::remove_file(&destination.0);
        return Err(error.into());
    }
    restore_mtime(&destination.0, item.jpeg.modified_time)?;
    restore_mtime(&destination.1, item.raw.modified_time)?;
    Ok(())
}

fn skip_counts(connection: &Connection) -> AppResult<(usize, usize, usize)> {
    connection
        .query_row(
            "SELECT SUM(tag_id IS NULL AND is_trashed=0),
         SUM(tag_id IS NOT NULL AND pair_status!='complete' AND is_missing=0 AND is_trashed=0),
         SUM(tag_id IS NOT NULL AND is_missing=1 AND is_trashed=0) FROM photo_entries",
            [],
            |row| {
                Ok((
                    row.get::<_, Option<i64>>(0)?.unwrap_or(0) as usize,
                    row.get::<_, Option<i64>>(1)?.unwrap_or(0) as usize,
                    row.get::<_, Option<i64>>(2)?.unwrap_or(0) as usize,
                ))
            },
        )
        .map_err(AppError::from)
}

pub fn plan_export(database_path: &Path, export_root: &Path) -> AppResult<ExportPlan> {
    let connection = db::connect(database_path)?;
    let items = export_items(&connection)?;
    let (untagged, incomplete, missing) = skip_counts(&connection)?;
    let conflicts = items
        .iter()
        .filter(|item| {
            has_conflict(&original_targets(
                &target_directory(export_root, &item.tag_path),
                item,
            ))
        })
        .count();
    Ok(ExportPlan {
        eligible: items.len(),
        conflicts,
        skipped_untagged: untagged,
        skipped_incomplete: incomplete,
        skipped_missing: missing,
    })
}

pub fn export_photos(
    database_path: &Path,
    export_root: &Path,
    strategy: &str,
) -> AppResult<ExportSummary> {
    if export_root.exists() && !export_root.is_dir() {
        return Err(AppError::InvalidInput(format!(
            "导出目标不是目录：{}",
            export_root.display()
        )));
    }
    fs::create_dir_all(export_root)?;
    let strategy = ConflictStrategy::parse(strategy)?;
    let connection = db::connect(database_path)?;
    let root = PathBuf::from(connection.query_row::<String, _, _>(
        "SELECT photo_root FROM project_settings WHERE id=1",
        [],
        |row| row.get(0),
    )?);
    let items = export_items(&connection)?;
    let (untagged, incomplete, missing) = skip_counts(&connection)?;
    let mut summary = ExportSummary {
        skipped_untagged: untagged,
        skipped_incomplete: incomplete,
        skipped_missing: missing,
        ..Default::default()
    };
    for item in items {
        let directory = target_directory(export_root, &item.tag_path);
        if let Err(error) = fs::create_dir_all(&directory) {
            summary.failed += 1;
            summary.failures.push(ExportFailure {
                photo_id: item.id,
                name: item.base_name.clone(),
                reason: error.to_string(),
            });
            continue;
        }
        let original = original_targets(&directory, &item);
        let conflict = has_conflict(&original);
        if conflict && strategy == ConflictStrategy::Skip {
            summary.skipped += 1;
            continue;
        }
        let destination = if conflict && strategy == ConflictStrategy::Rename {
            summary.renamed += 1;
            renamed_targets(&directory, &item)
        } else {
            original
        };
        match copy_pair(&root, &destination, &item) {
            Ok(()) => summary.succeeded += 1,
            Err(error) => {
                summary.failed += 1;
                summary.failures.push(ExportFailure {
                    photo_id: item.id,
                    name: item.base_name.clone(),
                    reason: error.to_string(),
                });
            }
        }
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
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
            tag_path: "x".into(),
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
}
