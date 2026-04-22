use std::collections::BTreeSet;
use std::fs;

use async_trait::async_trait;
use serde_json::{json, Value};
use tauri::Emitter;

use super::common::{string_arg, usize_arg, with_result_mode_parameter};
use crate::agent::cad::{
    bbox_intersects, filter_entities, CadBBox, CadEntityDetail, CadEntityQueryFilters,
    CadEntityRecord, CadPoint, CadViewportHint, CreateCadReviewIssueInput, CreateCadReviewRunInput,
    DwgDocumentRecord, DwgParseCacheRecord, DwgViewerSessionState,
};
use crate::agent::db::DispatcherDb;
use crate::agent::tools::context::ToolContext;
use crate::agent::tools::registry::AgentTool;

const DEFAULT_PARSER_VERSION: &str = "dwg-worker-v1";

pub(super) fn cad_compute_geometry_tool() -> Box<dyn AgentTool> {
    Box::new(CadComputeGeometryTool)
}

pub(super) fn cad_save_review_result_tool() -> Box<dyn AgentTool> {
    Box::new(CadSaveReviewResultTool)
}

struct CadComputeGeometryTool;
struct CadSaveReviewResultTool;

#[async_trait]
impl AgentTool for CadComputeGeometryTool {
    fn name(&self) -> &'static str {
        "cad_compute_geometry"
    }

    fn description(&self) -> &'static str {
        "对 CAD 点位、包围盒或缓存实体做几何计算，适合辅助审查规则判断。"
    }

    fn parameters(&self) -> Value {
        with_result_mode_parameter(
            json!({
                "type": "object",
                "properties": {
                    "operation": {
                        "type": "string",
                        "enum": [
                            "distance",
                            "angle",
                            "bbox_intersects",
                            "point_to_bbox_distance",
                            "bbox_center",
                            "entity_bbox",
                            "nearest_entities",
                            "text_anchor",
                            "segment_intersection"
                        ]
                    },
                    "payload": { "type": "object" }
                },
                "required": ["operation", "payload"]
            }),
            "full",
            "几何计算通常是结构化中间结果，建议保留完整 JSON。",
        )
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
        let Some(operation) = string_arg(args, "operation") else {
            return "错误：缺少必填参数 operation".to_string();
        };
        let payload = args.get("payload").cloned().unwrap_or_else(|| json!({}));
        let result = match operation.as_str() {
            "distance" => compute_distance(&payload),
            "angle" => compute_angle(&payload),
            "bbox_intersects" => compute_bbox_intersects(&payload),
            "point_to_bbox_distance" => compute_point_to_bbox_distance(&payload),
            "bbox_center" => compute_bbox_center(&payload),
            "entity_bbox" => compute_entity_bbox(&payload, context),
            "nearest_entities" => compute_nearest_entities(&payload, context),
            "text_anchor" => compute_text_anchor(&payload, context),
            "segment_intersection" => compute_segment_intersection(&payload),
            _ => Err(format!("错误：不支持的几何操作：{operation}")),
        };

        match result {
            Ok(value) => serde_json::to_string_pretty(&value)
                .unwrap_or_else(|error| format!("序列化几何结果失败：{error}")),
            Err(message) => message,
        }
    }
}

#[async_trait]
impl AgentTool for CadSaveReviewResultTool {
    fn name(&self) -> &'static str {
        "cad_save_review_result"
    }

    fn description(&self) -> &'static str {
        "同步一次 CAD 审查结果到 UI 问题清单；同一轮审查可复用 runId 持续更新同一份问题单。"
    }

    fn parameters(&self) -> Value {
        with_result_mode_parameter(
            json!({
                "type": "object",
                "properties": {
                    "runId": { "type": "string" },
                    "filePath": { "type": "string" },
                    "sourceMessageId": { "type": "string" },
                    "goal": { "type": "string" },
                    "status": { "type": "string" },
                    "summary": { "type": "string" },
                    "ruleAttachmentIds": { "type": "array", "items": { "type": "string" } },
                    "issues": { "type": "array", "items": { "type": "object" } }
                },
                "required": ["filePath", "sourceMessageId", "goal", "status", "summary", "issues"]
            }),
            "full",
            "审查结果会被保存为结构化 artifact；issue 应尽量附带 entityRefs、viewportHint、anchorPoint 或 bbox，以支持 UI 定位。",
        )
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
        let Some(raw_file_path) = string_arg(args, "filePath") else {
            return "错误：缺少必填参数 filePath".to_string();
        };
        let normalized_file_path = match normalize_path(&raw_file_path, context) {
            Ok(value) => value,
            Err(message) => return message,
        };
        let input = match serde_json::from_value::<CreateCadReviewRunInput>(json!({
            "runId": args.get("runId"),
            "workspaceId": context.workspace_id,
            "filePath": normalized_file_path,
            "sourceMessageId": args.get("sourceMessageId"),
            "ruleAttachmentIds": args.get("ruleAttachmentIds").cloned().unwrap_or_else(|| json!([])),
            "goal": args.get("goal"),
            "status": args.get("status"),
            "summary": args.get("summary"),
            "issues": args.get("issues").cloned().unwrap_or_else(|| json!([])),
        })) {
            Ok(value) => value,
            Err(error) => return format!("错误：审查结果参数无效：{error}"),
        };

        let db = match DispatcherDb::new(context.dispatcher_db_path.clone()) {
            Ok(db) => db,
            Err(error) => return format!("打开 dispatcher 数据库失败：{error}"),
        };
        let input = match enrich_review_input(input, &db, context) {
            Ok(value) => value,
            Err(error) => return error,
        };
        match db.create_cad_review_run(&input) {
            Ok(detail) => {
                if let Some(app_handle) = &context.app_handle {
                    let payload = json!({
                        "workspaceId": detail.run.workspace_id,
                        "filePath": detail.run.file_path,
                        "runId": detail.run.id,
                    });
                    if detail.run.created_at == detail.run.updated_at {
                        let _ = app_handle.emit("cad-review/run-created", payload);
                    } else {
                        let _ = app_handle.emit("cad-review/run-saved", payload);
                    }
                }
                let locatable_issue_count = detail
                    .issues
                    .iter()
                    .filter(|issue| {
                        !issue.entity_refs.is_empty()
                            || issue.viewport_hint.is_some()
                            || issue.anchor_point.is_some()
                            || issue.bbox.is_some()
                    })
                    .count();
                serde_json::to_string_pretty(&json!({
                    "status": "ok",
                    "message": format!("已同步 {} 条 CAD 审查问题到 UI，{} 条支持定位", detail.issues.len(), locatable_issue_count),
                    "run": detail.run,
                    "issues": detail.issues,
                }))
                .unwrap_or_else(|error| format!("序列化审查结果失败：{error}"))
            }
            Err(error) => format!("保存 CAD 审查结果失败：{error}"),
        }
    }
}

fn enrich_review_input(
    mut input: CreateCadReviewRunInput,
    db: &DispatcherDb,
    context: &ToolContext,
) -> Result<CreateCadReviewRunInput, String> {
    let document = resolve_review_document(&input.file_path, db, context);
    let viewer_session = context
        .dwg_viewer_bridge
        .as_ref()
        .and_then(|bridge| bridge.best_session_for_file(&context.workspace_id, &input.file_path));
    let mut issues = Vec::with_capacity(input.issues.len());
    for issue in input.issues {
        issues.push(enrich_review_issue(
            issue,
            document.as_ref(),
            viewer_session.as_ref(),
            db,
        )?);
    }
    input.issues = issues;
    Ok(input)
}

fn resolve_review_document(
    file_path: &str,
    db: &DispatcherDb,
    context: &ToolContext,
) -> Option<DwgDocumentRecord> {
    let metadata = fs::metadata(file_path).ok()?;
    let mtime = file_mtime(&metadata).ok()?;
    let project_path = context.workspace.to_string_lossy().into_owned();
    db.get_dwg_document(
        &project_path,
        file_path,
        metadata.len(),
        mtime,
        DEFAULT_PARSER_VERSION,
    )
    .ok()
    .flatten()
}

fn enrich_review_issue(
    mut issue: CreateCadReviewIssueInput,
    document: Option<&DwgDocumentRecord>,
    viewer_session: Option<&DwgViewerSessionState>,
    db: &DispatcherDb,
) -> Result<CreateCadReviewIssueInput, String> {
    issue.entity_refs = dedupe_string_vec(issue.entity_refs);
    let entity_details = load_issue_entity_details(&issue, document, db)?;

    if issue.layer.is_none() {
        issue.layer = unique_issue_layer(&entity_details);
    }
    if issue.bbox.is_none() {
        issue.bbox = merge_issue_bbox(&entity_details);
    }
    if issue.anchor_point.is_none() {
        issue.anchor_point =
            resolve_issue_anchor(&entity_details).or_else(|| issue.bbox.as_ref().map(bbox_center));
    }

    let mut viewport_hint = issue.viewport_hint.unwrap_or(CadViewportHint {
        center: None,
        bbox: None,
        zoom_scale: None,
    });
    if viewport_hint.center.is_none() {
        viewport_hint.center = issue.anchor_point.clone().or_else(|| {
            if issue.entity_refs.is_empty() {
                None
            } else {
                viewer_session.and_then(|session| session.center.clone())
            }
        });
    }
    if viewport_hint.bbox.is_none() {
        viewport_hint.bbox = issue.bbox.clone().or_else(|| {
            if issue.entity_refs.is_empty() {
                None
            } else {
                viewer_session.and_then(|session| session.viewport_box.clone())
            }
        });
    }
    if viewport_hint.zoom_scale.is_none() {
        viewport_hint.zoom_scale = viewer_session.and_then(|session| session.zoom_scale);
    }
    issue.viewport_hint =
        (viewport_hint.center.is_some() || viewport_hint.bbox.is_some()).then_some(viewport_hint);

    Ok(issue)
}

fn load_issue_entity_details(
    issue: &CreateCadReviewIssueInput,
    document: Option<&DwgDocumentRecord>,
    db: &DispatcherDb,
) -> Result<Vec<CadEntityDetail>, String> {
    if issue.entity_refs.is_empty() {
        return Ok(Vec::new());
    }
    let Some(document) = document else {
        return Ok(Vec::new());
    };
    db.get_dwg_entity_details(&document.id, &issue.entity_refs)
        .map_err(|error| format!("读取审查问题实体定位信息失败：{error}"))
}

fn unique_issue_layer(details: &[CadEntityDetail]) -> Option<String> {
    let layers = details
        .iter()
        .map(|detail| detail.envelope.layer.trim())
        .filter(|layer| !layer.is_empty())
        .collect::<BTreeSet<_>>();
    (layers.len() == 1).then(|| layers.iter().next().unwrap_or(&"").to_string())
}

fn merge_issue_bbox(details: &[CadEntityDetail]) -> Option<CadBBox> {
    let mut merged: Option<CadBBox> = None;
    for detail in details {
        let Some(bbox) = detail.envelope.bbox.as_ref() else {
            continue;
        };
        merged = Some(match merged {
            Some(current) => CadBBox {
                min_x: current.min_x.min(bbox.min_x),
                min_y: current.min_y.min(bbox.min_y),
                max_x: current.max_x.max(bbox.max_x),
                max_y: current.max_y.max(bbox.max_y),
            },
            None => bbox.clone(),
        });
    }
    merged
}

fn resolve_issue_anchor(details: &[CadEntityDetail]) -> Option<CadPoint> {
    details.iter().find_map(|detail| {
        detail
            .envelope
            .anchor
            .clone()
            .or_else(|| detail.envelope.center.clone())
            .or_else(|| detail.envelope.bbox.as_ref().map(bbox_center))
    })
}

fn dedupe_string_vec(values: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut deduped = Vec::new();
    for value in values {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            continue;
        }
        if seen.insert(trimmed.to_string()) {
            deduped.push(trimmed.to_string());
        }
    }
    deduped
}

fn load_cache(
    path: &str,
    parser_version: &str,
    context: &ToolContext,
) -> Result<DwgParseCacheRecord, String> {
    let file_path = normalize_path(path, context)?;
    let metadata =
        fs::metadata(&file_path).map_err(|error| format!("读取 DWG 文件元数据失败：{error}"))?;
    let db = DispatcherDb::new(context.dispatcher_db_path.clone())
        .map_err(|error| format!("打开 dispatcher 数据库失败：{error}"))?;
    db.get_dwg_parse_cache(
        &context.workspace.to_string_lossy(),
        &file_path,
        metadata.len(),
        file_mtime(&metadata)?,
        parser_version,
    )
    .map_err(|error| format!("读取 DWG 解析缓存失败：{error}"))?
    .ok_or_else(|| "错误：未找到 DWG 解析缓存，请先在文件预览中打开该 DWG".to_string())
}

fn normalize_path(path: &str, context: &ToolContext) -> Result<String, String> {
    let raw = std::path::PathBuf::from(path);
    let joined = if raw.is_absolute() {
        raw
    } else {
        context.workspace.join(raw)
    };
    let normalized = joined
        .canonicalize()
        .map_err(|error| format!("解析 DWG 路径失败：{error}"))?;
    if context.restrict_to_workspace && !normalized.starts_with(&context.workspace) {
        return Err(format!("错误：禁止访问工作区之外的 DWG 路径：{path}"));
    }
    Ok(normalized.to_string_lossy().into_owned())
}

fn file_mtime(metadata: &fs::Metadata) -> Result<i64, String> {
    let modified = metadata
        .modified()
        .map_err(|error| format!("读取 DWG 修改时间失败：{error}"))?;
    let duration = modified
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| format!("DWG 修改时间无效：{error}"))?;
    Ok(duration.as_secs() as i64)
}

fn parse_point(value: &Value, key: &str) -> Result<CadPoint, String> {
    serde_json::from_value(
        value
            .get(key)
            .cloned()
            .ok_or_else(|| format!("错误：payload 缺少 {key}"))?,
    )
    .map_err(|error| format!("错误：{key} 点位无效：{error}"))
}

fn parse_bbox(value: &Value, key: &str) -> Result<CadBBox, String> {
    serde_json::from_value(
        value
            .get(key)
            .cloned()
            .ok_or_else(|| format!("错误：payload 缺少 {key}"))?,
    )
    .map_err(|error| format!("错误：{key} 包围盒无效：{error}"))
}

fn parse_point_value(value: &Value) -> Result<CadPoint, String> {
    serde_json::from_value(value.clone()).map_err(|error| format!("错误：点位无效：{error}"))
}

fn compute_distance(payload: &Value) -> Result<Value, String> {
    let start = parse_point(payload, "start")?;
    let end = parse_point(payload, "end")?;
    let dx = end.x - start.x;
    let dy = end.y - start.y;
    Ok(json!({
        "operation": "distance",
        "distance": (dx * dx + dy * dy).sqrt(),
        "dx": dx,
        "dy": dy,
    }))
}

fn compute_angle(payload: &Value) -> Result<Value, String> {
    let start = parse_point(payload, "start")?;
    let end = parse_point(payload, "end")?;
    let angle = (end.y - start.y).atan2(end.x - start.x);
    Ok(json!({
        "operation": "angle",
        "radians": angle,
        "degrees": angle.to_degrees(),
    }))
}

fn compute_bbox_intersects(payload: &Value) -> Result<Value, String> {
    let left = parse_bbox(payload, "left")?;
    let right = parse_bbox(payload, "right")?;
    Ok(json!({
        "operation": "bbox_intersects",
        "intersects": bbox_intersects(&left, &right),
    }))
}

fn compute_point_to_bbox_distance(payload: &Value) -> Result<Value, String> {
    let point = parse_point(payload, "point")?;
    let bbox = parse_bbox(payload, "bbox")?;
    let dx = if point.x < bbox.min_x {
        bbox.min_x - point.x
    } else if point.x > bbox.max_x {
        point.x - bbox.max_x
    } else {
        0.0
    };
    let dy = if point.y < bbox.min_y {
        bbox.min_y - point.y
    } else if point.y > bbox.max_y {
        point.y - bbox.max_y
    } else {
        0.0
    };
    Ok(json!({
        "operation": "point_to_bbox_distance",
        "distance": (dx * dx + dy * dy).sqrt(),
        "dx": dx,
        "dy": dy,
    }))
}

fn compute_bbox_center(payload: &Value) -> Result<Value, String> {
    let bbox = parse_bbox(payload, "bbox")?;
    Ok(json!({
        "operation": "bbox_center",
        "center": bbox_center(&bbox),
    }))
}

fn compute_entity_bbox(payload: &Value, context: &ToolContext) -> Result<Value, String> {
    let entity_id =
        string_arg(payload, "entityId").ok_or_else(|| "错误：payload 缺少 entityId".to_string())?;
    let cache = load_payload_cache(payload, context)?;
    let Some(entity) = find_entity(&cache.entities, &entity_id) else {
        return Ok(json!({
            "operation": "entity_bbox",
            "entityId": entity_id,
            "supported": false,
            "reason": "entity_not_found",
        }));
    };

    if let Some(bbox) = entity.bbox.clone() {
        return Ok(json!({
            "operation": "entity_bbox",
            "entityId": entity_id,
            "supported": true,
            "bbox": bbox,
        }));
    }

    Ok(json!({
        "operation": "entity_bbox",
        "entityId": entity_id,
        "supported": false,
        "reason": "bbox_unavailable",
    }))
}

fn compute_nearest_entities(payload: &Value, context: &ToolContext) -> Result<Value, String> {
    let point = parse_point(payload, "point")?;
    let limit = usize_arg(payload, "limit").unwrap_or(10).clamp(1, 100);
    let filters = payload
        .get("filters")
        .cloned()
        .map(serde_json::from_value::<CadEntityQueryFilters>)
        .transpose()
        .map_err(|error| format!("错误：filters 无效：{error}"))?
        .unwrap_or_default();
    let cache = load_payload_cache(payload, context)?;

    let items = filter_entities(&cache.entities, &filters)
        .into_iter()
        .filter_map(|entity| {
            entity_anchor(&entity).map(|anchor| {
                let dx = anchor.x - point.x;
                let dy = anchor.y - point.y;
                (
                    dx * dx + dy * dy,
                    json!({
                        "entityId": entity.id,
                        "distance": (dx * dx + dy * dy).sqrt(),
                        "anchor": anchor,
                        "entity": entity,
                    }),
                )
            })
        })
        .collect::<Vec<_>>();

    let mut items = items;
    items.sort_by(|left, right| left.0.total_cmp(&right.0));

    Ok(json!({
        "operation": "nearest_entities",
        "point": point,
        "total": items.len(),
        "items": items
            .into_iter()
            .take(limit)
            .map(|(_, item)| item)
            .collect::<Vec<_>>(),
    }))
}

fn compute_text_anchor(payload: &Value, context: &ToolContext) -> Result<Value, String> {
    let entity_id =
        string_arg(payload, "entityId").ok_or_else(|| "错误：payload 缺少 entityId".to_string())?;
    let cache = load_payload_cache(payload, context)?;
    let Some(entity) = find_entity(&cache.entities, &entity_id) else {
        return Ok(json!({
            "operation": "text_anchor",
            "entityId": entity_id,
            "supported": false,
            "reason": "entity_not_found",
        }));
    };

    let Some(anchor) = entity_anchor(entity) else {
        return Ok(json!({
            "operation": "text_anchor",
            "entityId": entity_id,
            "supported": false,
            "reason": "anchor_unavailable",
        }));
    };

    Ok(json!({
        "operation": "text_anchor",
        "entityId": entity_id,
        "supported": true,
        "anchor": anchor,
        "text": entity.text,
    }))
}

fn compute_segment_intersection(payload: &Value) -> Result<Value, String> {
    let left = parse_segment(payload, "left")?;
    let right = parse_segment(payload, "right")?;
    let Some((point, left_t, right_t)) =
        segment_intersection_point(&left.0, &left.1, &right.0, &right.1)
    else {
        let parallel = cross(subtract(&left.1, &left.0), subtract(&right.1, &right.0)).abs() < 1e-9;
        return Ok(json!({
            "operation": "segment_intersection",
            "intersects": false,
            "relation": if parallel { "parallel_or_colinear" } else { "disjoint" },
        }));
    };

    Ok(json!({
        "operation": "segment_intersection",
        "intersects": true,
        "relation": "point",
        "point": point,
        "leftT": left_t,
        "rightT": right_t,
    }))
}

fn load_payload_cache(
    payload: &Value,
    context: &ToolContext,
) -> Result<DwgParseCacheRecord, String> {
    let path = string_arg(payload, "path").ok_or_else(|| "错误：payload 缺少 path".to_string())?;
    let parser_version =
        string_arg(payload, "parserVersion").unwrap_or_else(|| DEFAULT_PARSER_VERSION.to_string());
    load_cache(&path, &parser_version, context)
}

fn find_entity<'a>(
    entities: &'a [CadEntityRecord],
    entity_id: &str,
) -> Option<&'a CadEntityRecord> {
    entities
        .iter()
        .find(|entity| entity.id == entity_id || entity.handle == entity_id)
}

fn bbox_center(bbox: &CadBBox) -> CadPoint {
    CadPoint {
        x: (bbox.min_x + bbox.max_x) / 2.0,
        y: (bbox.min_y + bbox.max_y) / 2.0,
    }
}

fn entity_anchor(entity: &CadEntityRecord) -> Option<CadPoint> {
    entity
        .center
        .clone()
        .or_else(|| entity.bbox.as_ref().map(bbox_center))
        .or_else(|| entity.vertices.first().cloned())
}

fn parse_segment(payload: &Value, key: &str) -> Result<(CadPoint, CadPoint), String> {
    let raw = payload
        .get(key)
        .ok_or_else(|| format!("错误：payload 缺少 {key}"))?;
    let start = raw
        .get("start")
        .ok_or_else(|| format!("错误：{key} 缺少 start"))
        .and_then(parse_point_value)?;
    let end = raw
        .get("end")
        .ok_or_else(|| format!("错误：{key} 缺少 end"))
        .and_then(parse_point_value)?;
    Ok((start, end))
}

fn subtract(left: &CadPoint, right: &CadPoint) -> CadPoint {
    CadPoint {
        x: left.x - right.x,
        y: left.y - right.y,
    }
}

fn cross(left: CadPoint, right: CadPoint) -> f64 {
    left.x * right.y - left.y * right.x
}

fn segment_intersection_point(
    left_start: &CadPoint,
    left_end: &CadPoint,
    right_start: &CadPoint,
    right_end: &CadPoint,
) -> Option<(CadPoint, f64, f64)> {
    let left_vector = subtract(left_end, left_start);
    let right_vector = subtract(right_end, right_start);
    let denominator = cross(left_vector.clone(), right_vector.clone());
    if denominator.abs() < 1e-9 {
        return None;
    }

    let start_delta = subtract(right_start, left_start);
    let left_t = cross(start_delta.clone(), right_vector) / denominator;
    let right_t = cross(start_delta, left_vector) / denominator;
    if !(0.0..=1.0).contains(&left_t) || !(0.0..=1.0).contains(&right_t) {
        return None;
    }

    Some((
        CadPoint {
            x: left_start.x + (left_end.x - left_start.x) * left_t,
            y: left_start.y + (left_end.y - left_start.y) * left_t,
        },
        left_t,
        right_t,
    ))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        bbox_center, compute_bbox_center, compute_segment_intersection, entity_anchor,
        segment_intersection_point,
    };
    use crate::agent::cad::{CadBBox, CadEntityRecord, CadPoint};

    fn sample_entity() -> CadEntityRecord {
        CadEntityRecord {
            id: "E1".to_string(),
            handle: "E1".to_string(),
            entity_type: "TEXT".to_string(),
            raw_type: "TEXT".to_string(),
            layer: "TEXT".to_string(),
            color: None,
            line_type: None,
            text: Some("房间名称".to_string()),
            block_name: None,
            center: Some(CadPoint { x: 12.0, y: 8.0 }),
            radius: None,
            vertices: Vec::new(),
            bbox: Some(CadBBox {
                min_x: 10.0,
                min_y: 6.0,
                max_x: 14.0,
                max_y: 10.0,
            }),
        }
    }

    #[test]
    fn bbox_center_returns_midpoint() {
        let point = bbox_center(&CadBBox {
            min_x: -2.0,
            min_y: 2.0,
            max_x: 6.0,
            max_y: 10.0,
        });
        assert_eq!(point, CadPoint { x: 2.0, y: 6.0 });
    }

    #[test]
    fn entity_anchor_prefers_center_then_bbox_then_vertex() {
        let entity = sample_entity();
        assert_eq!(entity_anchor(&entity), entity.center);
    }

    #[test]
    fn compute_bbox_center_serializes_center() {
        let result = compute_bbox_center(&json!({
            "bbox": { "minX": 0.0, "minY": 0.0, "maxX": 8.0, "maxY": 4.0 }
        }))
        .expect("bbox center");
        assert_eq!(result["center"]["x"], json!(4.0));
        assert_eq!(result["center"]["y"], json!(2.0));
    }

    #[test]
    fn segment_intersection_returns_point_for_crossing_segments() {
        let result = compute_segment_intersection(&json!({
            "left": {
                "start": { "x": 0.0, "y": 0.0 },
                "end": { "x": 10.0, "y": 10.0 }
            },
            "right": {
                "start": { "x": 0.0, "y": 10.0 },
                "end": { "x": 10.0, "y": 0.0 }
            }
        }))
        .expect("segment intersection");
        assert_eq!(result["intersects"], json!(true));
        assert_eq!(result["point"]["x"], json!(5.0));
        assert_eq!(result["point"]["y"], json!(5.0));
    }

    #[test]
    fn segment_intersection_point_returns_none_for_parallel_segments() {
        let left_start = CadPoint { x: 0.0, y: 0.0 };
        let left_end = CadPoint { x: 10.0, y: 0.0 };
        let right_start = CadPoint { x: 0.0, y: 2.0 };
        let right_end = CadPoint { x: 10.0, y: 2.0 };
        assert!(
            segment_intersection_point(&left_start, &left_end, &right_start, &right_end).is_none()
        );
    }
}
