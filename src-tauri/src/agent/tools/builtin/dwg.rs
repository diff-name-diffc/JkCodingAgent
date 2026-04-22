use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use super::common::{string_arg, usize_arg, with_result_mode_parameter};
use crate::agent::cad::{
    CadBBox, CadEntityQueryFilters, CadPoint, CadReviewIssueRecord, DwgIssueMarker,
    DwgViewerSessionState,
};
use crate::agent::db::DispatcherDb;
use crate::agent::tools::context::ToolContext;
use crate::agent::tools::registry::AgentTool;

const DEFAULT_PARSER_VERSION: &str = "dwg-worker-v1";

pub(super) fn cad_ensure_dwg_index_tool() -> Box<dyn AgentTool> {
    Box::new(CadEnsureDwgIndexTool)
}

pub(super) fn cad_get_dwg_overview_tool() -> Box<dyn AgentTool> {
    Box::new(CadGetDwgOverviewTool)
}

pub(super) fn cad_get_dwg_summary_tool() -> Box<dyn AgentTool> {
    Box::new(CadGetDwgSummaryTool)
}

pub(super) fn cad_list_dwg_layers_tool() -> Box<dyn AgentTool> {
    Box::new(CadListDwgLayersTool)
}

pub(super) fn cad_query_dwg_entities_tool() -> Box<dyn AgentTool> {
    Box::new(CadQueryDwgEntitiesTool)
}

pub(super) fn cad_get_dwg_entity_detail_tool() -> Box<dyn AgentTool> {
    Box::new(CadGetDwgEntityDetailTool)
}

pub(super) fn cad_inspect_dwg_region_tool() -> Box<dyn AgentTool> {
    Box::new(CadInspectDwgRegionTool)
}

pub(super) fn cad_get_dwg_viewer_session_tool() -> Box<dyn AgentTool> {
    Box::new(CadGetDwgViewerSessionTool)
}

pub(super) fn cad_get_dwg_viewport_tool() -> Box<dyn AgentTool> {
    Box::new(CadGetDwgViewportTool)
}

pub(super) fn cad_set_dwg_issue_markers_tool() -> Box<dyn AgentTool> {
    Box::new(CadSetDwgIssueMarkersTool)
}

pub(super) fn cad_clear_dwg_issue_markers_tool() -> Box<dyn AgentTool> {
    Box::new(CadClearDwgIssueMarkersTool)
}

pub(super) fn cad_control_dwg_viewer_tool() -> Box<dyn AgentTool> {
    Box::new(CadControlDwgViewerTool)
}

pub(super) fn cad_pick_dwg_viewer_tool() -> Box<dyn AgentTool> {
    Box::new(CadPickDwgViewerTool)
}

pub(super) fn cad_capture_dwg_viewer_tool() -> Box<dyn AgentTool> {
    Box::new(CadCaptureDwgViewerTool)
}

struct CadEnsureDwgIndexTool;
struct CadGetDwgOverviewTool;
struct CadGetDwgSummaryTool;
struct CadListDwgLayersTool;
struct CadQueryDwgEntitiesTool;
struct CadGetDwgEntityDetailTool;
struct CadInspectDwgRegionTool;
struct CadGetDwgViewerSessionTool;
struct CadGetDwgViewportTool;
struct CadSetDwgIssueMarkersTool;
struct CadClearDwgIssueMarkersTool;
struct CadControlDwgViewerTool;
struct CadPickDwgViewerTool;
struct CadCaptureDwgViewerTool;

#[async_trait]
impl AgentTool for CadEnsureDwgIndexTool {
    fn name(&self) -> &'static str {
        "cad_ensure_dwg_index"
    }

    fn description(&self) -> &'static str {
        "确保指定 DWG 已建立可渐进查询的索引；必要时自动打开或复用 DWG 工作台触发解析。"
    }

    fn parameters(&self) -> Value {
        with_result_mode_parameter(
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "parserVersion": { "type": "string" },
                    "refreshPolicy": {
                        "type": "string",
                        "enum": ["if_missing_or_stale", "always", "never"]
                    },
                    "openViewerIfNeeded": { "type": "boolean" }
                },
                "required": ["path"]
            }),
            "full",
            "索引确保结果需要保留 docId 与状态。",
        )
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
        let Some(path) = string_arg(args, "path") else {
            return "错误：缺少必填参数 path".to_string();
        };
        let parser_version =
            string_arg(args, "parserVersion").unwrap_or_else(|| DEFAULT_PARSER_VERSION.to_string());
        let refresh_policy =
            string_arg(args, "refreshPolicy").unwrap_or_else(|| "if_missing_or_stale".to_string());
        let open_viewer = args
            .get("openViewerIfNeeded")
            .and_then(Value::as_bool)
            .unwrap_or(true);

        match ensure_index(
            &path,
            &parser_version,
            &refresh_policy,
            open_viewer,
            context,
        )
        .await
        {
            Ok(value) => serialize(value),
            Err(message) => message,
        }
    }
}

#[async_trait]
impl AgentTool for CadGetDwgOverviewTool {
    fn name(&self) -> &'static str {
        "cad_get_dwg_overview"
    }

    fn description(&self) -> &'static str {
        "返回 DWG 顶层概览，包含范围、图层统计、实体类型统计与建议的下一步探索动作。"
    }

    fn parameters(&self) -> Value {
        overview_parameters()
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
        match load_overview(args, context).await {
            Ok(value) => serialize(value),
            Err(message) => message,
        }
    }
}

#[async_trait]
impl AgentTool for CadGetDwgSummaryTool {
    fn name(&self) -> &'static str {
        "cad_get_dwg_summary"
    }

    fn description(&self) -> &'static str {
        "兼容旧工具名，返回 DWG 概览摘要。"
    }

    fn parameters(&self) -> Value {
        overview_parameters()
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
        match load_overview(args, context)
            .await
            .map(|overview| overview.document.summary)
        {
            Ok(value) => serialize(value),
            Err(message) => message,
        }
    }
}

#[async_trait]
impl AgentTool for CadListDwgLayersTool {
    fn name(&self) -> &'static str {
        "cad_list_dwg_layers"
    }

    fn description(&self) -> &'static str {
        "按图层分页返回 DWG 统计，帮助模型先从顶层结构缩小范围。"
    }

    fn parameters(&self) -> Value {
        with_result_mode_parameter(
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "docId": { "type": "string" },
                    "parserVersion": { "type": "string" },
                    "cursor": { "type": "integer", "minimum": 0 },
                    "limit": { "type": "integer", "minimum": 1 },
                    "sortBy": { "type": "string", "enum": ["entityCount", "name"] }
                },
                "required": ["path"]
            }),
            "full",
            "图层列表应保留计数与 nextCursor。",
        )
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
        let cursor = usize_arg(args, "cursor").unwrap_or(0);
        let limit = usize_arg(args, "limit").unwrap_or(20).clamp(1, 100);
        let sort_by = string_arg(args, "sortBy").unwrap_or_else(|| "entityCount".to_string());
        let db = match dispatcher_db(context) {
            Ok(value) => value,
            Err(message) => return message,
        };
        let document = match resolve_document(&db, args, context, false).await {
            Ok(value) => value,
            Err(message) => return message,
        };
        match db.list_dwg_layers(
            &document.id,
            cursor,
            limit,
            if sort_by == "name" {
                "name"
            } else {
                "entityCount"
            },
        ) {
            Ok(value) => serialize(value),
            Err(error) => format!("列出 DWG 图层失败：{error}"),
        }
    }
}

#[async_trait]
impl AgentTool for CadQueryDwgEntitiesTool {
    fn name(&self) -> &'static str {
        "cad_query_dwg_entities"
    }

    fn description(&self) -> &'static str {
        "按图层、类型、文字、块名和包围盒分页查询 DWG 实体包络，不直接暴露大 payload。"
    }

    fn parameters(&self) -> Value {
        with_result_mode_parameter(
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "docId": { "type": "string" },
                    "parserVersion": { "type": "string" },
                    "cursor": { "type": "integer", "minimum": 0 },
                    "limit": { "type": "integer", "minimum": 1 },
                    "filters": {
                        "type": "object",
                        "properties": {
                            "layers": { "type": "array", "items": { "type": "string" } },
                            "entityTypes": { "type": "array", "items": { "type": "string" } },
                            "textQuery": { "type": "string" },
                            "blockName": { "type": "string" },
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
            "查询结果只返回 envelope，便于渐进式探索。",
        )
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
        let cursor = usize_arg(args, "cursor").unwrap_or(0);
        let limit = usize_arg(args, "limit").unwrap_or(50).clamp(1, 100);
        let filters = args
            .get("filters")
            .cloned()
            .map(serde_json::from_value::<CadEntityQueryFilters>)
            .transpose()
            .unwrap_or_else(|_| Some(CadEntityQueryFilters::default()))
            .unwrap_or_default();
        let db = match dispatcher_db(context) {
            Ok(value) => value,
            Err(message) => return message,
        };
        let document = match resolve_document(&db, args, context, false).await {
            Ok(value) => value,
            Err(message) => return message,
        };
        match db.query_dwg_entities(&document.id, &filters, cursor, limit) {
            Ok(value) => serialize(value),
            Err(error) => format!("查询 DWG 实体失败：{error}"),
        }
    }
}

#[async_trait]
impl AgentTool for CadGetDwgEntityDetailTool {
    fn name(&self) -> &'static str {
        "cad_get_dwg_entity_detail"
    }

    fn description(&self) -> &'static str {
        "只对少量实体拉取 envelope 与 payload 细节，避免一次性展开整图。"
    }

    fn parameters(&self) -> Value {
        with_result_mode_parameter(
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "docId": { "type": "string" },
                    "parserVersion": { "type": "string" },
                    "entityIds": { "type": "array", "items": { "type": "string" }, "minItems": 1 }
                },
                "required": ["path", "entityIds"]
            }),
            "full",
            "实体明细数量应保持较小。",
        )
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
        let entity_ids = args
            .get("entityIds")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if entity_ids.is_empty() {
            return "错误：缺少 entityIds".to_string();
        }
        if entity_ids.len() > 20 {
            return "错误：entityIds 最多允许 20 个，请先缩小范围".to_string();
        }
        let db = match dispatcher_db(context) {
            Ok(value) => value,
            Err(message) => return message,
        };
        let document = match resolve_document(&db, args, context, false).await {
            Ok(value) => value,
            Err(message) => return message,
        };
        match db.get_dwg_entity_details(&document.id, &entity_ids) {
            Ok(value) => serialize(value),
            Err(error) => format!("读取 DWG 实体明细失败：{error}"),
        }
    }
}

#[async_trait]
impl AgentTool for CadInspectDwgRegionTool {
    fn name(&self) -> &'static str {
        "cad_inspect_dwg_region"
    }

    fn description(&self) -> &'static str {
        "围绕 bbox 或 point+radius 做区域级检查，返回统计、文字样本与少量实体样本。"
    }

    fn parameters(&self) -> Value {
        with_result_mode_parameter(
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "docId": { "type": "string" },
                    "parserVersion": { "type": "string" },
                    "bbox": {
                        "type": "object",
                        "properties": {
                            "minX": { "type": "number" },
                            "minY": { "type": "number" },
                            "maxX": { "type": "number" },
                            "maxY": { "type": "number" }
                        },
                        "required": ["minX", "minY", "maxX", "maxY"]
                    },
                    "point": {
                        "type": "object",
                        "properties": {
                            "x": { "type": "number" },
                            "y": { "type": "number" }
                        },
                        "required": ["x", "y"]
                    },
                    "radius": { "type": "number", "minimum": 0 },
                    "groupBy": { "type": "string", "enum": ["layer", "entityType"] },
                    "sampleLimit": { "type": "integer", "minimum": 1 }
                },
                "required": ["path"]
            }),
            "full",
            "区域检查结果默认包含建议的下一步动作。",
        )
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
        let bbox = match parse_region_bbox(args) {
            Ok(value) => value,
            Err(message) => return message,
        };
        let group_by = string_arg(args, "groupBy").unwrap_or_else(|| "layer".to_string());
        let sample_limit = usize_arg(args, "sampleLimit").unwrap_or(30).clamp(1, 100);
        let db = match dispatcher_db(context) {
            Ok(value) => value,
            Err(message) => return message,
        };
        let document = match resolve_document(&db, args, context, false).await {
            Ok(value) => value,
            Err(message) => return message,
        };
        match db.inspect_dwg_region(
            &document.id,
            &bbox,
            if group_by == "entityType" {
                "entityType"
            } else {
                "layer"
            },
            sample_limit,
        ) {
            Ok(value) => serialize(value),
            Err(error) => format!("区域检查失败：{error}"),
        }
    }
}

#[async_trait]
impl AgentTool for CadGetDwgViewerSessionTool {
    fn name(&self) -> &'static str {
        "cad_get_dwg_viewer_session"
    }

    fn description(&self) -> &'static str {
        "查询、复用或自动打开指定 DWG 的 viewer 会话。"
    }

    fn parameters(&self) -> Value {
        with_result_mode_parameter(
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "policy": {
                        "type": "string",
                        "enum": ["prefer_active", "reuse_any", "open_if_missing"]
                    }
                },
                "required": ["path"]
            }),
            "full",
            "返回 viewer 会话与状态快照。",
        )
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
        let Some(path) = string_arg(args, "path") else {
            return "错误：缺少必填参数 path".to_string();
        };
        let policy = string_arg(args, "policy").unwrap_or_else(|| "prefer_active".to_string());
        match resolve_session_for_path(&path, &policy, context).await {
            Ok((session, opened_by_tool)) => serialize(json!({
                "sessionId": session.session_id,
                "status": "ready",
                "openedByTool": opened_by_tool,
                "stateSnapshot": session,
            })),
            Err(message) => message,
        }
    }
}

#[async_trait]
impl AgentTool for CadGetDwgViewportTool {
    fn name(&self) -> &'static str {
        "cad_get_dwg_viewport"
    }

    fn description(&self) -> &'static str {
        "读取当前 viewer 视口状态，并可附带当前窗口区域内的少量实体样本。"
    }

    fn parameters(&self) -> Value {
        with_result_mode_parameter(
            json!({
                "type": "object",
                "properties": {
                    "sessionId": { "type": "string" },
                    "includeVisibleEntities": { "type": "boolean" },
                    "sampleLimit": { "type": "integer", "minimum": 1 }
                },
                "required": ["sessionId"]
            }),
            "full",
            "视口结果默认返回轻量状态快照。",
        )
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
        let Some(session_id) = string_arg(args, "sessionId") else {
            return "错误：缺少必填参数 sessionId".to_string();
        };
        let Some(bridge) = &context.dwg_viewer_bridge else {
            return "错误：当前环境不支持 DWG Viewer bridge".to_string();
        };
        let Some(session) = bridge.get_session(&session_id) else {
            return format!("错误：未找到 DWG Viewer 会话：{session_id}");
        };
        let include_visible = args
            .get("includeVisibleEntities")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let sample_limit = usize_arg(args, "sampleLimit").unwrap_or(20).clamp(1, 50);
        let visible_entities = if include_visible {
            load_visible_entities(context, &session, sample_limit)
        } else {
            None
        };
        serialize(json!({
            "sessionId": session.session_id,
            "state": session,
            "visibleEntitySample": visible_entities,
        }))
    }
}

#[async_trait]
impl AgentTool for CadSetDwgIssueMarkersTool {
    fn name(&self) -> &'static str {
        "cad_set_dwg_issue_markers"
    }

    fn description(&self) -> &'static str {
        "在当前 DWG viewer 上设置审查问题圈标记，不影响截图导出。"
    }

    fn parameters(&self) -> Value {
        with_result_mode_parameter(
            json!({
                "type": "object",
                "properties": {
                    "sessionId": { "type": "string" },
                    "replace": { "type": "boolean" },
                    "activeMarkerId": { "type": "string" },
                    "markers": {
                        "type": "array",
                        "minItems": 1,
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": { "type": "string" },
                                "severity": { "type": "string" },
                                "title": { "type": "string" },
                                "anchorPoint": {
                                    "type": "object",
                                    "properties": {
                                        "x": { "type": "number" },
                                        "y": { "type": "number" }
                                    },
                                    "required": ["x", "y"]
                                },
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
                            },
                            "required": ["id"]
                        }
                    }
                },
                "required": ["sessionId", "markers"]
            }),
            "full",
            "圈标记默认只影响 viewer 叠加层，不写入截图。",
        )
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
        let Some(session_id) = string_arg(args, "sessionId") else {
            return "错误：缺少必填参数 sessionId".to_string();
        };
        let Some(bridge) = &context.dwg_viewer_bridge else {
            return "错误：当前环境不支持 DWG Viewer bridge".to_string();
        };
        let Some(app_handle) = &context.app_handle else {
            return "错误：当前环境不支持发起 DWG Viewer 命令".to_string();
        };
        let markers = match args
            .get("markers")
            .cloned()
            .map(serde_json::from_value::<Vec<DwgIssueMarker>>)
            .transpose()
        {
            Ok(Some(value)) if !value.is_empty() => value,
            Ok(_) => return "错误：markers 不能为空".to_string(),
            Err(error) => return format!("错误：markers 无效：{error}"),
        };
        if markers.len() > 200 {
            return "错误：markers 最多允许 200 个，请先缩小范围".to_string();
        }

        match bridge
            .issue_command(
                app_handle,
                &session_id,
                "set_issue_markers",
                json!({
                    "markers": markers,
                    "replace": args.get("replace").and_then(Value::as_bool).unwrap_or(true),
                    "activeMarkerId": args.get("activeMarkerId").and_then(Value::as_str),
                }),
                Duration::from_secs(5),
            )
            .await
        {
            Ok(result) => serialize(json!({
                "ok": result.ok,
                "result": result.result,
                "error": result.error,
                "session": bridge.get_session(&session_id),
            })),
            Err(error) => format!("设置 DWG 问题圈标记失败：{error}"),
        }
    }
}

#[async_trait]
impl AgentTool for CadClearDwgIssueMarkersTool {
    fn name(&self) -> &'static str {
        "cad_clear_dwg_issue_markers"
    }

    fn description(&self) -> &'static str {
        "清除当前 DWG viewer 上的审查问题圈标记。"
    }

    fn parameters(&self) -> Value {
        with_result_mode_parameter(
            json!({
                "type": "object",
                "properties": {
                    "sessionId": { "type": "string" },
                    "markerIds": {
                        "type": "array",
                        "items": { "type": "string" }
                    }
                },
                "required": ["sessionId"]
            }),
            "full",
            "不传 markerIds 时清空全部圈标记。",
        )
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
        let Some(session_id) = string_arg(args, "sessionId") else {
            return "错误：缺少必填参数 sessionId".to_string();
        };
        let Some(bridge) = &context.dwg_viewer_bridge else {
            return "错误：当前环境不支持 DWG Viewer bridge".to_string();
        };
        let Some(app_handle) = &context.app_handle else {
            return "错误：当前环境不支持发起 DWG Viewer 命令".to_string();
        };

        match bridge
            .issue_command(
                app_handle,
                &session_id,
                "clear_issue_markers",
                json!({
                    "markerIds": args.get("markerIds").cloned().unwrap_or(Value::Null),
                }),
                Duration::from_secs(5),
            )
            .await
        {
            Ok(result) => serialize(json!({
                "ok": result.ok,
                "result": result.result,
                "error": result.error,
                "session": bridge.get_session(&session_id),
            })),
            Err(error) => format!("清除 DWG 问题圈标记失败：{error}"),
        }
    }
}

#[async_trait]
impl AgentTool for CadControlDwgViewerTool {
    fn name(&self) -> &'static str {
        "cad_control_dwg_viewer"
    }

    fn description(&self) -> &'static str {
        "统一执行 fit、focus、zoom、pan、select 等 DWG viewer 控制动作。"
    }

    fn parameters(&self) -> Value {
        with_result_mode_parameter(
            json!({
                "type": "object",
                "properties": {
                    "sessionId": { "type": "string" },
                    "action": {
                        "type": "object",
                        "properties": {
                            "type": {
                                "type": "string",
                                "enum": [
                                    "fit_drawing",
                                    "fit_bbox",
                                    "fit_entities",
                                    "focus_issue",
                                    "fly_to_point",
                                    "zoom_by_factor",
                                    "pan_by_view_ratio",
                                    "select_entities",
                                    "clear_selection",
                                    "set_mode"
                                ]
                            }
                        },
                        "required": ["type"]
                    }
                },
                "required": ["sessionId", "action"]
            }),
            "full",
            "控制结果返回最新 viewport 状态。",
        )
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
        let Some(session_id) = string_arg(args, "sessionId") else {
            return "错误：缺少必填参数 sessionId".to_string();
        };
        let Some(action) = args.get("action").cloned() else {
            return "错误：缺少必填参数 action".to_string();
        };
        let Some(action_type) = action.get("type").and_then(Value::as_str) else {
            return "错误：action.type 无效".to_string();
        };
        let Some(bridge) = &context.dwg_viewer_bridge else {
            return "错误：当前环境不支持 DWG Viewer bridge".to_string();
        };
        let Some(app_handle) = &context.app_handle else {
            return "错误：当前环境不支持发起 DWG Viewer 命令".to_string();
        };

        let (command_name, command_payload) = match action_type {
            "focus_issue" => match build_focus_issue_command(&action) {
                Ok(value) => value,
                Err(message) => return message,
            },
            other => (other.to_string(), action.clone()),
        };

        match bridge
            .issue_command(
                app_handle,
                &session_id,
                &command_name,
                command_payload,
                Duration::from_secs(5),
            )
            .await
        {
            Ok(result) => serialize(json!({
                "ok": result.ok,
                "result": result.result,
                "error": result.error,
                "session": bridge.get_session(&session_id),
            })),
            Err(error) => format!("控制 DWG Viewer 失败：{error}"),
        }
    }
}

#[async_trait]
impl AgentTool for CadPickDwgViewerTool {
    fn name(&self) -> &'static str {
        "cad_pick_dwg_viewer"
    }

    fn description(&self) -> &'static str {
        "基于当前 DWG viewer 会话做拾取，返回命中的实体包络。"
    }

    fn parameters(&self) -> Value {
        with_result_mode_parameter(
            json!({
                "type": "object",
                "properties": {
                    "sessionId": { "type": "string" },
                    "worldPoint": {
                        "type": "object",
                        "properties": {
                            "x": { "type": "number" },
                            "y": { "type": "number" }
                        },
                        "required": ["x", "y"]
                    },
                    "screenPoint": {
                        "type": "object",
                        "properties": {
                            "x": { "type": "number" },
                            "y": { "type": "number" }
                        },
                        "required": ["x", "y"]
                    },
                    "hitRadius": { "type": "number", "minimum": 0 },
                    "pickOneOnly": { "type": "boolean" }
                },
                "required": ["sessionId"]
            }),
            "full",
            "拾取结果保留完整 JSON。",
        )
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
        let Some(session_id) = string_arg(args, "sessionId") else {
            return "错误：缺少必填参数 sessionId".to_string();
        };
        let Some(bridge) = &context.dwg_viewer_bridge else {
            return "错误：当前环境不支持 DWG Viewer bridge".to_string();
        };
        let Some(app_handle) = &context.app_handle else {
            return "错误：当前环境不支持发起 DWG Viewer 命令".to_string();
        };
        match bridge
            .issue_command(
                app_handle,
                &session_id,
                "pick",
                args.clone(),
                Duration::from_secs(5),
            )
            .await
        {
            Ok(result) => serialize(json!({
                "ok": result.ok,
                "result": result.result,
                "error": result.error,
            })),
            Err(error) => format!("拾取 DWG Viewer 实体失败：{error}"),
        }
    }
}

#[async_trait]
impl AgentTool for CadCaptureDwgViewerTool {
    fn name(&self) -> &'static str {
        "cad_capture_dwg_viewer"
    }

    fn description(&self) -> &'static str {
        "捕获当前 DWG viewer 画面，返回图像 data URL 与关联的 viewport 状态。"
    }

    fn parameters(&self) -> Value {
        with_result_mode_parameter(
            json!({
                "type": "object",
                "properties": {
                    "sessionId": { "type": "string" },
                    "bounds": {
                        "type": "object",
                        "properties": {
                            "minX": { "type": "number" },
                            "minY": { "type": "number" },
                            "maxX": { "type": "number" },
                            "maxY": { "type": "number" }
                        },
                        "required": ["minX", "minY", "maxX", "maxY"]
                    },
                    "longSide": { "type": "integer", "minimum": 1 }
                },
                "required": ["sessionId"]
            }),
            "full",
            "截图结果包含 dataUrl，默认不直接灌入模型主上下文。",
        )
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
        let Some(session_id) = string_arg(args, "sessionId") else {
            return "错误：缺少必填参数 sessionId".to_string();
        };
        let Some(bridge) = &context.dwg_viewer_bridge else {
            return "错误：当前环境不支持 DWG Viewer bridge".to_string();
        };
        let Some(app_handle) = &context.app_handle else {
            return "错误：当前环境不支持发起 DWG Viewer 命令".to_string();
        };
        match bridge
            .issue_command(
                app_handle,
                &session_id,
                "capture",
                args.clone(),
                Duration::from_secs(5),
            )
            .await
        {
            Ok(result) => serialize(json!({
                "ok": result.ok,
                "result": result.result,
                "error": result.error,
                "session": bridge.get_session(&session_id),
            })),
            Err(error) => format!("捕获 DWG Viewer 画面失败：{error}"),
        }
    }
}

fn dispatcher_db(context: &ToolContext) -> Result<DispatcherDb, String> {
    DispatcherDb::new(context.dispatcher_db_path.clone())
        .map_err(|error| format!("打开 dispatcher 数据库失败：{error}"))
}

fn overview_parameters() -> Value {
    with_result_mode_parameter(
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "docId": { "type": "string" },
                "parserVersion": { "type": "string" }
            },
            "required": ["path"]
        }),
        "full",
        "概览结果应保留完整 JSON，便于模型继续缩小范围。",
    )
}

fn serialize(value: impl serde::Serialize) -> String {
    serde_json::to_string_pretty(&value).unwrap_or_else(|error| format!("序列化结果失败：{error}"))
}

fn normalize_path(path: &str, context: &ToolContext) -> Result<String, String> {
    let raw = PathBuf::from(path);
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

async fn ensure_index(
    path: &str,
    parser_version: &str,
    refresh_policy: &str,
    open_viewer_if_needed: bool,
    context: &ToolContext,
) -> Result<Value, String> {
    let normalized = normalize_path(path, context)?;
    let metadata =
        fs::metadata(&normalized).map_err(|error| format!("读取 DWG 文件元数据失败：{error}"))?;
    let file_size = metadata.len();
    let file_mtime = file_mtime(&metadata)?;
    let db = dispatcher_db(context)?;
    let cached = db
        .get_dwg_document_overview(
            &context.workspace.to_string_lossy(),
            &normalized,
            file_size,
            file_mtime,
            parser_version,
        )
        .map_err(|error| format!("读取 DWG 索引失败：{error}"))?;

    if cached.is_some() && refresh_policy != "always" {
        let overview = cached.expect("cached overview");
        return Ok(json!({
            "docId": overview.document.id,
            "status": "ready",
            "parseSource": "cache",
            "summaryPreview": overview.document.summary,
        }));
    }
    if refresh_policy == "never" {
        return Err("错误：未找到可用 DWG 索引".to_string());
    }
    if !open_viewer_if_needed {
        return Err("错误：当前未允许自动打开 DWG 工作台，无法触发重建索引".to_string());
    }

    let _ = resolve_session_for_path(&normalized, "open_if_missing", context).await?;
    let started = std::time::Instant::now();
    while started.elapsed() < Duration::from_secs(60) {
        if let Some(overview) = db
            .get_dwg_document_overview(
                &context.workspace.to_string_lossy(),
                &normalized,
                file_size,
                file_mtime,
                parser_version,
            )
            .map_err(|error| format!("读取 DWG 索引失败：{error}"))?
        {
            return Ok(json!({
                "docId": overview.document.id,
                "status": "ready",
                "parseSource": if cached.is_some() { "reindexed" } else { "indexed" },
                "summaryPreview": overview.document.summary,
            }));
        }
        if let Some(bridge) = &context.dwg_viewer_bridge {
            if let Some(session) = bridge.best_session_for_file(&context.workspace_id, &normalized)
            {
                if session.parse_status == "error" {
                    return Err(format!(
                        "错误：DWG 索引构建失败：{}",
                        session.parse_error.unwrap_or_else(|| {
                            "前端解析器进入错误状态，请检查 DWG 工作台报错".to_string()
                        })
                    ));
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    Err("错误：等待 DWG 索引构建超时".to_string())
}

async fn load_overview(
    args: &Value,
    context: &ToolContext,
) -> Result<crate::agent::cad::DwgDocumentOverview, String> {
    let db = dispatcher_db(context)?;
    let document = resolve_document(&db, args, context, false).await?;
    Ok(crate::agent::cad::DwgDocumentOverview {
        document,
        next_suggested_actions: vec![
            "先按图层收窄范围".to_string(),
            "优先使用 cad_inspect_dwg_region 缩小局部区域".to_string(),
            "仅对小批量实体调用 cad_get_dwg_entity_detail".to_string(),
        ],
    })
}

async fn resolve_document(
    db: &DispatcherDb,
    args: &Value,
    context: &ToolContext,
    open_if_missing: bool,
) -> Result<crate::agent::cad::DwgDocumentRecord, String> {
    if let Some(doc_id) = string_arg(args, "docId") {
        return db
            .get_dwg_document_by_id(&doc_id)
            .map_err(|error| format!("读取 DWG 文档失败：{error}"))?
            .ok_or_else(|| format!("错误：未找到 DWG 文档：{doc_id}"));
    }
    let Some(path) = string_arg(args, "path") else {
        return Err("错误：缺少必填参数 path".to_string());
    };
    let parser_version =
        string_arg(args, "parserVersion").unwrap_or_else(|| DEFAULT_PARSER_VERSION.to_string());
    let normalized = normalize_path(&path, context)?;
    let metadata =
        fs::metadata(&normalized).map_err(|error| format!("读取 DWG 文件元数据失败：{error}"))?;
    if let Some(document) = db
        .get_dwg_document(
            &context.workspace.to_string_lossy(),
            &normalized,
            metadata.len(),
            file_mtime(&metadata)?,
            &parser_version,
        )
        .map_err(|error| format!("读取 DWG 文档失败：{error}"))?
    {
        return Ok(document);
    }
    if !open_if_missing {
        return Err("错误：未找到 DWG 索引，请先调用 cad_ensure_dwg_index".to_string());
    }
    ensure_index(
        &normalized,
        &parser_version,
        "if_missing_or_stale",
        true,
        context,
    )
    .await?;
    db.get_dwg_document(
        &context.workspace.to_string_lossy(),
        &normalized,
        metadata.len(),
        file_mtime(&metadata)?,
        &parser_version,
    )
    .map_err(|error| format!("读取 DWG 文档失败：{error}"))?
    .ok_or_else(|| "错误：DWG 索引仍不可用".to_string())
}

async fn resolve_session_for_path(
    path: &str,
    policy: &str,
    context: &ToolContext,
) -> Result<(DwgViewerSessionState, bool), String> {
    let normalized = normalize_path(path, context)?;
    let Some(bridge) = &context.dwg_viewer_bridge else {
        return Err("错误：当前环境不支持 DWG Viewer bridge".to_string());
    };
    if let Some(session) = bridge.best_session_for_file(&context.workspace_id, &normalized) {
        return Ok((session, false));
    }
    if policy != "open_if_missing" {
        return Err("错误：未找到可复用的 DWG Viewer 会话".to_string());
    }
    let Some(app_handle) = &context.app_handle else {
        return Err("错误：当前环境不支持自动打开 DWG 工作台".to_string());
    };
    bridge
        .request_open(app_handle, &context.workspace_id, &normalized)
        .await
        .map_err(|error| format!("请求打开 DWG 工作台失败：{error}"))?;
    let session = bridge
        .wait_for_session(&context.workspace_id, &normalized, Duration::from_secs(30))
        .await
        .map_err(|error| error.to_string())?;
    Ok((session, true))
}

fn parse_region_bbox(args: &Value) -> Result<CadBBox, String> {
    if let Some(bbox) = args.get("bbox") {
        return serde_json::from_value::<CadBBox>(bbox.clone())
            .map_err(|error| format!("错误：bbox 无效：{error}"));
    }
    let Some(point) = args.get("point").cloned() else {
        return Err("错误：缺少 bbox，或 point + radius".to_string());
    };
    let point = serde_json::from_value::<CadPoint>(point)
        .map_err(|error| format!("错误：point 无效：{error}"))?;
    let radius = args
        .get("radius")
        .and_then(Value::as_f64)
        .filter(|value| *value >= 0.0)
        .ok_or_else(|| "错误：radius 无效".to_string())?;
    Ok(CadBBox {
        min_x: point.x - radius,
        min_y: point.y - radius,
        max_x: point.x + radius,
        max_y: point.y + radius,
    })
}

fn load_visible_entities(
    context: &ToolContext,
    session: &DwgViewerSessionState,
    sample_limit: usize,
) -> Option<Value> {
    let bbox = session.viewport_box.clone()?;
    let doc_id = session.doc_id.clone()?;
    let db = dispatcher_db(context).ok()?;
    db.inspect_dwg_region(&doc_id, &bbox, "layer", sample_limit)
        .ok()
        .and_then(|result| serde_json::to_value(result).ok())
}

fn build_focus_issue_command(action: &Value) -> Result<(String, Value), String> {
    let issue = action
        .get("issue")
        .cloned()
        .ok_or_else(|| "错误：focus_issue 需要 issue 对象".to_string())?;
    let issue = serde_json::from_value::<CadReviewIssueRecord>(issue)
        .map_err(|error| format!("错误：issue 无效：{error}"))?;
    if !issue.entity_refs.is_empty() {
        return Ok((
            "fit_entities".to_string(),
            json!({ "entityIds": issue.entity_refs, "reason": "focus_issue" }),
        ));
    }
    if let Some(viewport_hint) = issue.viewport_hint {
        if let Some(center) = viewport_hint.center {
            return Ok((
                "fly_to_point".to_string(),
                json!({ "point": center, "zoomScale": viewport_hint.zoom_scale }),
            ));
        }
        if let Some(bbox) = viewport_hint.bbox {
            return Ok(("fit_bbox".to_string(), json!({ "bbox": bbox })));
        }
    }
    if let Some(anchor) = issue.anchor_point {
        return Ok(("fly_to_point".to_string(), json!({ "point": anchor })));
    }
    if let Some(bbox) = issue.bbox {
        return Ok(("fit_bbox".to_string(), json!({ "bbox": bbox })));
    }
    Ok(("noop".to_string(), json!({ "reason": "no_focus_target" })))
}
