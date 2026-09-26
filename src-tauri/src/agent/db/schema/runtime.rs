//! v8：异步工具身份与事务型完成事件 outbox。基线与迁移共用 DDL
//! （含 v9 为 `delivery_message_id` 补的索引，见下方索引注释）。
use super::DispatcherDb;
use anyhow::{Context, Result};
use rusqlite::{params, Connection, Transaction};

pub(super) fn migrate(db: &DispatcherDb, conn: &mut Connection) -> Result<()> {
    let stamp = chrono::Utc::now().format("%Y%m%d%H%M%S%3f");
    let name = db
        .path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("jkbot.sqlite3");
    let backup = db
        .path
        .with_file_name(format!("{name}.pre-v8-backup-{stamp}"));
    conn.execute("VACUUM INTO ?1", params![backup.to_string_lossy()])
        .context("v7→v8 迁移前整库快照失败，未修改数据库")?;
    let tx = conn.transaction().context("begin v7→v8 migration")?;
    extend_schema(&tx)?;
    tx.pragma_update(None, "user_version", 8)?;
    tx.commit().context("commit v7→v8 migration")
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v7_upgrade_preserves_rows_and_creates_snapshot() {
        let directory = std::env::temp_dir().join(format!("runtime-v8-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("db.sqlite3");
        {
            let conn = Connection::open(&path).unwrap();
            // 原基线 DDL 不包含 v8 扩展，因此是真正的 v7 形态。
            conn.execute_batch(super::super::BASELINE_DDL).unwrap();
            conn.execute_batch(
                "INSERT INTO dispatcher_messages(id, workspace_id, role, created_at)
                 VALUES ('request', 'workspace', 'assistant', '2026-09-26T00:00:00Z');
                 INSERT INTO dispatcher_tool_runs(id, workspace_id, tool_call_id,
                    tool_name, provider, category, status, created_at, updated_at)
                 VALUES ('task', 'workspace', 'call', 'echo', 'builtin', 'general',
                    'succeeded', '2026-09-26T00:00:00Z', '2026-09-26T00:00:00Z');
                 PRAGMA user_version = 7;",
            )
            .unwrap();
        }
        {
            let db = DispatcherDb::new(path.clone()).unwrap();
            let run = db.load_tool_run("task").unwrap();
            assert_eq!(run.status, "succeeded");
            assert!(run.agent_run_id.is_none());
            assert!(run.phase.is_none());
            let conn = db.conn().unwrap();
            // v7 库沿迁移链一路升到当前基线（v7→v8→v9）。
            assert_eq!(
                conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i32>(0))
                    .unwrap(),
                super::super::SCHEMA_VERSION
            );
            assert_eq!(
                conn.query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index'
                       AND name = 'idx_tool_completions_delivery'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
                1,
                "v9 的投递消息索引应随迁移链一并创建"
            );
            assert_eq!(
                conn.query_row("SELECT id FROM dispatcher_messages", [], |row| row
                    .get::<_, String>(0))
                    .unwrap(),
                "request"
            );
            assert_eq!(
                conn.query_row(
                    "SELECT COUNT(*) FROM dispatcher_tool_completions",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
                0
            );
        }
        let backups = std::fs::read_dir(&directory)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .contains("pre-v8-backup")
            })
            .collect::<Vec<_>>();
        assert_eq!(backups.len(), 1);
        let snapshot = Connection::open(backups[0].path()).unwrap();
        assert_eq!(
            snapshot
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i32>(0))
                .unwrap(),
            7
        );
        drop(snapshot);
        drop(DispatcherDb::new(path).unwrap());
        std::fs::remove_dir_all(directory).unwrap();
    }
}
