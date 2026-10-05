use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectInfo {
    pub database_path: String,
    pub photo_root: String,
    pub sort_field: String,
    pub sort_direction: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TagNode {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub name: String,
    pub sort_order: i64,
    pub photo_count: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoEntry {
    pub id: i64,
    pub relative_dir: String,
    pub display_base_name: String,
    pub pair_status: String,
    pub is_missing: bool,
    pub tag_id: Option<i64>,
    pub tag_path: Option<String>,
    pub jpeg_relative_path: Option<String>,
    pub raw_relative_path: Option<String>,
    pub modified_time: Option<f64>,
    pub thumbnail_data_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProjectCounts {
    pub total: i64,
    pub tagged: i64,
    pub missing: i64,
    pub conflicts: i64,
    pub incomplete: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSnapshot {
    pub project: ProjectInfo,
    pub tags: Vec<TagNode>,
    pub photos: Vec<PhotoEntry>,
    pub counts: ProjectCounts,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoQuery {
    pub filter: Option<String>,
    pub sort_field: Option<String>,
    pub sort_direction: Option<String>,
}

impl Default for PhotoQuery {
    fn default() -> Self {
        Self {
            filter: Some("all".into()),
            sort_field: None,
            sort_direction: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ScanSummary {
    pub discovered_files: usize,
    pub total_groups: usize,
    pub complete: usize,
    pub missing_jpeg: usize,
    pub missing_raw: usize,
    pub conflicts: usize,
    pub moved_files: usize,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteImpact {
    pub tag_count: i64,
    pub affected_photos: i64,
}
