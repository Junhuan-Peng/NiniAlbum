use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::UNIX_EPOCH,
};

use base64::{engine::general_purpose::STANDARD, Engine};
use exif::{In, Tag, Value};
use image::{codecs::jpeg::JpegEncoder, DynamicImage, GenericImageView, ImageReader};
use rayon::prelude::*;
use rusqlite::{params, OptionalExtension, Transaction};
use walkdir::WalkDir;

use crate::{
    db,
    error::{AppError, AppResult},
    models::{ScanProgress, ScanSummary},
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
    content_hash: Option<String>,
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

#[derive(Debug)]
struct FileCandidate {
    absolute_path: PathBuf,
    relative_path: String,
    original_file_name: String,
    kind: FileKind,
    key: GroupKey,
    display_base_name: String,
}

#[derive(Debug, Clone, Default)]
struct PhotoMetadata {
    pixel_width: Option<u32>,
    pixel_height: Option<u32>,
    orientation: Option<u16>,
    captured_at: Option<String>,
    camera_make: Option<String>,
    camera_model: Option<String>,
    lens_model: Option<String>,
    exposure_time: Option<String>,
    aperture: Option<String>,
    iso: Option<u32>,
    focal_length: Option<String>,
    latitude: Option<f64>,
    longitude: Option<f64>,
    perceptual_hash: Option<u64>,
}

#[derive(Debug)]
struct PhotoArtifact {
    thumbnail: Option<Vec<u8>>,
    metadata: PhotoMetadata,
}

pub type ProgressCallback = Arc<dyn Fn(ScanProgress) + Send + Sync>;

fn report(progress: &ProgressCallback, phase: &str, completed: usize, total: usize, message: &str) {
    progress(ScanProgress {
        phase: phase.into(),
        completed,
        total,
        message: message.into(),
    });
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

fn collect_groups(
    root: &Path,
    progress: &ProgressCallback,
) -> AppResult<(Vec<ScannedGroup>, Vec<String>, usize)> {
    let mut candidates = Vec::new();
    let mut warnings = Vec::new();
    report(progress, "discovering", 0, 0, "正在递归发现照片文件");
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
        let key = GroupKey {
            relative_dir: relative
                .parent()
                .unwrap_or_else(|| Path::new(""))
                .to_string_lossy()
                .into_owned(),
            pairing_key: normalize_pairing_key(&stem),
        };
        candidates.push(FileCandidate {
            absolute_path: entry.path().to_path_buf(),
            relative_path: relative.to_string_lossy().into_owned(),
            original_file_name: entry.file_name().to_string_lossy().into_owned(),
            kind,
            key,
            display_base_name: stem,
        });
        if candidates.len() % 200 == 0 {
            report(
                progress,
                "discovering",
                candidates.len(),
                0,
                "正在递归发现照片文件",
            );
        }
    }

    let total = candidates.len();
    report(progress, "fingerprinting", 0, total, "正在并发读取文件指纹");
    let completed = AtomicUsize::new(0);
    let fingerprint_results = rayon::ThreadPoolBuilder::new()
        .num_threads(4)
        .thread_name(|index| format!("fingerprint-{index}"))
        .build()
        .expect("failed to create fingerprint thread pool")
        .install(|| {
            candidates
                .into_par_iter()
                .map(|candidate| {
                    let result = (|| {
                        let metadata = std::fs::metadata(&candidate.absolute_path)?;
                        let fingerprint =
                            quick_fingerprint(&candidate.absolute_path, metadata.len())?;
                        Ok::<_, AppError>((
                            candidate.key,
                            candidate.display_base_name,
                            ScannedFile {
                                absolute_path: candidate.absolute_path,
                                relative_path: candidate.relative_path,
                                original_file_name: candidate.original_file_name,
                                kind: candidate.kind,
                                size: metadata.len(),
                                modified_time: modified_time(&metadata),
                                quick_fingerprint: fingerprint,
                                content_hash: None,
                            },
                        ))
                    })()
                    .map_err(|error| error.to_string());
                    let done = completed.fetch_add(1, Ordering::Relaxed) + 1;
                    if done.is_multiple_of(20) || done == total {
                        report(
                            progress,
                            "fingerprinting",
                            done,
                            total,
                            "正在并发读取文件指纹",
                        );
                    }
                    result
                })
                .collect::<Vec<_>>()
        });

    let mut groups: BTreeMap<GroupKey, ScannedGroup> = BTreeMap::new();
    let mut discovered = 0;
    for result in fingerprint_results {
        match result {
            Ok((key, display_base_name, file)) => {
                groups
                    .entry(key.clone())
                    .or_insert_with(|| ScannedGroup {
                        key,
                        display_base_name,
                        files: Vec::new(),
                    })
                    .files
                    .push(file);
                discovered += 1;
            }
            Err(error) => warnings.push(error),
        }
    }

    let mut groups: Vec<_> = groups.into_values().collect();
    let mut fingerprint_counts = BTreeMap::new();
    for file in groups.iter().flat_map(|group| &group.files) {
        *fingerprint_counts
            .entry((
                file.kind.as_str(),
                file.size,
                file.quick_fingerprint.clone(),
            ))
            .or_insert(0_usize) += 1;
    }
    let duplicate_total = groups
        .iter()
        .flat_map(|group| &group.files)
        .filter(|file| {
            fingerprint_counts
                .get(&(
                    file.kind.as_str(),
                    file.size,
                    file.quick_fingerprint.clone(),
                ))
                .copied()
                .unwrap_or(0)
                > 1
        })
        .count();
    if duplicate_total > 0 {
        report(
            progress,
            "fingerprinting",
            0,
            duplicate_total,
            "正在确认重复指纹",
        );
        let duplicate_done = AtomicUsize::new(0);
        let hash_errors = Mutex::new(Vec::new());
        groups.par_iter_mut().for_each(|group| {
            for file in &mut group.files {
                let key = (
                    file.kind.as_str(),
                    file.size,
                    file.quick_fingerprint.clone(),
                );
                if fingerprint_counts.get(&key).copied().unwrap_or(0) > 1 {
                    match content_hash(&file.absolute_path) {
                        Ok(hash) => file.content_hash = Some(hash),
                        Err(error) => {
                            hash_errors
                                .lock()
                                .expect("hash error lock poisoned")
                                .push(format!(
                                    "完整指纹生成失败 {}：{error}",
                                    file.absolute_path.display()
                                ))
                        }
                    }
                    let done = duplicate_done.fetch_add(1, Ordering::Relaxed) + 1;
                    if done.is_multiple_of(10) || done == duplicate_total {
                        report(
                            progress,
                            "fingerprinting",
                            done,
                            duplicate_total,
                            "正在确认重复指纹",
                        );
                    }
                }
            }
        });
        warnings.extend(hash_errors.into_inner().expect("hash error lock poisoned"));
    }
    Ok((groups, warnings, discovered))
}

fn display_field(exif: &exif::Exif, tag: Tag) -> Option<String> {
    exif.get_field(tag, In::PRIMARY)
        .map(|field| field.display_value().with_unit(exif).to_string())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn ascii_field(exif: &exif::Exif, tag: Tag) -> Option<String> {
    let field = exif.get_field(tag, In::PRIMARY)?;
    match &field.value {
        Value::Ascii(values) => values.first().map(|bytes| {
            String::from_utf8_lossy(bytes)
                .trim_matches(char::from(0))
                .trim()
                .to_string()
        }),
        _ => display_field(exif, tag),
    }
    .filter(|value| !value.is_empty())
}

fn gps_coordinate(exif: &exif::Exif, value_tag: Tag, reference_tag: Tag) -> Option<f64> {
    let field = exif.get_field(value_tag, In::PRIMARY)?;
    let Value::Rational(values) = &field.value else {
        return None;
    };
    if values.len() < 3 {
        return None;
    }
    let decimal = values[0].to_f64() + values[1].to_f64() / 60.0 + values[2].to_f64() / 3600.0;
    let reference = ascii_field(exif, reference_tag)?;
    Some(if matches!(reference.as_str(), "S" | "W") {
        -decimal
    } else {
        decimal
    })
}

fn normalize_exif_date(value: String) -> Option<String> {
    if value.len() < 19 {
        return None;
    }
    let mut bytes = value.into_bytes();
    if bytes.get(4) == Some(&b':') && bytes.get(7) == Some(&b':') {
        bytes[4] = b'-';
        bytes[7] = b'-';
    }
    String::from_utf8(bytes)
        .ok()
        .map(|value| value[..19].to_string())
}

fn read_exif(path: &Path) -> PhotoMetadata {
    let Ok(file) = File::open(path) else {
        return PhotoMetadata::default();
    };
    let Ok(exif) = exif::Reader::new().read_from_container(&mut BufReader::new(file)) else {
        return PhotoMetadata::default();
    };
    let orientation = exif
        .get_field(Tag::Orientation, In::PRIMARY)
        .and_then(|field| field.value.get_uint(0))
        .map(|value| value as u16);
    PhotoMetadata {
        orientation,
        captured_at: display_field(&exif, Tag::DateTimeOriginal).and_then(normalize_exif_date),
        camera_make: ascii_field(&exif, Tag::Make),
        camera_model: ascii_field(&exif, Tag::Model),
        lens_model: ascii_field(&exif, Tag::LensModel),
        exposure_time: display_field(&exif, Tag::ExposureTime),
        aperture: display_field(&exif, Tag::FNumber),
        iso: exif
            .get_field(Tag::PhotographicSensitivity, In::PRIMARY)
            .and_then(|field| field.value.get_uint(0)),
        focal_length: display_field(&exif, Tag::FocalLength),
        latitude: gps_coordinate(&exif, Tag::GPSLatitude, Tag::GPSLatitudeRef),
        longitude: gps_coordinate(&exif, Tag::GPSLongitude, Tag::GPSLongitudeRef),
        ..Default::default()
    }
}

fn orient_image(image: DynamicImage, orientation: Option<u16>) -> DynamicImage {
    match orientation.unwrap_or(1) {
        2 => image.fliph(),
        3 => image.rotate180(),
        4 => image.flipv(),
        5 => image.rotate90().fliph(),
        6 => image.rotate90(),
        7 => image.rotate270().fliph(),
        8 => image.rotate270(),
        _ => image,
    }
}

fn difference_hash(image: &DynamicImage) -> u64 {
    let gray = image
        .resize_exact(9, 8, image::imageops::FilterType::Triangle)
        .to_luma8();
    let mut hash = 0_u64;
    for y in 0..8 {
        for x in 0..8 {
            hash <<= 1;
            if gray.get_pixel(x, y)[0] > gray.get_pixel(x + 1, y)[0] {
                hash |= 1;
            }
        }
    }
    hash
}

fn photo_artifact(path: &Path) -> AppResult<PhotoArtifact> {
    let mut metadata = read_exif(path);
    let image = ImageReader::open(path)?.with_guessed_format()?.decode()?;
    let image = orient_image(image, metadata.orientation);
    let (width, height) = image.dimensions();
    metadata.pixel_width = Some(width);
    metadata.pixel_height = Some(height);
    metadata.perceptual_hash = Some(difference_hash(&image));
    let thumb = image.thumbnail(480, 480);
    let mut bytes = Vec::new();
    JpegEncoder::new_with_quality(&mut bytes, 80).encode_image(&thumb)?;
    Ok(PhotoArtifact {
        thumbnail: Some(bytes),
        metadata,
    })
}

fn generate_artifacts(
    groups: &[ScannedGroup],
    progress: &ProgressCallback,
) -> Vec<Result<PhotoArtifact, String>> {
    let total = groups.len();
    let completed = AtomicUsize::new(0);
    report(progress, "thumbnails", 0, total, "正在并发生成 JPEG 缩略图");
    let work = || {
        groups
            .par_iter()
            .map(|group| {
                let result = if classify_group(&group.files) == "conflict" {
                    Ok(PhotoArtifact {
                        thumbnail: None,
                        metadata: PhotoMetadata::default(),
                    })
                } else if let Some(jpeg) =
                    group.files.iter().find(|file| file.kind == FileKind::Jpeg)
                {
                    photo_artifact(&jpeg.absolute_path).map_err(|error| {
                        format!("缩略图生成失败 {}：{error}", jpeg.absolute_path.display())
                    })
                } else {
                    Ok(PhotoArtifact {
                        thumbnail: None,
                        metadata: PhotoMetadata::default(),
                    })
                };
                let done = completed.fetch_add(1, Ordering::Relaxed) + 1;
                if done.is_multiple_of(10) || done == total {
                    report(
                        progress,
                        "thumbnails",
                        done,
                        total,
                        "正在并发生成 JPEG 缩略图",
                    );
                }
                result
            })
            .collect()
    };
    rayon::ThreadPoolBuilder::new()
        .num_threads(4)
        .thread_name(|index| format!("thumbnail-{index}"))
        .build()
        .expect("failed to create thumbnail thread pool")
        .install(work)
}

pub fn photo_preview(database_path: &Path, photo_id: i64) -> AppResult<String> {
    let connection = db::connect(database_path)?;
    let (root, relative): (String, String) = connection.query_row(
        "SELECT s.photo_root,f.relative_path FROM project_settings s,photo_files f
         WHERE f.photo_entry_id=?1 AND f.file_kind='jpeg' AND f.is_missing=0 ORDER BY f.id LIMIT 1",
        [photo_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let path = PathBuf::from(root).join(relative);
    let metadata = read_exif(&path);
    let image = ImageReader::open(path)?.with_guessed_format()?.decode()?;
    let image = orient_image(image, metadata.orientation);
    let preview = image.thumbnail(2400, 2400);
    let mut bytes = Vec::new();
    JpegEncoder::new_with_quality(&mut bytes, 88).encode_image(&preview)?;
    Ok(format!("data:image/jpeg;base64,{}", STANDARD.encode(bytes)))
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

fn timestamp_seconds(value: &str) -> Option<i64> {
    let values = value
        .split(['-', ' ', ':'])
        .map(str::parse::<i64>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    if values.len() != 6 {
        return None;
    }
    let (mut year, month, day) = (values[0], values[1], values[2]);
    year -= i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let adjusted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * adjusted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    Some(days * 86_400 + values[3] * 3_600 + values[4] * 60 + values[5])
}

fn refresh_smart_groups(transaction: &Transaction<'_>) -> AppResult<()> {
    transaction.execute(
        "UPDATE photo_entries SET date_group=CASE WHEN captured_at IS NULL THEN NULL ELSE substr(captured_at,1,10) END,
         location_group=CASE WHEN latitude IS NULL OR longitude IS NULL THEN NULL ELSE printf('%.2f, %.2f',latitude,longitude) END,
         similarity_group=NULL",
        [],
    )?;
    let mut statement = transaction.prepare(
        "SELECT id,captured_at,perceptual_hash FROM photo_entries
         WHERE is_missing=0 AND captured_at IS NOT NULL AND perceptual_hash IS NOT NULL
         ORDER BY captured_at,id",
    )?;
    let photos = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)? as u64,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    let mut parent: Vec<usize> = (0..photos.len()).collect();
    fn root(parent: &mut [usize], mut value: usize) -> usize {
        while parent[value] != value {
            parent[value] = parent[parent[value]];
            value = parent[value];
        }
        value
    }
    for current in 0..photos.len() {
        let Some(current_time) = timestamp_seconds(&photos[current].1) else {
            continue;
        };
        for previous in (0..current).rev() {
            let Some(previous_time) = timestamp_seconds(&photos[previous].1) else {
                continue;
            };
            if current_time - previous_time > 10 * 60 {
                break;
            }
            if (photos[current].2 ^ photos[previous].2).count_ones() <= 8 {
                let current_root = root(&mut parent, current);
                let previous_root = root(&mut parent, previous);
                if current_root != previous_root {
                    parent[current_root] = previous_root;
                }
            }
        }
    }
    let mut groups: BTreeMap<usize, Vec<i64>> = BTreeMap::new();
    for (index, photo) in photos.iter().enumerate() {
        let group_root = root(&mut parent, index);
        groups.entry(group_root).or_default().push(photo.0);
    }
    let mut group_number = 0;
    for ids in groups.values().filter(|ids| ids.len() > 1) {
        group_number += 1;
        let label = format!("相似组 {group_number:03}");
        for id in ids {
            transaction.execute(
                "UPDATE photo_entries SET similarity_group=?1 WHERE id=?2",
                params![label, id],
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
pub fn scan_project(database_path: &Path) -> AppResult<ScanSummary> {
    scan_project_with_progress(database_path, Arc::new(|_| {}))
}

pub fn scan_project_with_progress(
    database_path: &Path,
    progress: ProgressCallback,
) -> AppResult<ScanSummary> {
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
    let (groups, mut warnings, discovered_files) = collect_groups(&root, &progress)?;
    let artifacts = generate_artifacts(&groups, &progress);
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

    let group_total = groups.len();
    report(&progress, "database", 0, group_total, "正在写入项目数据库");
    for (index, (group, artifact_result)) in groups.into_iter().zip(artifacts).enumerate() {
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
                     modified_time=?5,quick_fingerprint=?6,content_hash=COALESCE(?7,content_hash),is_missing=0,
                     last_seen_scan_id=?8,updated_at=CURRENT_TIMESTAMP WHERE id=?9",
                    params![entry_id,file.original_file_name,file.kind.as_str(),file.size as i64,file.modified_time,
                        file.quick_fingerprint,file.content_hash,scan_id,file_id])?;
            } else if scan_complete {
                if let Some((file_id, old_entry_id)) = movement_candidate(&transaction, file)? {
                    transaction.execute(
                        "UPDATE photo_files SET photo_entry_id=?1,original_file_name=?2,relative_path=?3,file_size=?4,
                         modified_time=?5,quick_fingerprint=?6,content_hash=COALESCE(?7,content_hash),is_missing=0,
                         last_seen_scan_id=?8,updated_at=CURRENT_TIMESTAMP WHERE id=?9",
                        params![entry_id,file.original_file_name,file.relative_path,file.size as i64,file.modified_time,
                            file.quick_fingerprint,file.content_hash,scan_id,file_id])?;
                    touched_entries.insert(old_entry_id);
                    summary.moved_files += 1;
                } else {
                    transaction.execute(
                        "INSERT INTO photo_files(photo_entry_id,file_kind,original_file_name,relative_path,file_size,modified_time,
                         quick_fingerprint,content_hash,is_missing,last_seen_scan_id) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,0,?9)",
                        params![entry_id,file.kind.as_str(),file.original_file_name,file.relative_path,file.size as i64,
                            file.modified_time,file.quick_fingerprint,file.content_hash,scan_id])?;
                }
            } else {
                transaction.execute(
                    "INSERT INTO photo_files(photo_entry_id,file_kind,original_file_name,relative_path,file_size,modified_time,
                     quick_fingerprint,content_hash,is_missing,last_seen_scan_id) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,0,?9)",
                    params![entry_id,file.kind.as_str(),file.original_file_name,file.relative_path,file.size as i64,
                        file.modified_time,file.quick_fingerprint,file.content_hash,scan_id])?;
            }
        }

        match artifact_result {
            Ok(artifact) => {
                if let Some(bytes) = artifact.thumbnail {
                    transaction.execute(
                        "UPDATE photo_entries SET thumbnail_blob=?1,thumbnail_mime='image/jpeg' WHERE id=?2",
                        params![bytes,entry_id])?;
                }
                let metadata = artifact.metadata;
                transaction.execute(
                    "UPDATE photo_entries SET pixel_width=?1,pixel_height=?2,orientation=?3,captured_at=?4,
                     camera_make=?5,camera_model=?6,lens_model=?7,exposure_time=?8,aperture=?9,iso=?10,
                     focal_length=?11,latitude=?12,longitude=?13,perceptual_hash=?14 WHERE id=?15",
                    params![metadata.pixel_width,metadata.pixel_height,metadata.orientation,metadata.captured_at,
                        metadata.camera_make,metadata.camera_model,metadata.lens_model,metadata.exposure_time,
                        metadata.aperture,metadata.iso,metadata.focal_length,metadata.latitude,metadata.longitude,
                        metadata.perceptual_hash.map(|value| value as i64),entry_id])?;
            }
            Err(error) => warnings.push(error),
        }
        let done = index + 1;
        if done.is_multiple_of(25) || done == group_total {
            report(
                &progress,
                "database",
                done,
                group_total,
                "正在写入项目数据库",
            );
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
    refresh_smart_groups(&transaction)?;
    transaction.execute(
        "UPDATE scan_runs SET status='completed',completed_at=CURRENT_TIMESTAMP,error_message=?1 WHERE id=?2",
        params![if warnings.is_empty() { None } else { Some(warnings.join("\n")) }, scan_id])?;
    transaction.commit()?;
    summary.warnings = warnings;
    report(&progress, "complete", 1, 1, "扫描完成");
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
            content_hash: None,
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

    #[test]
    fn exif_dates_convert_to_comparable_seconds() {
        assert_eq!(
            timestamp_seconds("2026-10-05 12:00:30").unwrap()
                - timestamp_seconds("2026-10-05 11:59:30").unwrap(),
            60
        );
        assert!(timestamp_seconds("not a date").is_none());
    }

    #[test]
    fn recursive_rescan_tracks_moves_and_preserves_tags() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path().join("photos");
        let original = root.join("day-one");
        std::fs::create_dir_all(&original).unwrap();
        let jpeg = original.join("DSC0001.JPG");
        let raw = original.join("dsc0001.arw");
        image::RgbImage::from_pixel(8, 8, image::Rgb([30, 60, 90]))
            .save(&jpeg)
            .unwrap();
        File::create(&raw)
            .unwrap()
            .write_all(b"raw payload")
            .unwrap();
        let database = workspace.path().join("project.sqlite");
        db::create_project(&database, &root).unwrap();

        let progress_events = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&progress_events);
        let first = scan_project_with_progress(
            &database,
            Arc::new(move |progress| {
                captured.lock().unwrap().push(progress);
            }),
        )
        .unwrap();
        assert_eq!(first.complete, 1);
        let phases: BTreeSet<_> = progress_events
            .lock()
            .unwrap()
            .iter()
            .map(|progress| progress.phase.clone())
            .collect();
        assert!(phases.contains("discovering"));
        assert!(phases.contains("fingerprinting"));
        assert!(phases.contains("thumbnails"));
        assert!(phases.contains("database"));
        assert!(phases.contains("complete"));
        let connection = db::connect(&database).unwrap();
        let entry_id: i64 = connection
            .query_row("SELECT id FROM photo_entries", [], |row| row.get(0))
            .unwrap();
        let tag_id = db::create_tag(&connection, None, "旅行").unwrap();
        db::assign_tag(&connection, entry_id, Some(tag_id)).unwrap();
        drop(connection);

        let moved = root.join("day-two");
        std::fs::create_dir_all(&moved).unwrap();
        std::fs::rename(&jpeg, moved.join("DSC0001.JPG")).unwrap();
        std::fs::rename(&raw, moved.join("dsc0001.arw")).unwrap();
        let second = scan_project(&database).unwrap();
        assert_eq!(second.moved_files, 2);
        let connection = db::connect(&database).unwrap();
        let state: (i64, Option<i64>, String, i64) = connection
            .query_row(
                "SELECT COUNT(*),MAX(tag_id),MAX(relative_dir),SUM(is_missing) FROM photo_entries",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(state.0, 1);
        assert_eq!(state.1, Some(tag_id));
        assert_eq!(PathBuf::from(state.2), PathBuf::from("day-two"));
        assert_eq!(state.3, 0);
    }

    #[test]
    #[ignore = "requires NINI_REAL_PHOTO_ROOT and a large local photo collection"]
    fn smoke_tests_a_real_photo_collection_without_modifying_sources() {
        let root = PathBuf::from(
            std::env::var("NINI_REAL_PHOTO_ROOT")
                .expect("set NINI_REAL_PHOTO_ROOT to an existing photo directory"),
        );
        assert!(root.is_dir());
        let workspace = tempfile::tempdir().unwrap();
        let database = workspace.path().join("real-smoke.sqlite");
        db::create_project(&database, &root).unwrap();

        let started = std::time::Instant::now();
        let summary = scan_project(&database).unwrap();
        let connection = db::connect(&database).unwrap();
        let database_counts: (i64, i64) = connection
            .query_row(
                "SELECT (SELECT COUNT(*) FROM photo_entries),(SELECT COUNT(*) FROM photo_files)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(database_counts.0 as usize, summary.total_groups);
        assert_eq!(database_counts.1 as usize, summary.discovered_files);
        assert!(summary.total_groups > 0);
        let metadata_counts: (i64, i64, i64, i64) = connection
            .query_row(
                "SELECT SUM(captured_at IS NOT NULL),SUM(latitude IS NOT NULL),
                 SUM(date_group IS NOT NULL),SUM(similarity_group IS NOT NULL) FROM photo_entries",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert!(
            metadata_counts.0 > 0,
            "real JPEGs should expose capture dates"
        );

        let photo_id: i64 = connection
            .query_row(
                "SELECT id FROM photo_entries WHERE pair_status='complete' AND is_missing=0 LIMIT 1",
                [],
                |row| row.get(0),
            )
            .expect("the real collection should contain at least one complete pair");
        let tag_id = db::create_tag(&connection, None, "真实目录冒烟测试").unwrap();
        db::assign_tag(&connection, photo_id, Some(tag_id)).unwrap();
        drop(connection);

        let export_root = workspace.path().join("export");
        let exported = crate::export::export_photos(&database, &export_root, "skip").unwrap();
        assert_eq!(exported.succeeded, 1);
        assert_eq!(exported.failed, 0);
        println!(
            "real scan: files={}, groups={}, complete={}, missing_jpeg={}, missing_raw={}, conflicts={}, warnings={}, exif_dates={}, gps={}, date_groups={}, similar_photos={}, elapsed={:?}",
            summary.discovered_files,
            summary.total_groups,
            summary.complete,
            summary.missing_jpeg,
            summary.missing_raw,
            summary.conflicts,
            summary.warnings.len(),
            metadata_counts.0,
            metadata_counts.1,
            metadata_counts.2,
            metadata_counts.3,
            started.elapsed()
        );
    }
}
