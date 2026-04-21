use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherAttachmentRef {
    pub id: String,
    pub original_name: String,
    pub stored_path: String,
    pub mime_type: String,
    pub size_bytes: u64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherAttachmentRecord {
    pub id: String,
    pub workspace_id: String,
    pub message_id: Option<String>,
    pub original_name: String,
    pub stored_path: String,
    pub mime_type: String,
    pub size_bytes: u64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CadPoint {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CadBBox {
    pub min_x: f64,
    pub min_y: f64,
    pub max_x: f64,
    pub max_y: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DwgLayerSummary {
    pub name: String,
    pub entity_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DwgBlockSummary {
    pub name: String,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DwgParseSummary {
    pub file_path: String,
    pub parser_version: String,
    pub total_entities: usize,
    pub unknown_entity_count: usize,
    pub bounds: Option<CadBBox>,
    pub layers: Vec<DwgLayerSummary>,
    pub entity_counts: BTreeMap<String, usize>,
    pub text_samples: Vec<String>,
    pub blocks: Vec<DwgBlockSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CadEntityRecord {
    pub id: String,
    pub handle: String,
    pub entity_type: String,
    pub raw_type: String,
    pub layer: String,
    pub color: Option<i64>,
    pub line_type: Option<String>,
    pub text: Option<String>,
    pub block_name: Option<String>,
    pub center: Option<CadPoint>,
    pub radius: Option<f64>,
    #[serde(default)]
    pub vertices: Vec<CadPoint>,
    pub bbox: Option<CadBBox>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DwgParseCacheRecord {
    pub id: String,
    pub project_path: String,
    pub file_path: String,
    pub file_size: u64,
    pub file_mtime: i64,
    pub parser_version: String,
    pub summary: DwgParseSummary,
    pub entities: Vec<CadEntityRecord>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CadEntityQueryFilters {
    #[serde(default)]
    pub layers: Vec<String>,
    #[serde(default)]
    pub entity_types: Vec<String>,
    pub text_query: Option<String>,
    pub bbox: Option<CadBBox>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CadEntityQueryResult {
    pub items: Vec<CadEntityRecord>,
    pub total: usize,
    pub next_cursor: Option<usize>,
    pub applied_filters: CadEntityQueryFilters,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CadReviewIssueRecord {
    pub id: String,
    pub run_id: String,
    pub severity: String,
    pub title: String,
    pub description: String,
    pub layer: Option<String>,
    #[serde(default)]
    pub entity_refs: Vec<String>,
    pub anchor_point: Option<CadPoint>,
    pub bbox: Option<CadBBox>,
    pub rule_ref: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CadReviewRunRecord {
    pub id: String,
    pub workspace_id: String,
    pub file_path: String,
    pub source_message_id: String,
    pub result_message_id: Option<String>,
    #[serde(default)]
    pub rule_attachment_ids: Vec<String>,
    pub goal: String,
    pub status: String,
    pub summary: String,
    pub issue_count: usize,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CadReviewRunDetail {
    pub run: CadReviewRunRecord,
    pub issues: Vec<CadReviewIssueRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveDwgParseCacheInput {
    pub project_path: String,
    pub file_path: String,
    pub file_size: u64,
    pub file_mtime: i64,
    pub parser_version: String,
    pub summary: DwgParseSummary,
    pub entities: Vec<CadEntityRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateCadReviewRunInput {
    pub workspace_id: String,
    pub file_path: String,
    pub source_message_id: String,
    #[serde(default)]
    pub rule_attachment_ids: Vec<String>,
    pub goal: String,
    pub status: String,
    pub summary: String,
    pub issues: Vec<CreateCadReviewIssueInput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateCadReviewIssueInput {
    pub severity: String,
    pub title: String,
    pub description: String,
    pub layer: Option<String>,
    #[serde(default)]
    pub entity_refs: Vec<String>,
    pub anchor_point: Option<CadPoint>,
    pub bbox: Option<CadBBox>,
    pub rule_ref: Option<String>,
}

pub fn build_attachment_context(attachments: &[DispatcherAttachmentRecord]) -> Option<String> {
    if attachments.is_empty() {
        return None;
    }

    let lines = attachments
        .iter()
        .map(|attachment| {
            format!(
                "- 名称：{}；路径：{}；类型：{}；大小：{} 字节；建议用途：{}",
                attachment.original_name,
                attachment.stored_path,
                attachment.mime_type,
                attachment.size_bytes,
                attachment_hint(attachment),
            )
        })
        .collect::<Vec<_>>();

    Some(format!(
        "以下文件已作为本轮会话附件提供，可直接按路径读取：\n{}",
        lines.join("\n")
    ))
}

pub fn filter_entities(
    entities: &[CadEntityRecord],
    filters: &CadEntityQueryFilters,
) -> Vec<CadEntityRecord> {
    let layer_filters = filters
        .layers
        .iter()
        .map(|layer| layer.to_ascii_lowercase())
        .collect::<Vec<_>>();
    let type_filters = filters
        .entity_types
        .iter()
        .map(|entity_type| entity_type.to_ascii_lowercase())
        .collect::<Vec<_>>();
    let text_query = filters
        .text_query
        .as_ref()
        .map(|query| query.trim().to_ascii_lowercase())
        .filter(|query| !query.is_empty());

    entities
        .iter()
        .filter(|entity| {
            (layer_filters.is_empty() || layer_filters.contains(&entity.layer.to_ascii_lowercase()))
                && (type_filters.is_empty()
                    || type_filters.contains(&entity.entity_type.to_ascii_lowercase()))
                && text_query.as_ref().is_none_or(|query| {
                    entity
                        .text
                        .as_ref()
                        .is_some_and(|text| text.to_ascii_lowercase().contains(query))
                        || entity
                            .block_name
                            .as_ref()
                            .is_some_and(|name| name.to_ascii_lowercase().contains(query))
                })
                && filters.bbox.as_ref().is_none_or(|bbox| {
                    entity
                        .bbox
                        .as_ref()
                        .is_some_and(|value| bbox_intersects(value, bbox))
                })
        })
        .cloned()
        .collect()
}

pub fn bbox_intersects(left: &CadBBox, right: &CadBBox) -> bool {
    !(left.max_x < right.min_x
        || left.min_x > right.max_x
        || left.max_y < right.min_y
        || left.min_y > right.max_y)
}

fn attachment_hint(attachment: &DispatcherAttachmentRecord) -> &'static str {
    let lower_name = attachment.original_name.to_ascii_lowercase();
    if lower_name.ends_with(".md") || attachment.mime_type.contains("markdown") {
        "规则说明 Markdown，可先读取后归纳审查规则"
    } else if lower_name.ends_with(".dwg") {
        "DWG 图纸，可结合 CAD 工具与解析缓存审查"
    } else if lower_name.ends_with(".json") {
        "结构化配置/规则文件，可直接解析字段"
    } else {
        "附件文件，可按需读取内容"
    }
}
