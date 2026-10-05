use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use image::{codecs::jpeg::JpegEncoder, ImageReader};
use rusqlite::{params, OptionalExtension, Transaction};
use walkdir::WalkDir;

use crate::{
    db,
    error::{AppError, AppResult},
    models::ScanSummary,
};

const FINGERPRINT_CHUNK: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
enum FileKind {
    Jpeg,
    Arw,
}

impl FileKind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Jpeg => "jpeg",
            Self::Arw => "arw",
        }
    }
}

#[derive(Debug, Clone)]
struct ScannedFile {
    absolute_path: PathBuf,
    relative_path: String,
    original_file_name: String,
    kind: FileKind,
    size: u64,
    modified_time: f64,
    quick_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct GroupKey {
    relative_dir: String,
    pairing_key: String,
}

#[derive(Debug)]
struct ScannedGroup {
    key: GroupKey,
    display_base_name: String,
    files: Vec<ScannedFile>,
}

pub fn normalize_pairing_key(stem: &str) -> String {
    stem.to_lowercase()
}

fn classify_group(files: &[ScannedFile]) -> &'static str {
    let jpeg = files
        .iter()
        .filter(|file| file.kind == FileKind::Jpeg)
        .count();
    let arw = files
        .iter()
        .filter(|file| file.kind == FileKind::Arw)
        .count();
    match (jpeg, arw) {
        (1, 1) => "complete",
        (1, 0) => "missing_raw",
        (0, 1) => "missing_jpeg",
        _ => "conflict",
    }
}

fn file_kind(path: &Path) -> Option<FileKind> {
    match path
        .extension()?
        .to_string_lossy()
        .to_ascii_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" => Some(FileKind::Jpeg),
        "arw" => Some(FileKind::Arw),
        _ => None,
    }
}

fn modified_time(metadata: &std::fs::Metadata) -> f64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs_f64())
        .unwrap_or_default()
}

pub fn quick_fingerprint(path: &Path, size: u64) -> AppResult<String> {
    let mut file = File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(&size.to_le_bytes());
    let mut first = vec![0_u8; FINGERPRINT_CHUNK.min(size as usize)];
    file.read_exact(&mut first)?;
    hasher.update(&first);
    if size > FINGERPRINT_CHUNK as u64 {
        let tail_len = FINGERPRINT_CHUNK.min(size as usize);
        file.seek(SeekFrom::End(-(tail_len as i64)))?;
        let mut last = vec![0_u8; tail_len];
        file.read_exact(&mut last)?;
        hasher.update(&last);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

pub fn content_hash(path: &Path) -> AppResult<String> {
    let mut file = File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

fn collect_groups(root: &Path) -> AppResult<(Vec<ScannedGroup>, Vec<String>, usize)> {
    let mut groups: BTreeMap<GroupKey, ScannedGroup> = BTreeMap::new();
    let mut warnings = Vec::new();
    let mut discovered = 0;
    for item in WalkDir::new(root).follow_links(false) {
        let entry = match item {
            Ok(entry) => entry,
            Err(error) => {
                warnings.push(error.to_string());
                continue;
            }
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let Some(kind) = file_kind(entry.path()) else {
            continue;
        };
        let relative = entry
            .path()
            .strip_prefix(root)
            .map_err(|_| AppError::InvalidRelativePath(entry.path().to_path_buf()))?;
        let Some(stem) = entry
            .path()
            .file_stem()
            .map(|value| value.to_string_lossy().into_owned())
        else {
            continue;
        };
        let relative_dir = relative.parent().unwrap_or_else(|| Path::new(""));
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(error) => {
                warnings.push(format!("{}: {error}", entry.path().display()));
                continue;
            }
        };
        let fingerprint = match quick_fingerprint(entry.path(), metadata.len()) {
            Ok(value) => value,
            Err(error) => {
                warnings.push(format!("{}: {error}", entry.path().display()));
                continue;
            }
        };
        let key = GroupKey {
            relative_dir: relative_dir.to_string_lossy().into_owned(),
            pairing_key: normalize_pairing_key(&stem),
        };
        let file = ScannedFile {
            absolute_path: entry.path().to_path_buf(),
            relative_path: relative.to_string_lossy().into_owned(),
            original_file_name: entry.file_name().to_string_lossy().into_owned(),
            kind,
            size: metadata.len(),
            modified_time: modified_time(&metadata),
            quick_fingerprint: fingerprint,
        };
        groups
            .entry(key.clone())
            .or_insert_with(|| ScannedGroup {
                key,
                display_base_name: stem.clone(),
                files: Vec::new(),
            })
            .files
            .push(file);
        discovered += 1;
    }
    Ok((groups.into_values().collect(), warnings, discovered))
}

fn thumbnail(path: &Path) -> AppResult<Vec<u8>> {
    let image = ImageReader::open(path)?.with_guessed_format()?.decode()?;
    let thumb = image.thumbnail(480, 480);
    let mut bytes = Vec::new();
    JpegEncoder::new_with_quality(&mut bytes, 80).encode_image(&thumb)?;
    Ok(bytes)
}

fn destination_entry(
    transaction: &Transaction<'_>,
    group: &ScannedGroup,
) -> AppResult<Option<i64>> {
    transaction
        .query_row(
            "SELECT id FROM photo_entries WHERE relative_dir=?1 AND pairing_key=?2",
            params![group.key.relative_dir, group.key.pairing_key],
            |row| row.get(0),
        )
        .optional()
        .map_err(AppError::from)
}

fn movement_candidate(
    transaction: &Transaction<'_>,
    file: &ScannedFile,
) -> AppResult<Option<(i64, i64)>> {
    let mut statement = transaction.prepare(
        "SELECT id,photo_entry_id,content_hash FROM photo_files
         WHERE file_kind=?1 AND file_size=?2 AND quick_fingerprint=?3 AND is_missing=1",
    )?;
    let candidates = statement
        .query_map(
            params![file.kind.as_str(), file.size as i64, file.quick_fingerprint],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )?
        .collect::<Result<Vec<_>, _>>()?;
    if candidates.len() == 1 {
        return Ok(Some((candidates[0].0, candidates[0].1)));
    }
    if candidates.len() > 1 {
        let hash = content_hash(&file.absolute_path)?;
        let matched: Vec<_> = candidates
            .into_iter()
            .filter(|candidate| candidate.2.as_deref() == Some(hash.as_str()))
            .collect();
        if matched.len() == 1 {
            return Ok(Some((matched[0].0, matched[0].1)));
        }
    }
    Ok(None)
}

fn refresh_entry_status(transaction: &Transaction<'_>, entry_id: i64) -> AppResult<()> {
    let (jpeg, arw): (i64, i64) = transaction.query_row(
        "SELECT SUM(file_kind='jpeg' AND is_missing=0),SUM(file_kind='arw' AND is_missing=0)
         FROM photo_files WHERE photo_entry_id=?1",
        [entry_id],
        |row| {
            Ok((
                row.get::<_, Option<i64>>(0)?.unwrap_or(0),
                row.get::<_, Option<i64>>(1)?.unwrap_or(0),
            ))
        },
    )?;
    let status = match (jpeg, arw) {
        (1, 1) => "complete",
        (1, 0) => "missing_raw",
        (0, 1) => "missing_jpeg",
        _ => "conflict",
    };
    let is_missing = i64::from(jpeg + arw == 0);
    transaction.execute(
        "UPDATE photo_entries SET pair_status=?1,is_missing=?2,updated_at=CURRENT_TIMESTAMP WHERE id=?3",
        params![status, is_missing, entry_id])?;
    Ok(())
}

pub fn scan_project(database_path: &Path) -> AppResult<ScanSummary> {
    let connection = db::connect(database_path)?;
    let root: String = connection.query_row(
        "SELECT photo_root FROM project_settings WHERE id=1",
        [],
        |row| row.get(0),
    )?;
    let root = PathBuf::from(root);
    if !root.is_dir() {
        return Err(AppError::InvalidPhotoRoot(root));
    }
    let (groups, mut warnings, discovered_files) = collect_groups(&root)?;
    let scan_complete = warnings.is_empty();
    let mut connection = db::connect(database_path)?;
    connection.execute("INSERT INTO scan_runs(status) VALUES('running')", [])?;
    let scan_id = connection.last_insert_rowid();
    let transaction = connection.transaction()?;
    if scan_complete {
        transaction.execute("UPDATE photo_files SET is_missing=1", [])?;
        transaction.execute("UPDATE photo_entries SET is_missing=1", [])?;
    }
    let mut summary = ScanSummary {
        discovered_files,
        total_groups: groups.len(),
        warnings: Vec::new(),
        ..Default::default()
    };
    let mut touched_entries = BTreeSet::new();

    for group in groups {
        let status = classify_group(&group.files);
        match status {
            "complete" => summary.complete += 1,
            "missing_jpeg" => summary.missing_jpeg += 1,
            "missing_raw" => summary.missing_raw += 1,
            _ => summary.conflicts += 1,
        }
        let mut entry_id = destination_entry(&transaction, &group)?;
        let mut candidates = Vec::new();
        if entry_id.is_none() && scan_complete {
            for file in &group.files {
                if let Some(candidate) = movement_candidate(&transaction, file)? {
                    candidates.push(candidate);
                }
            }
            let unique: BTreeSet<_> = candidates.iter().map(|candidate| candidate.1).collect();
            if unique.len() == 1 {
                entry_id = unique.first().copied();
            }
        }
        let entry_id = if let Some(id) = entry_id {
            transaction.execute(
                "UPDATE photo_entries SET relative_dir=?1,pairing_key=?2,display_base_name=?3,is_missing=0,updated_at=CURRENT_TIMESTAMP WHERE id=?4",
                params![group.key.relative_dir, group.key.pairing_key, group.display_base_name, id])?;
            id
        } else {
            transaction.execute(
                "INSERT INTO photo_entries(relative_dir,pairing_key,display_base_name,pair_status,is_missing) VALUES(?1,?2,?3,?4,0)",
                params![group.key.relative_dir, group.key.pairing_key, group.display_base_name, status])?;
            transaction.last_insert_rowid()
        };
        touched_entries.insert(entry_id);

        for file in &group.files {
            let existing: Option<i64> = transaction
                .query_row(
                    "SELECT id FROM photo_files WHERE relative_path=?1",
                    [&file.relative_path],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(file_id) = existing {
                transaction.execute(
                    "UPDATE photo_files SET photo_entry_id=?1,original_file_name=?2,file_kind=?3,file_size=?4,
                     modified_time=?5,quick_fingerprint=?6,is_missing=0,last_seen_scan_id=?7,updated_at=CURRENT_TIMESTAMP WHERE id=?8",
                    params![entry_id,file.original_file_name,file.kind.as_str(),file.size as i64,file.modified_time,
                        file.quick_fingerprint,scan_id,file_id])?;
            } else if scan_complete {
                if let Some((file_id, old_entry_id)) = movement_candidate(&transaction, file)? {
                    transaction.execute(
                        "UPDATE photo_files SET photo_entry_id=?1,original_file_name=?2,relative_path=?3,file_size=?4,
                         modified_time=?5,quick_fingerprint=?6,is_missing=0,last_seen_scan_id=?7,updated_at=CURRENT_TIMESTAMP WHERE id=?8",
                        params![entry_id,file.original_file_name,file.relative_path,file.size as i64,file.modified_time,
                            file.quick_fingerprint,scan_id,file_id])?;
                    touched_entries.insert(old_entry_id);
                    summary.moved_files += 1;
                } else {
                    transaction.execute(
                        "INSERT INTO photo_files(photo_entry_id,file_kind,original_file_name,relative_path,file_size,modified_time,
                         quick_fingerprint,is_missing,last_seen_scan_id) VALUES(?1,?2,?3,?4,?5,?6,?7,0,?8)",
                        params![entry_id,file.kind.as_str(),file.original_file_name,file.relative_path,file.size as i64,
                            file.modified_time,file.quick_fingerprint,scan_id])?;
                }
            } else {
                transaction.execute(
                    "INSERT INTO photo_files(photo_entry_id,file_kind,original_file_name,relative_path,file_size,modified_time,
                     quick_fingerprint,is_missing,last_seen_scan_id) VALUES(?1,?2,?3,?4,?5,?6,?7,0,?8)",
                    params![entry_id,file.kind.as_str(),file.original_file_name,file.relative_path,file.size as i64,
                        file.modified_time,file.quick_fingerprint,scan_id])?;
            }
        }

        if status != "conflict" {
            if let Some(jpeg) = group.files.iter().find(|file| file.kind == FileKind::Jpeg) {
                match thumbnail(&jpeg.absolute_path) {
                    Ok(bytes) => {
                        transaction.execute(
                        "UPDATE photo_entries SET thumbnail_blob=?1,thumbnail_mime='image/jpeg' WHERE id=?2", params![bytes,entry_id])?;
                    }
                    Err(error) => warnings.push(format!(
                        "缩略图生成失败 {}：{error}",
                        jpeg.absolute_path.display()
                    )),
                }
            }
        }
    }

    if scan_complete {
        let mut statement = transaction.prepare("SELECT id FROM photo_entries")?;
        for id in statement.query_map([], |row| row.get::<_, i64>(0))? {
            touched_entries.insert(id?);
        }
    }
    for id in touched_entries {
        refresh_entry_status(&transaction, id)?;
    }
    transaction.execute(
        "UPDATE scan_runs SET status='completed',completed_at=CURRENT_TIMESTAMP,error_message=?1 WHERE id=?2",
        params![if warnings.is_empty() { None } else { Some(warnings.join("\n")) }, scan_id])?;
    transaction.commit()?;
    summary.warnings = warnings;
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn scanned(kind: FileKind) -> ScannedFile {
        ScannedFile {
            absolute_path: PathBuf::new(),
            relative_path: String::new(),
            original_file_name: String::new(),
            kind,
            size: 0,
            modified_time: 0.0,
            quick_fingerprint: String::new(),
        }
    }

    #[test]
    fn classifies_pairing_states_and_conflicts() {
        assert_eq!(
            classify_group(&[scanned(FileKind::Jpeg), scanned(FileKind::Arw)]),
            "complete"
        );
        assert_eq!(classify_group(&[scanned(FileKind::Jpeg)]), "missing_raw");
        assert_eq!(classify_group(&[scanned(FileKind::Arw)]), "missing_jpeg");
        assert_eq!(
            classify_group(&[
                scanned(FileKind::Jpeg),
                scanned(FileKind::Jpeg),
                scanned(FileKind::Arw)
            ]),
            "conflict"
        );
    }

    #[test]
    fn fingerprints_are_stable_and_content_sensitive() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.arw");
        File::create(&path)
            .unwrap()
            .write_all(b"same content")
            .unwrap();
        let a = quick_fingerprint(&path, 12).unwrap();
        let b = quick_fingerprint(&path, 12).unwrap();
        assert_eq!(a, b);
        File::create(&path)
            .unwrap()
            .write_all(b"other data!!")
            .unwrap();
        let c = quick_fingerprint(&path, 12).unwrap();
        assert_ne!(a, c);
    }
}
