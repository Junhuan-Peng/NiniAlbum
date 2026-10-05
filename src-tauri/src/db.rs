use std::{
    path::{Path, PathBuf},
    sync::RwLock,
};

use base64::{engine::general_purpose::STANDARD, Engine};
use rusqlite::{params, Connection};

use crate::{
    error::{AppError, AppResult},
    models::{
        DeleteImpact, PhotoEntry, PhotoQuery, ProjectCounts, ProjectInfo, ProjectSnapshot, TagNode,
    },
};

pub struct AppState {
    project_path: RwLock<Option<PathBuf>>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            project_path: RwLock::new(None),
        }
    }
}

impl AppState {
    pub fn set_project(&self, path: PathBuf) {
        *self
            .project_path
            .write()
            .expect("project path lock poisoned") = Some(path);
    }

    pub fn project_path(&self) -> AppResult<PathBuf> {
        self.project_path
            .read()
            .expect("project path lock poisoned")
            .clone()
            .ok_or(AppError::NoProject)
    }
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS project_settings (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  photo_root TEXT NOT NULL,
  sort_field TEXT NOT NULL DEFAULT 'file_name'
    CHECK (sort_field IN ('file_name','relative_path','modified_time','tagged_status','missing_status')),
  sort_direction TEXT NOT NULL DEFAULT 'asc' CHECK (sort_direction IN ('asc','desc')),
  schema_version INTEGER NOT NULL DEFAULT 1,
  created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE TABLE IF NOT EXISTS tag_tree (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  parent_id INTEGER NULL REFERENCES tag_tree(id) ON DELETE CASCADE,
  name TEXT NOT NULL CHECK (length(trim(name)) > 0),
  sort_order INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_tag_sibling_name
  ON tag_tree(parent_id, name COLLATE NOCASE) WHERE parent_id IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_tag_root_name
  ON tag_tree(name COLLATE NOCASE) WHERE parent_id IS NULL;
CREATE INDEX IF NOT EXISTS idx_tag_parent_sort ON tag_tree(parent_id, sort_order, name);
CREATE TABLE IF NOT EXISTS scan_runs (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  started_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  completed_at TEXT NULL,
  status TEXT NOT NULL DEFAULT 'running' CHECK (status IN ('running','completed','failed')),
  error_message TEXT NULL
);
CREATE TABLE IF NOT EXISTS photo_entries (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  relative_dir TEXT NOT NULL,
  pairing_key TEXT NOT NULL,
  display_base_name TEXT NOT NULL,
  pair_status TEXT NOT NULL DEFAULT 'complete'
    CHECK (pair_status IN ('complete','missing_jpeg','missing_raw','conflict')),
  is_missing INTEGER NOT NULL DEFAULT 0 CHECK (is_missing IN (0,1)),
  tag_id INTEGER NULL REFERENCES tag_tree(id) ON DELETE RESTRICT,
  thumbnail_blob BLOB NULL,
  thumbnail_mime TEXT NULL,
  created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  UNIQUE(relative_dir, pairing_key)
);
CREATE INDEX IF NOT EXISTS idx_photo_tag_id ON photo_entries(tag_id);
CREATE INDEX IF NOT EXISTS idx_photo_pair_status ON photo_entries(pair_status);
CREATE INDEX IF NOT EXISTS idx_photo_missing ON photo_entries(is_missing);
CREATE TABLE IF NOT EXISTS photo_files (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  photo_entry_id INTEGER NOT NULL REFERENCES photo_entries(id) ON DELETE CASCADE,
  file_kind TEXT NOT NULL CHECK (file_kind IN ('jpeg','arw')),
  original_file_name TEXT NOT NULL,
  relative_path TEXT NOT NULL UNIQUE,
  file_size INTEGER NOT NULL,
  modified_time REAL NOT NULL,
  quick_fingerprint TEXT NOT NULL,
  content_hash TEXT NULL,
  is_missing INTEGER NOT NULL DEFAULT 0 CHECK (is_missing IN (0,1)),
  last_seen_scan_id INTEGER NULL REFERENCES scan_runs(id),
  created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_file_entry_kind ON photo_files(photo_entry_id, file_kind);
CREATE INDEX IF NOT EXISTS idx_file_quick_fingerprint ON photo_files(file_size, quick_fingerprint);
CREATE INDEX IF NOT EXISTS idx_file_content_hash ON photo_files(content_hash);
CREATE INDEX IF NOT EXISTS idx_file_last_seen ON photo_files(last_seen_scan_id);
"#;

pub fn connect(path: &Path) -> AppResult<Connection> {
    let connection = Connection::open(path)?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "synchronous", "NORMAL")?;
    Ok(connection)
}

pub fn create_project(database_path: &Path, photo_root: &Path) -> AppResult<()> {
    if database_path.exists() {
        return Err(AppError::ProjectExists(database_path.to_path_buf()));
    }
    if !photo_root.is_dir() {
        return Err(AppError::InvalidPhotoRoot(photo_root.to_path_buf()));
    }
    if let Some(parent) = database_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let connection = connect(database_path)?;
    connection.execute_batch(SCHEMA)?;
    connection.execute(
        "INSERT INTO project_settings (id, photo_root) VALUES (1, ?1)",
        [photo_root.to_string_lossy().as_ref()],
    )?;
    Ok(())
}

pub fn open_project(database_path: &Path) -> AppResult<ProjectInfo> {
    if !database_path.is_file() {
        return Err(AppError::NotFound(database_path.display().to_string()));
    }
    let connection = connect(database_path)?;
    connection.execute_batch(SCHEMA)?;
    project_info(&connection, database_path)
}

pub fn project_info(connection: &Connection, database_path: &Path) -> AppResult<ProjectInfo> {
    connection
        .query_row(
            "SELECT photo_root, sort_field, sort_direction FROM project_settings WHERE id = 1",
            [],
            |row| {
                let photo_root: String = row.get(0)?;
                Ok(ProjectInfo {
                    database_path: database_path.to_string_lossy().into_owned(),
                    photo_root_available: Path::new(&photo_root).is_dir(),
                    photo_root,
                    sort_field: row.get(1)?,
                    sort_direction: row.get(2)?,
                })
            },
        )
        .map_err(AppError::from)
}

pub fn set_photo_root(connection: &Connection, photo_root: &Path) -> AppResult<()> {
    if !photo_root.is_dir() {
        return Err(AppError::InvalidPhotoRoot(photo_root.to_path_buf()));
    }
    connection.execute(
        "UPDATE project_settings SET photo_root=?1,updated_at=CURRENT_TIMESTAMP WHERE id=1",
        [photo_root.to_string_lossy().as_ref()],
    )?;
    Ok(())
}

pub fn validate_tag_name(name: &str) -> AppResult<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(AppError::EmptyTagName);
    }
    if trimmed.ends_with('.')
        || trimmed
            .chars()
            .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
    {
        return Err(AppError::InvalidTagName(trimmed.into()));
    }
    let upper = trimmed.to_ascii_uppercase();
    let reserved = matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (upper.len() == 4
            && (upper.starts_with("COM") || upper.starts_with("LPT"))
            && upper.as_bytes()[3].is_ascii_digit()
            && upper.as_bytes()[3] != b'0');
    if reserved {
        return Err(AppError::InvalidTagName(trimmed.into()));
    }
    Ok(trimmed.into())
}

pub fn create_tag(connection: &Connection, parent_id: Option<i64>, name: &str) -> AppResult<i64> {
    let name = validate_tag_name(name)?;
    if let Some(parent) = parent_id {
        let exists: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM tag_tree WHERE id=?1)",
            [parent],
            |r| r.get(0),
        )?;
        if !exists {
            return Err(AppError::NotFound(format!("父标签 {parent}")));
        }
    }
    let next: i64 = connection.query_row(
        "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM tag_tree WHERE parent_id IS ?1",
        [parent_id],
        |row| row.get(0),
    )?;
    connection.execute(
        "INSERT INTO tag_tree(parent_id,name,sort_order) VALUES(?1,?2,?3)",
        params![parent_id, name, next],
    )?;
    Ok(connection.last_insert_rowid())
}

pub fn rename_tag(connection: &Connection, id: i64, name: &str) -> AppResult<()> {
    let name = validate_tag_name(name)?;
    let changed = connection.execute(
        "UPDATE tag_tree SET name=?1, updated_at=CURRENT_TIMESTAMP WHERE id=?2",
        params![name, id],
    )?;
    if changed == 0 {
        return Err(AppError::NotFound(format!("标签 {id}")));
    }
    Ok(())
}

pub fn delete_impact(connection: &Connection, id: i64) -> AppResult<DeleteImpact> {
    connection.query_row(
        "WITH RECURSIVE subtree(id) AS (
           SELECT id FROM tag_tree WHERE id=?1
           UNION ALL SELECT t.id FROM tag_tree t JOIN subtree s ON t.parent_id=s.id
         ) SELECT COUNT(*), (SELECT COUNT(*) FROM photo_entries WHERE tag_id IN subtree) FROM subtree",
        [id], |row| Ok(DeleteImpact { tag_count: row.get(0)?, affected_photos: row.get(1)? }))
        .map_err(AppError::from)
}

pub fn delete_tag(connection: &mut Connection, id: i64, unassign: bool) -> AppResult<()> {
    let impact = delete_impact(connection, id)?;
    if impact.tag_count == 0 {
        return Err(AppError::NotFound(format!("标签 {id}")));
    }
    if impact.affected_photos > 0 && !unassign {
        return Err(AppError::TagInUse(impact.affected_photos));
    }
    let transaction = connection.transaction()?;
    if unassign {
        transaction.execute(
            "WITH RECURSIVE subtree(id) AS (
               SELECT id FROM tag_tree WHERE id=?1
               UNION ALL SELECT t.id FROM tag_tree t JOIN subtree s ON t.parent_id=s.id
             ) UPDATE photo_entries SET tag_id=NULL, updated_at=CURRENT_TIMESTAMP WHERE tag_id IN subtree", [id])?;
    }
    transaction.execute("DELETE FROM tag_tree WHERE id=?1", [id])?;
    transaction.commit()?;
    Ok(())
}

pub fn assign_tag(connection: &Connection, photo_id: i64, tag_id: Option<i64>) -> AppResult<()> {
    let changed = connection.execute(
        "UPDATE photo_entries SET tag_id=?1, updated_at=CURRENT_TIMESTAMP WHERE id=?2",
        params![tag_id, photo_id],
    )?;
    if changed == 0 {
        return Err(AppError::NotFound(format!("照片组 {photo_id}")));
    }
    Ok(())
}

pub fn list_tags(connection: &Connection) -> AppResult<Vec<TagNode>> {
    let mut statement = connection.prepare(
        "SELECT t.id,t.parent_id,t.name,t.sort_order,COUNT(p.id)
         FROM tag_tree t LEFT JOIN photo_entries p ON p.tag_id=t.id
         GROUP BY t.id ORDER BY t.parent_id, t.sort_order, t.name COLLATE NOCASE",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(TagNode {
            id: row.get(0)?,
            parent_id: row.get(1)?,
            name: row.get(2)?,
            sort_order: row.get(3)?,
            photo_count: row.get(4)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
}

fn order_clause(field: &str, direction: &str) -> &'static str {
    match (field, direction) {
        ("relative_path", "desc") => "e.relative_dir DESC, e.display_base_name DESC",
        ("relative_path", _) => "e.relative_dir ASC, e.display_base_name ASC",
        ("modified_time", "desc") => "modified_time DESC, e.display_base_name ASC",
        ("modified_time", _) => "modified_time ASC, e.display_base_name ASC",
        ("tagged_status", "desc") => "(e.tag_id IS NOT NULL) DESC, e.display_base_name ASC",
        ("tagged_status", _) => "(e.tag_id IS NOT NULL) ASC, e.display_base_name ASC",
        ("missing_status", "desc") => "e.is_missing DESC, e.display_base_name ASC",
        ("missing_status", _) => "e.is_missing ASC, e.display_base_name ASC",
        ("file_name", "desc") => "e.display_base_name COLLATE NOCASE DESC",
        _ => "e.display_base_name COLLATE NOCASE ASC",
    }
}

fn filter_clause(filter: &str) -> &'static str {
    match filter {
        "tagged" => "e.tag_id IS NOT NULL",
        "untagged" => "e.tag_id IS NULL",
        "complete" => "e.pair_status='complete' AND e.is_missing=0",
        "incomplete" => "e.pair_status IN ('missing_jpeg','missing_raw') AND e.is_missing=0",
        "conflict" => "e.pair_status='conflict' AND e.is_missing=0",
        "missing" => "e.is_missing=1",
        _ => "1=1",
    }
}

pub fn list_photos(connection: &Connection, query: &PhotoQuery) -> AppResult<Vec<PhotoEntry>> {
    let settings: (String, String) = connection.query_row(
        "SELECT sort_field,sort_direction FROM project_settings WHERE id=1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let field = query.sort_field.as_deref().unwrap_or(&settings.0);
    let direction = query.sort_direction.as_deref().unwrap_or(&settings.1);
    let filter = filter_clause(query.filter.as_deref().unwrap_or("all"));
    let sql = format!(
        r#"
      WITH RECURSIVE paths(id,path) AS (
        SELECT id,name FROM tag_tree WHERE parent_id IS NULL
        UNION ALL SELECT t.id, paths.path || ' > ' || t.name FROM tag_tree t JOIN paths ON t.parent_id=paths.id
      )
      SELECT e.id,e.relative_dir,e.display_base_name,e.pair_status,e.is_missing,e.tag_id,paths.path,
        (SELECT relative_path FROM photo_files WHERE photo_entry_id=e.id AND file_kind='jpeg' AND is_missing=0 ORDER BY id LIMIT 1),
        (SELECT relative_path FROM photo_files WHERE photo_entry_id=e.id AND file_kind='arw' AND is_missing=0 ORDER BY id LIMIT 1),
        (SELECT MAX(modified_time) FROM photo_files WHERE photo_entry_id=e.id AND is_missing=0) AS modified_time,
        e.thumbnail_blob,e.thumbnail_mime
      FROM photo_entries e LEFT JOIN paths ON paths.id=e.tag_id
      WHERE {filter} ORDER BY {}"#,
        order_clause(field, direction)
    );
    let mut statement = connection.prepare(&sql)?;
    let rows = statement.query_map([], |row| {
        let blob: Option<Vec<u8>> = row.get(10)?;
        let mime: Option<String> = row.get(11)?;
        let thumbnail_data_url = blob.map(|bytes| {
            format!(
                "data:{};base64,{}",
                mime.as_deref().unwrap_or("image/jpeg"),
                STANDARD.encode(bytes)
            )
        });
        Ok(PhotoEntry {
            id: row.get(0)?,
            relative_dir: row.get(1)?,
            display_base_name: row.get(2)?,
            pair_status: row.get(3)?,
            is_missing: row.get::<_, i64>(4)? != 0,
            tag_id: row.get(5)?,
            tag_path: row.get(6)?,
            jpeg_relative_path: row.get(7)?,
            raw_relative_path: row.get(8)?,
            modified_time: row.get(9)?,
            thumbnail_data_url,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
}

pub fn counts(connection: &Connection) -> AppResult<ProjectCounts> {
    connection.query_row(
        "SELECT COUNT(*), SUM(tag_id IS NOT NULL), SUM(is_missing=1),
         SUM(pair_status='conflict'), SUM(pair_status IN ('missing_jpeg','missing_raw')) FROM photo_entries", [],
        |r| Ok(ProjectCounts {
            total: r.get(0)?, tagged: r.get::<_, Option<i64>>(1)?.unwrap_or(0),
            missing: r.get::<_, Option<i64>>(2)?.unwrap_or(0), conflicts: r.get::<_, Option<i64>>(3)?.unwrap_or(0),
            incomplete: r.get::<_, Option<i64>>(4)?.unwrap_or(0),
        })) .map_err(AppError::from)
}

pub fn snapshot(database_path: &Path, query: &PhotoQuery) -> AppResult<ProjectSnapshot> {
    let connection = connect(database_path)?;
    Ok(ProjectSnapshot {
        project: project_info(&connection, database_path)?,
        tags: list_tags(&connection)?,
        photos: list_photos(&connection, query)?,
        counts: counts(&connection)?,
    })
}

pub fn set_sort(connection: &Connection, field: &str, direction: &str) -> AppResult<()> {
    let valid_field = matches!(
        field,
        "file_name" | "relative_path" | "modified_time" | "tagged_status" | "missing_status"
    );
    let valid_direction = matches!(direction, "asc" | "desc");
    if !valid_field || !valid_direction {
        return Err(AppError::InvalidInput("无效排序选项".into()));
    }
    connection.execute("UPDATE project_settings SET sort_field=?1,sort_direction=?2,updated_at=CURRENT_TIMESTAMP WHERE id=1", params![field,direction])?;
    Ok(())
}

pub fn delete_missing(connection: &mut Connection, ids: &[i64]) -> AppResult<usize> {
    let transaction = connection.transaction()?;
    let mut removed = 0;
    for id in ids {
        removed += transaction.execute(
            "DELETE FROM photo_entries WHERE id=?1 AND is_missing=1",
            [id],
        )?;
    }
    transaction.commit()?;
    Ok(removed)
}

pub fn checkpoint(connection: &Connection) -> AppResult<()> {
    connection.execute_batch("PRAGMA wal_checkpoint(FULL);")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_names_reject_unsafe_and_reserved_values() {
        assert!(validate_tag_name("  风景  ").is_ok());
        assert!(validate_tag_name("A/B").is_err());
        assert!(validate_tag_name("CON").is_err());
        assert!(validate_tag_name("LPT1").is_err());
        assert!(validate_tag_name("name.").is_err());
    }

    #[test]
    fn delete_used_subtree_requires_explicit_unassign() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .unwrap();
        connection.execute_batch(SCHEMA).unwrap();
        connection
            .execute(
                "INSERT INTO project_settings(id,photo_root) VALUES(1,'/tmp')",
                [],
            )
            .unwrap();
        let root = create_tag(&connection, None, "root").unwrap();
        let child = create_tag(&connection, Some(root), "child").unwrap();
        connection.execute("INSERT INTO photo_entries(relative_dir,pairing_key,display_base_name,tag_id) VALUES('','a','A',?1)", [child]).unwrap();
        assert!(matches!(
            delete_tag(&mut connection, root, false),
            Err(AppError::TagInUse(1))
        ));
        delete_tag(&mut connection, root, true).unwrap();
        let tag: Option<i64> = connection
            .query_row("SELECT tag_id FROM photo_entries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(tag, None);
    }
}
