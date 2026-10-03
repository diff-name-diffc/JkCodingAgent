//! 异步工具身份与事务型完成事件 outbox（历史 v8/v9 扩展；v16 基线收敛后
//! 仅由基线建库路径 `extend_schema` 使用）。
use anyhow::{Context, Result};
use rusqlite::{params, Transaction};

pub(super) fn extend_schema(tx: &Transaction<'_>) -> Result<()> {
    for (table, column, kind) in [
        ("dispatcher_messages", "tool_task_id", "TEXT"),
        ("dispatcher_tool_runs", "agent_run_id", "TEXT"),
        ("dispatcher_tool_runs", "scope_id", "TEXT"),
        ("dispatcher_tool_runs", "dispatch_round", "INTEGER"),
        ("dispatcher_tool_runs", "root_request_message_id", "TEXT"),
        ("dispatcher_tool_runs", "reply_message_id", "TEXT"),
        ("dispatcher_tool_runs", "dispatch_mode", "TEXT"),
        ("dispatcher_tool_runs", "phase", "TEXT"),
    ] {
        let present: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2)",
            params![table, column],
            |row| row.get(0),
        )?;
        if !present {
            tx.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {kind}"))?;
        }
    }
    tx.execute_batch(DDL)
        .context("create asynchronous tool runtime schema")
}

const DDL: &str = "
CREATE INDEX IF NOT EXISTS idx_tool_runs_scope ON dispatcher_tool_runs(agent_run_id, scope_id);
CREATE INDEX IF NOT EXISTS idx_tool_runs_request ON dispatcher_tool_runs(workspace_id, root_request_message_id);
CREATE TABLE IF NOT EXISTS dispatcher_tool_completions (
    event_id INTEGER PRIMARY KEY AUTOINCREMENT,
    tool_run_id TEXT NOT NULL UNIQUE REFERENCES dispatcher_tool_runs(id) ON DELETE CASCADE,
    agent_run_id TEXT NOT NULL,
    scope_id TEXT NOT NULL,
    status TEXT NOT NULL,
    error_kind TEXT,
    fatal INTEGER NOT NULL DEFAULT 0 CHECK(fatal IN (0, 1)),
    retryable INTEGER NOT NULL DEFAULT 0 CHECK(retryable IN (0, 1)),
    display_content TEXT NOT NULL,
    context_payload TEXT NOT NULL,
    result_mode TEXT NOT NULL,
    usage_json TEXT,
    delivery_message_id TEXT REFERENCES dispatcher_messages(id) ON DELETE SET NULL,
    observed_request_step INTEGER,
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_tool_completions_scope ON dispatcher_tool_completions(agent_run_id, scope_id, event_id);
-- v9：删除触发器按 delivery_message_id 反查（见下方 reset_tool_completion_observation），
-- 无索引时每次消息删除都对该表全表扫描。
CREATE INDEX IF NOT EXISTS idx_tool_completions_delivery ON dispatcher_tool_completions(delivery_message_id);
CREATE TRIGGER IF NOT EXISTS reset_tool_completion_observation BEFORE DELETE ON dispatcher_messages
BEGIN
    UPDATE dispatcher_tool_completions SET observed_request_step = NULL
    WHERE delivery_message_id = OLD.id;
END;
";
