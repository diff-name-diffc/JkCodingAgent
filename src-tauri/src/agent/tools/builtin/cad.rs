use std::fs;

use async_trait::async_trait;
use serde_json::{json, Value};

use super::common::{string_arg, usize_arg, with_result_mode_parameter};
use crate::agent::cad::{
    bbox_intersects, filter_entities, CadBBox, CadEntityQueryFilters, CadEntityRecord, CadPoint,
    CreateCadReviewRunInput, DwgParseCacheRecord,
};
use crate::agent::db::DispatcherDb;
use crate::agent::tools::context::ToolContext;
use crate::agent::tools::registry::AgentTool;

const DEFAULT_PARSER_VERSION: &str = "dwg-worker-v1";

pub(super) fn cad_get_dwg_summary_tool() -> Box<dyn AgentTool> {
    Box::new(CadGetDwgSummaryTool)
}

pub(super) fn cad_query_dwg_entities_tool() -> Box<dyn AgentTool> {
    Box::new(CadQueryDwgEntitiesTool)
}

pub(super) fn cad_compute_geometry_tool() -> Box<dyn AgentTool> {
    Box::new(CadComputeGeometryTool)
}

pub(super) fn cad_save_review_result_tool() -> Box<dyn AgentTool> {
    Box::new(CadSaveReviewResultTool)
}

struct CadGetDwgSummaryTool;
struct CadQueryDwgEntitiesTool;
struct CadComputeGeometryTool;
struct CadSaveReviewResultTool;

#[async_trait]
impl AgentTool for CadGetDwgSummaryTool {
    fn name(&self) -> &'static str {
        "cad_get_dwg_summary"
    }

    fn description(&self) -> &'static str {
        "读取指定 DWG 的解析缓存摘要，返回图层、范围、实体计数、文字样本与块引用摘要。若缓存不存在，请先在文件预览中打开该 DWG。"
    }

    fn parameters(&self) -> Value {
        with_result_mode_parameter(
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "DWG 文件路径" },
                    "parserVersion": { "type": "string", "description": "解析器版本，默认 dwg-worker-v1" }
                },
                "required": ["path"]
            }),
            "full",
            "CAD 摘要通常需要原样保留，便于模型后续按图层和实体类型做审查。",
        )
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
        let Some(path) = string_arg(args, "path") else {
            return "错误：缺少必填参数 path".to_string();
        };
        let parser_version =
            string_arg(args, "parserVersion").unwrap_or_else(|| DEFAULT_PARSER_VERSION.to_string());
        match load_cache(&path, &parser_version, context) {
            Ok(cache) => serde_json::to_string_pretty(&cache.summary)
                .unwrap_or_else(|error| format!("序列化摘要失败：{error}")),
            Err(message) => message,
        }
    }
}

#[async_trait]
impl AgentTool for CadQueryDwgEntitiesTool {
    fn name(&self) -> &'static str {
        "cad_query_dwg_entities"
    }

    fn description(&self) -> &'static str {
        "按图层、实体类型、文字关键词、范围分页查询 DWG 解析缓存中的实体，避免一次性暴露整图实体。"
    }

    fn parameters(&self) -> Value {
        with_result_mode_parameter(
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "DWG 文件路径" },
                    "parserVersion": { "type": "string", "description": "解析器版本，默认 dwg-worker-v1" },
                    "cursor": { "type": "integer", "description": "分页游标，默认 0", "minimum": 0 },
                    "limit": { "type": "integer", "description": "分页大小，默认 50", "minimum": 1 },
                    "filters": {
                        "type": "object",
                        "properties": {
                            "layers": { "type": "array", "items": { "type": "string" } },
                            "entityTypes": { "type": "array", "items": { "type": "string" } },
                            "textQuery": { "type": "string" },
                            "bbox": {
                                "type": "object",
                                "properties": {
                                    "minX": { "type": "number" },
                                    "minY": { "type": "number" },
                                    "maxX": { "type": "number" },
                                    "maxY": { "type": "number" }
                                },
                                "required": ["minX", "minY", "maxX", "maxY"]
                            }
                        }
                    }
                },
                "required": ["path"]
            }),
            "full",
            "实体查询结果是渐进式审查上下文，建议保留完整 JSON。",
        )
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
        let Some(path) = string_arg(args, "path") else {
            return "错误：缺少必填参数 path".to_string();
        };
        let parser_version =
            string_arg(args, "parserVersion").unwrap_or_else(|| DEFAULT_PARSER_VERSION.to_string());
        let cursor = usize_arg(args, "cursor").unwrap_or(0);
        let limit = usize_arg(args, "limit").unwrap_or(50).clamp(1, 500);
        let filters = args
            .get("filters")
            .cloned()
            .map(serde_json::from_value::<CadEntityQueryFilters>)
            .transpose()
            .unwrap_or_else(|_| Some(CadEntityQueryFilters::default()))
            .unwrap_or_default();

        match load_cache(&path, &parser_version, context) {
            Ok(cache) => {
                let db = match DispatcherDb::new(context.dispatcher_db_path.clone()) {
                    Ok(db) => db,
                    Err(error) => return format!("打开 dispatcher 数据库失败：{error}"),
                };
                match db.query_dwg_parse_entities(
                    &context.workspace.to_string_lossy(),
                    &cache.file_path,
                    cache.file_size,
                    cache.file_mtime,
                    &parser_version,
                    &filters,
                    cursor,
                    limit,
                ) {
                    Ok(Some(result)) => serde_json::to_string_pretty(&result)
                        .unwrap_or_else(|error| format!("序列化查询结果失败：{error}")),
                    Ok(None) => "错误：未找到 DWG 解析缓存，请先在文件预览中打开该 DWG".to_string(),
                    Err(error) => format!("查询 DWG 实体失败：{error}"),
                }
            }
            Err(message) => message,
        }
    }
}

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
        "持久化一次 CAD 审查结果和问题清单，并回传可用于前端联动的运行记录。"
    }

    fn parameters(&self) -> Value {
        with_result_mode_parameter(
            json!({
                "type": "object",
                "properties": {
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
            "审查结果会被保存为结构化 artifact，建议保留完整 JSON。",
        )
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
        let input = match serde_json::from_value::<CreateCadReviewRunInput>(json!({
            "workspaceId": context.workspace_id,
            "filePath": args.get("filePath"),
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
        match db.create_cad_review_run(&input) {
            Ok(detail) => serde_json::to_string_pretty(&json!({
                "status": "ok",
                "message": format!("已保存 {} 条 CAD 审查问题", detail.issues.len()),
                "run": detail.run,
                "issues": detail.issues,
            }))
            .unwrap_or_else(|error| format!("序列化审查结果失败：{error}")),
            Err(error) => format!("保存 CAD 审查结果失败：{error}"),
        }
    }
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
