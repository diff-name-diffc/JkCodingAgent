//! 工具运行台账：记录每次 agent tool call 的生命周期、结果状态和观测元数据。
//!
//! 本文件保留公开 DTO、列投影与行映射（生命周期/树查询共用的读取底座）；
//! 写入与查询按变化原因拆分：
//! - `lifecycle`：单条运行的状态机（planned → running → 终态，单向推进）；
//! - `tree`：父子调用树的列举、消息绑定与未绑定树清理；
//! - `async_api`：全部 `spawn_blocking` 异步包装；
//! - `tests`：生命周期与树语义回归测试。

mod async_api;
mod lifecycle;
mod tree;

#[cfg(test)]
mod tests;

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

pub(crate) const TOOL_RUN_ORIGIN_MODEL: &str = "model";
pub(super) const TOOL_RUN_SELECT_COLUMNS: &str =
    "r.id, r.workspace_id, r.tool_call_id, r.parent_run_id, r.origin, r.step_id, r.sequence,
     r.tool_name, r.provider, r.category, r.status, r.arguments_json,
     r.effective_arguments_json, r.result_mode, r.message_id, r.error_kind,
     r.error_message, r.action_kind, r.started_at, r.finished_at, r.duration_ms,
     r.metadata_json, r.created_at, r.updated_at";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherToolRunRecord {
    pub id: String,
    pub workspace_id: String,
    pub tool_call_id: String,
    pub parent_run_id: Option<String>,
    pub origin: String,
    pub step_id: Option<String>,
    pub sequence: u64,
    pub tool_name: String,
    pub provider: String,
    pub category: String,
    pub status: String,
    pub arguments_json: String,
    pub effective_arguments_json: String,
    pub result_mode: Option<String>,
    pub message_id: Option<String>,
    pub error_kind: Option<String>,
    pub error_message: Option<String>,
    pub action_kind: Option<String>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub duration_ms: u64,
    pub metadata_json: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct NewToolRun {
    pub workspace_id: String,
    pub tool_call_id: String,
    pub tool_name: String,
    pub provider: String,
    pub category: String,
    pub arguments_json: String,
    pub effective_arguments_json: String,
    pub metadata_json: String,
}

/// 单次工具调用在调用树中的位置。
///
/// 根调用使用默认值；受限运行时通过 `create_tool_run_with_trace` 为内部调用
/// 显式提供父 run、IR step 与父节点内的稳定顺序。
#[derive(Debug, Clone)]
pub struct ToolRunTraceContext {
    pub parent_run_id: Option<String>,
    pub origin: String,
    pub step_id: Option<String>,
    pub sequence: u64,
}

impl Default for ToolRunTraceContext {
    fn default() -> Self {
        Self {
            parent_run_id: None,
            origin: TOOL_RUN_ORIGIN_MODEL.to_string(),
            step_id: None,
            sequence: 0,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct FinishToolRun {
    pub status: String,
    pub result_mode: Option<String>,
    pub message_id: Option<String>,
    pub error_kind: Option<String>,
    pub error_message: Option<String>,
    pub action_kind: Option<String>,
    pub metadata_json: Option<String>,
}

pub(super) fn load_tool_run_on_conn(
    conn: &Connection,
    id: &str,
) -> Result<DispatcherToolRunRecord> {
    let sql = format!(
        "SELECT {TOOL_RUN_SELECT_COLUMNS}
         FROM dispatcher_tool_runs r
         WHERE r.id = ?1"
    );
    conn.query_row(&sql, params![id], map_tool_run)
        .optional()
        .context("load dispatcher tool run")?
        .with_context(|| format!("dispatcher tool run not found: {id}"))
}

pub(super) fn map_tool_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<DispatcherToolRunRecord> {
    // duration_ms 容忍 NULL/负值等异常或旧数据，不再让整行读取失败。
    let duration_ms = row
        .get::<_, Option<i64>>("duration_ms")?
        .map(|value| u64::try_from(value.max(0)).unwrap_or(0))
        .unwrap_or(0);
    let sequence = row
        .get::<_, Option<i64>>("sequence")?
        .map(|value| u64::try_from(value.max(0)).unwrap_or(0))
        .unwrap_or(0);
    Ok(DispatcherToolRunRecord {
        id: row.get("id")?,
        workspace_id: row.get("workspace_id")?,
        tool_call_id: row.get("tool_call_id")?,
        parent_run_id: row.get("parent_run_id")?,
        origin: row.get("origin")?,
        step_id: row.get("step_id")?,
        sequence,
        tool_name: row.get("tool_name")?,
        provider: row.get("provider")?,
        category: row.get("category")?,
        status: row.get("status")?,
        arguments_json: row.get("arguments_json")?,
        effective_arguments_json: row.get("effective_arguments_json")?,
        result_mode: row.get("result_mode")?,
        message_id: row.get("message_id")?,
        error_kind: row.get("error_kind")?,
        error_message: row.get("error_message")?,
        action_kind: row.get("action_kind")?,
        started_at: row.get("started_at")?,
        finished_at: row.get("finished_at")?,
        duration_ms,
        metadata_json: row.get("metadata_json")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}
