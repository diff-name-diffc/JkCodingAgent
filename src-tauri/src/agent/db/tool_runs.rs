//! 工具运行台账：记录每次 agent tool call 的生命周期、结果状态和观测元数据。

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::util::now;
use super::DispatcherDb;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherToolRunRecord {
    pub id: String,
    pub workspace_id: String,
    pub tool_call_id: String,
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

impl DispatcherDb {
    pub fn create_tool_run(&self, run: NewToolRun) -> Result<DispatcherToolRunRecord> {
        let conn = self.conn()?;
        let id = Uuid::new_v4().to_string();
        let timestamp = now();
        conn.execute(
            "INSERT INTO dispatcher_tool_runs (
                id, workspace_id, tool_call_id, tool_name, provider, category, status,
                arguments_json, effective_arguments_json, metadata_json, created_at, updated_at
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'planned', ?7, ?8, ?9, ?10, ?10)",
            params![
                &id,
                &run.workspace_id,
                &run.tool_call_id,
                &run.tool_name,
                &run.provider,
                &run.category,
                &run.arguments_json,
                &run.effective_arguments_json,
                &run.metadata_json,
                &timestamp
            ],
        )
        .context("create dispatcher tool run")?;
        self.load_tool_run(&id)
    }

    pub fn mark_tool_run_started(&self, id: &str) -> Result<DispatcherToolRunRecord> {
        let conn = self.conn()?;
        let timestamp = now();
        // 生命周期单向推进：仅允许 planned → running，
        // 已在运行或已到终态的记录不得被回退。
        conn.execute(
            "UPDATE dispatcher_tool_runs
             SET status = 'running', started_at = COALESCE(started_at, ?1), updated_at = ?1
             WHERE id = ?2 AND status = 'planned'",
            params![&timestamp, id],
        )
        .context("mark dispatcher tool run started")?;
        self.load_tool_run(id)
    }

    pub fn finish_tool_run(
        &self,
        id: &str,
        finish: FinishToolRun,
    ) -> Result<DispatcherToolRunRecord> {
        let mut conn = self.conn()?;
        // IMMEDIATE：事务内先读状态再写入，避免延迟事务升级写锁时的 SQLITE_BUSY。
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("begin finish dispatcher tool run transaction")?;
        let current: Option<(String, Option<String>)> = tx
            .query_row(
                "SELECT status, started_at FROM dispatcher_tool_runs WHERE id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .context("load dispatcher tool run state before finish")?;
        let Some((status, started_at)) = current else {
            anyhow::bail!("dispatcher tool run not found: {id}");
        };
        if is_terminal_run_status(&status) {
            // 状态守卫：终态只能由第一个 finish 落定，重复/乱序 finish 不得覆盖，
            // 幂等返回当前快照。
            tx.commit()
                .context("commit finish dispatcher tool run no-op")?;
            return load_tool_run_on_conn(&conn, id);
        }

        let finished_at = now();
        // 时长在 Rust 侧计算（不再依赖 SQL julianday 解析文本时间戳）；
        // started_at 缺失或无法解析时容错为 0。
        let duration_ms = duration_since_started_ms(started_at.as_deref(), &finished_at);
        // 可选字段统一 COALESCE 保留语义：未提供新值时保留既有值，
        // 避免重复/乱序 finish 清空已落定的字段。
        let changed = tx
            .execute(
                "UPDATE dispatcher_tool_runs
                 SET status = ?1,
                     result_mode = COALESCE(?2, result_mode),
                     message_id = COALESCE(?3, message_id),
                     error_kind = COALESCE(?4, error_kind),
                     error_message = COALESCE(?5, error_message),
                     action_kind = COALESCE(?6, action_kind),
                     finished_at = ?7,
                     duration_ms = ?8,
                     metadata_json = COALESCE(?9, metadata_json),
                     updated_at = ?7
                 WHERE id = ?10
                   AND status NOT IN ('succeeded', 'recoverable_error', 'fatal_error', 'cancelled')",
                params![
                    &finish.status,
                    &finish.result_mode,
                    &finish.message_id,
                    &finish.error_kind,
                    &finish.error_message,
                    &finish.action_kind,
                    &finished_at,
                    duration_ms,
                    &finish.metadata_json,
                    id
                ],
            )
            .context("finish dispatcher tool run")?;
        if changed == 0 {
            // 与状态守卫的双重保险：其他写者抢先推进到终态时，退化为只读返回。
            tx.commit()
                .context("commit finish dispatcher tool run no-op")?;
            return load_tool_run_on_conn(&conn, id);
        }
        tx.commit().context("commit finish dispatcher tool run")?;
        load_tool_run_on_conn(&conn, id)
    }

    #[allow(dead_code)]
    pub fn attach_tool_run_message(
        &self,
        id: &str,
        message_id: &str,
    ) -> Result<DispatcherToolRunRecord> {
        let conn = self.conn()?;
        let timestamp = now();
        conn.execute(
            "UPDATE dispatcher_tool_runs
             SET message_id = ?1, updated_at = ?2
             WHERE id = ?3",
            params![message_id, &timestamp, id],
        )
        .context("attach dispatcher tool run message")?;
        self.load_tool_run(id)
    }

    pub fn load_tool_run(&self, id: &str) -> Result<DispatcherToolRunRecord> {
        let conn = self.conn()?;
        load_tool_run_on_conn(&conn, id)
    }

    #[allow(dead_code)]
    pub fn list_recent_tool_runs(
        &self,
        workspace_id: &str,
        limit: usize,
    ) -> Result<Vec<DispatcherToolRunRecord>> {
        let conn = self.conn()?;
        let limit = i64::try_from(limit.clamp(1, 200)).unwrap_or(200);
        let mut stmt = conn.prepare(
            "SELECT id, workspace_id, tool_call_id, tool_name, provider, category, status,
                    arguments_json, effective_arguments_json, result_mode, message_id,
                    error_kind, error_message, action_kind, started_at, finished_at,
                    duration_ms, metadata_json, created_at, updated_at
             FROM dispatcher_tool_runs
             WHERE workspace_id = ?1
             ORDER BY created_at DESC, rowid DESC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![workspace_id, limit], map_tool_run)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("list recent dispatcher tool runs")
    }

    pub async fn create_tool_run_async(&self, run: NewToolRun) -> Result<DispatcherToolRunRecord> {
        let db = self.clone();
        tokio::task::spawn_blocking(move || db.create_tool_run(run))
            .await
            .context("create_tool_run spawn_blocking")?
    }

    pub async fn mark_tool_run_started_async(&self, id: &str) -> Result<DispatcherToolRunRecord> {
        let db = self.clone();
        let id = id.to_string();
        tokio::task::spawn_blocking(move || db.mark_tool_run_started(&id))
            .await
            .context("mark_tool_run_started spawn_blocking")?
    }

    pub async fn finish_tool_run_async(
        &self,
        id: &str,
        finish: FinishToolRun,
    ) -> Result<DispatcherToolRunRecord> {
        let db = self.clone();
        let id = id.to_string();
        tokio::task::spawn_blocking(move || db.finish_tool_run(&id, finish))
            .await
            .context("finish_tool_run spawn_blocking")?
    }

    #[allow(dead_code)]
    pub async fn load_tool_run_async(&self, id: &str) -> Result<DispatcherToolRunRecord> {
        let db = self.clone();
        let id = id.to_string();
        tokio::task::spawn_blocking(move || db.load_tool_run(&id))
            .await
            .context("load_tool_run spawn_blocking")?
    }

    #[allow(dead_code)]
    pub async fn list_recent_tool_runs_async(
        &self,
        workspace_id: &str,
        limit: usize,
    ) -> Result<Vec<DispatcherToolRunRecord>> {
        let db = self.clone();
        let workspace_id = workspace_id.to_string();
        tokio::task::spawn_blocking(move || db.list_recent_tool_runs(&workspace_id, limit))
            .await
            .context("list_recent_tool_runs spawn_blocking")?
    }
}

fn load_tool_run_on_conn(conn: &Connection, id: &str) -> Result<DispatcherToolRunRecord> {
    conn.query_row(
        "SELECT id, workspace_id, tool_call_id, tool_name, provider, category, status,
                arguments_json, effective_arguments_json, result_mode, message_id,
                error_kind, error_message, action_kind, started_at, finished_at,
                duration_ms, metadata_json, created_at, updated_at
         FROM dispatcher_tool_runs
         WHERE id = ?1",
        params![id],
        map_tool_run,
    )
    .optional()
    .context("load dispatcher tool run")?
    .with_context(|| format!("dispatcher tool run not found: {id}"))
}

/// 运行到达这些状态后生命周期即结束，不得被后续 finish 覆盖（单向推进）。
fn is_terminal_run_status(status: &str) -> bool {
    matches!(
        status,
        "succeeded" | "recoverable_error" | "fatal_error" | "cancelled"
    )
}

/// 在 Rust 侧由 RFC3339 文本时间戳计算时长（毫秒）。
/// started_at 缺失、任一时间戳无法解析时容错为 0，不产生 NULL。
fn duration_since_started_ms(started_at: Option<&str>, finished_at: &str) -> i64 {
    let Some(started_at) = started_at else {
        return 0;
    };
    let (Ok(started), Ok(finished)) = (
        chrono::DateTime::parse_from_rfc3339(started_at),
        chrono::DateTime::parse_from_rfc3339(finished_at),
    ) else {
        return 0;
    };
    (finished - started).num_milliseconds().max(0)
}

fn map_tool_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<DispatcherToolRunRecord> {
    // duration_ms 容忍 NULL/负值等异常或旧数据，不再让整行读取失败。
    let duration_ms = row
        .get::<_, Option<i64>>(16)?
        .map(|value| u64::try_from(value.max(0)).unwrap_or(0))
        .unwrap_or(0);
    Ok(DispatcherToolRunRecord {
        id: row.get(0)?,
        workspace_id: row.get(1)?,
        tool_call_id: row.get(2)?,
        tool_name: row.get(3)?,
        provider: row.get(4)?,
        category: row.get(5)?,
        status: row.get(6)?,
        arguments_json: row.get(7)?,
        effective_arguments_json: row.get(8)?,
        result_mode: row.get(9)?,
        message_id: row.get(10)?,
        error_kind: row.get(11)?,
        error_message: row.get(12)?,
        action_kind: row.get(13)?,
        started_at: row.get(14)?,
        finished_at: row.get(15)?,
        duration_ms,
        metadata_json: row.get(17)?,
        created_at: row.get(18)?,
        updated_at: row.get(19)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> DispatcherDb {
        let path = std::env::temp_dir().join(format!(
            "jkcodingagent-tool-runs-{}.sqlite3",
            Uuid::new_v4()
        ));
        DispatcherDb::new(path).expect("create test dispatcher db")
    }

    fn new_run(workspace_id: &str) -> NewToolRun {
        NewToolRun {
            workspace_id: workspace_id.to_string(),
            tool_call_id: format!("call-{}", Uuid::new_v4()),
            tool_name: "demo_tool".to_string(),
            provider: "builtin".to_string(),
            category: "general".to_string(),
            arguments_json: "{}".to_string(),
            effective_arguments_json: "{}".to_string(),
            metadata_json: "{}".to_string(),
        }
    }

    fn finish(status: &str) -> FinishToolRun {
        FinishToolRun {
            status: status.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn finish_advances_lifecycle_and_records_duration() {
        let db = test_db();
        let run = db.create_tool_run(new_run("ws")).expect("create run");
        assert_eq!(run.status, "planned");

        let started = db.mark_tool_run_started(&run.id).expect("start run");
        assert_eq!(started.status, "running");
        assert!(started.started_at.is_some());

        let finished = db
            .finish_tool_run(&run.id, finish("succeeded"))
            .expect("finish run");
        assert_eq!(finished.status, "succeeded");
        assert!(finished.finished_at.is_some());
        assert!(finished.started_at.is_some());
    }

    #[test]
    fn terminal_state_is_not_overwritten_by_second_finish() {
        let db = test_db();
        let run = db.create_tool_run(new_run("ws")).expect("create run");
        db.mark_tool_run_started(&run.id).expect("start run");

        let first = db
            .finish_tool_run(
                &run.id,
                FinishToolRun {
                    status: "recoverable_error".to_string(),
                    error_kind: Some("retryable".to_string()),
                    error_message: Some("boom".to_string()),
                    ..Default::default()
                },
            )
            .expect("first finish");
        assert_eq!(first.status, "recoverable_error");
        assert_eq!(first.error_message.as_deref(), Some("boom"));

        // 重复/乱序 finish 不得把终态改回或清空错误信息。
        let second = db
            .finish_tool_run(
                &run.id,
                FinishToolRun {
                    status: "succeeded".to_string(),
                    error_kind: None,
                    error_message: None,
                    ..Default::default()
                },
            )
            .expect("second finish is a no-op");
        assert_eq!(second.status, "recoverable_error", "终态不得被覆盖");
        assert_eq!(
            second.error_message.as_deref(),
            Some("boom"),
            "错误信息不得被清空"
        );
    }

    #[test]
    fn started_does_not_regress_terminal_state() {
        let db = test_db();
        let run = db.create_tool_run(new_run("ws")).expect("create run");
        db.mark_tool_run_started(&run.id).expect("start run");
        db.finish_tool_run(&run.id, finish("succeeded"))
            .expect("finish run");

        let regressed = db.mark_tool_run_started(&run.id).expect("re-start no-op");
        assert_eq!(regressed.status, "succeeded", "终态不得回退到 running");
    }

    #[test]
    fn finish_missing_run_errors() {
        let db = test_db();
        let error = db
            .finish_tool_run("no-such-run", finish("succeeded"))
            .expect_err("missing run must fail");
        assert!(error.to_string().contains("not found"));
    }

    #[test]
    fn duration_is_nonnegative_even_with_missing_started_at() {
        // 直接 finish 一个 planned（未 started）的 run，时长应容错为 0 而非 NULL。
        let db = test_db();
        let run = db.create_tool_run(new_run("ws")).expect("create run");
        let finished = db
            .finish_tool_run(&run.id, finish("cancelled"))
            .expect("finish planned run");
        assert_eq!(finished.duration_ms, 0);
        assert!(finished.started_at.is_none());
    }
}
