//! 工具运行台账：记录每次 agent tool call 的生命周期、结果状态和观测元数据。

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::util::now;
use super::DispatcherDb;

pub(crate) const TOOL_RUN_ORIGIN_MODEL: &str = "model";
const TOOL_RUN_SELECT_COLUMNS: &str =
    "r.id, r.workspace_id, r.tool_call_id, r.parent_run_id, r.origin, r.step_id, r.sequence,
     r.tool_name, r.provider, r.category, r.status, r.arguments_json,
     r.effective_arguments_json, r.result_mode, r.message_id, r.error_kind,
     r.error_message, r.action_kind, r.started_at, r.finished_at, r.duration_ms,
     r.metadata_json, r.created_at, r.updated_at";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherToolRunRecord {
    pub id: String,
    pub workspace_id: String,
    pub tool_call_id: String,
    #[serde(default)]
    pub parent_run_id: Option<String>,
    #[serde(default = "default_tool_run_origin")]
    pub origin: String,
    #[serde(default)]
    pub step_id: Option<String>,
    #[serde(default)]
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

fn default_tool_run_origin() -> String {
    TOOL_RUN_ORIGIN_MODEL.to_string()
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
    #[cfg(test)]
    pub fn create_tool_run(&self, run: NewToolRun) -> Result<DispatcherToolRunRecord> {
        self.create_tool_run_with_trace(run, ToolRunTraceContext::default())
    }

    pub fn create_tool_run_with_trace(
        &self,
        run: NewToolRun,
        trace: ToolRunTraceContext,
    ) -> Result<DispatcherToolRunRecord> {
        let mut conn = self.conn()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("begin create dispatcher tool run transaction")?;
        let id = Uuid::new_v4().to_string();
        let timestamp = now();
        let origin = trace.origin.trim();
        if origin.is_empty() {
            anyhow::bail!("dispatcher tool run origin must not be empty");
        }
        let sequence = i64::try_from(trace.sequence)
            .context("dispatcher tool run sequence exceeds sqlite INTEGER range")?;

        if let Some(parent_run_id) = trace.parent_run_id.as_deref() {
            let parent_workspace_id = tx
                .query_row(
                    "SELECT workspace_id FROM dispatcher_tool_runs WHERE id = ?1",
                    params![parent_run_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .context("load parent dispatcher tool run")?
                .with_context(|| {
                    format!("parent dispatcher tool run not found: {parent_run_id}")
                })?;
            if parent_workspace_id != run.workspace_id {
                anyhow::bail!(
                    "parent dispatcher tool run {parent_run_id} belongs to workspace {parent_workspace_id}, not {}",
                    run.workspace_id
                );
            }
        }

        tx.execute(
            "INSERT INTO dispatcher_tool_runs (
                id, workspace_id, tool_call_id, parent_run_id, origin, step_id, sequence,
                tool_name, provider, category, status, arguments_json,
                effective_arguments_json, metadata_json, created_at, updated_at
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'planned', ?11, ?12, ?13, ?14, ?14)",
            params![
                &id,
                &run.workspace_id,
                &run.tool_call_id,
                &trace.parent_run_id,
                origin,
                &trace.step_id,
                sequence,
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
        tx.commit()
            .context("commit create dispatcher tool run transaction")?;
        load_tool_run_on_conn(&conn, &id)
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
                   AND status NOT IN (
                       'succeeded', 'recoverable_error', 'fatal_error', 'cancelled',
                       'failed', 'internal_error'
                   )",
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

    /// 将外层 LLM tool 结果消息绑定到整棵内部调用树及其产物。
    ///
    /// ToolProgram 子调用不会生成伪造的 LLM tool message，因此执行期间的
    /// message_id 为空；外层结果落库后在同一事务中统一补挂，保证按消息截断
    /// 时不会遗留孤儿 child run/artifact。
    pub fn attach_tool_run_tree_message(
        &self,
        root_run_id: &str,
        message_id: &str,
    ) -> Result<Vec<DispatcherToolRunRecord>> {
        let mut conn = self.conn()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("begin attach dispatcher tool run tree message transaction")?;
        let workspace_id = tx
            .query_row(
                "SELECT workspace_id FROM dispatcher_tool_runs WHERE id = ?1",
                params![root_run_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .context("load dispatcher tool run root before message attach")?
            .with_context(|| format!("dispatcher tool run root not found: {root_run_id}"))?;
        let message_workspace_id = tx
            .query_row(
                "SELECT workspace_id FROM dispatcher_messages WHERE id = ?1",
                params![message_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .context("load dispatcher message before tool run attach")?
            .with_context(|| format!("dispatcher message not found: {message_id}"))?;
        if message_workspace_id != workspace_id {
            anyhow::bail!(
                "dispatcher message {message_id} belongs to workspace {message_workspace_id}, not {workspace_id}"
            );
        }

        let timestamp = now();
        tx.execute(
            "WITH RECURSIVE tool_run_tree(id) AS (
                 SELECT id FROM dispatcher_tool_runs
                 WHERE id = ?1 AND workspace_id = ?2
                 UNION ALL
                 SELECT child.id
                 FROM dispatcher_tool_runs child
                 INNER JOIN tool_run_tree tree ON child.parent_run_id = tree.id
                 WHERE child.workspace_id = ?2
             )
             UPDATE dispatcher_tool_runs
             SET message_id = ?3, updated_at = ?4
             WHERE id IN (SELECT id FROM tool_run_tree)",
            params![root_run_id, &workspace_id, message_id, &timestamp],
        )
        .context("attach dispatcher tool run tree message")?;
        tx.execute(
            "WITH RECURSIVE tool_run_tree(id) AS (
                 SELECT id FROM dispatcher_tool_runs
                 WHERE id = ?1 AND workspace_id = ?2
                 UNION ALL
                 SELECT child.id
                 FROM dispatcher_tool_runs child
                 INNER JOIN tool_run_tree tree ON child.parent_run_id = tree.id
                 WHERE child.workspace_id = ?2
             )
             UPDATE dispatcher_tool_artifacts
             SET message_id = ?3
             WHERE workspace_id = ?2
               AND tool_run_id IN (SELECT id FROM tool_run_tree)",
            params![root_run_id, &workspace_id, message_id],
        )
        .context("attach dispatcher tool run tree artifacts to message")?;
        tx.commit()
            .context("commit attach dispatcher tool run tree message transaction")?;
        self.list_tool_run_tree(&workspace_id, root_run_id)
    }

    /// 外层 tool message 持久化失败时删除尚未绑定消息的整棵运行树。
    /// parent_run_id 与 artifact.tool_run_id 均为 ON DELETE CASCADE，因此只需
    /// 精确删除根记录；workspace 条件防止跨会话误删。
    pub fn delete_unattached_tool_run_tree(
        &self,
        workspace_id: &str,
        root_run_id: &str,
    ) -> Result<()> {
        let conn = self.conn()?;
        let changed = conn
            .execute(
                "DELETE FROM dispatcher_tool_runs
                 WHERE id = ?1 AND workspace_id = ?2 AND message_id IS NULL",
                params![root_run_id, workspace_id],
            )
            .context("delete unattached dispatcher tool run tree")?;
        if changed == 0 {
            anyhow::bail!(
                "unattached dispatcher tool run root not found: {root_run_id} in {workspace_id}"
            );
        }
        Ok(())
    }

    pub fn load_tool_run(&self, id: &str) -> Result<DispatcherToolRunRecord> {
        let conn = self.conn()?;
        load_tool_run_on_conn(&conn, id)
    }

    /// 按外层模型工具调用定位完整运行树。`root_run_id` 可用于实时卡片精确命中；
    /// 历史消息没有该字段时，使用同一 tool_call_id 的最新根运行。
    pub fn list_tool_run_tree_for_call(
        &self,
        workspace_id: &str,
        tool_call_id: &str,
        root_run_id: Option<&str>,
    ) -> Result<Vec<DispatcherToolRunRecord>> {
        let conn = self.conn()?;
        let root_id = conn
            .query_row(
                "SELECT id
                 FROM dispatcher_tool_runs
                 WHERE workspace_id = ?1
                   AND parent_run_id IS NULL
                   AND (
                       (?3 IS NOT NULL AND id = ?3)
                       OR (?3 IS NULL AND tool_call_id = ?2)
                   )
                 ORDER BY created_at DESC, rowid DESC
                 LIMIT 1",
                params![workspace_id, tool_call_id, root_run_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .context("locate dispatcher tool run tree root")?;
        let Some(root_id) = root_id else {
            return Ok(Vec::new());
        };
        drop(conn);
        self.list_tool_run_tree(workspace_id, &root_id)
    }

    /// 按父子关系返回一棵工具调用树，结果为稳定的深度优先顺序。
    pub fn list_tool_run_tree(
        &self,
        workspace_id: &str,
        root_run_id: &str,
    ) -> Result<Vec<DispatcherToolRunRecord>> {
        let conn = self.conn()?;
        let sql = format!(
            "WITH RECURSIVE tool_run_tree(id, sort_path) AS (
                 SELECT id, printf('%s', id)
                 FROM dispatcher_tool_runs
                 WHERE id = ?2 AND workspace_id = ?1
                 UNION ALL
                 SELECT child.id,
                        tool_run_tree.sort_path || printf('/%020d-%s', child.sequence, child.id)
                 FROM dispatcher_tool_runs child
                 INNER JOIN tool_run_tree ON child.parent_run_id = tool_run_tree.id
                 WHERE child.workspace_id = ?1
             )
             SELECT {TOOL_RUN_SELECT_COLUMNS}
             FROM dispatcher_tool_runs r
             INNER JOIN tool_run_tree tree ON tree.id = r.id
             ORDER BY tree.sort_path"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![workspace_id, root_run_id], map_tool_run)?;
        let runs = rows
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("list dispatcher tool run tree")?;
        if runs.is_empty() {
            anyhow::bail!(
                "dispatcher tool run root not found in workspace {workspace_id}: {root_run_id}"
            );
        }
        Ok(runs)
    }

    #[allow(dead_code)]
    pub async fn create_tool_run_with_trace_async(
        &self,
        run: NewToolRun,
        trace: ToolRunTraceContext,
    ) -> Result<DispatcherToolRunRecord> {
        let db = self.clone();
        tokio::task::spawn_blocking(move || db.create_tool_run_with_trace(run, trace))
            .await
            .context("create_tool_run_with_trace spawn_blocking")?
    }

    pub async fn mark_tool_run_started_async(&self, id: &str) -> Result<DispatcherToolRunRecord> {
        let db = self.clone();
        let id = id.to_string();
        tokio::task::spawn_blocking(move || db.mark_tool_run_started(&id))
            .await
            .context("mark_tool_run_started spawn_blocking")?
    }

    pub async fn load_tool_run_async(&self, id: &str) -> Result<DispatcherToolRunRecord> {
        let db = self.clone();
        let id = id.to_string();
        tokio::task::spawn_blocking(move || db.load_tool_run(&id))
            .await
            .context("load_tool_run spawn_blocking")?
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

    pub async fn attach_tool_run_tree_message_async(
        &self,
        root_run_id: &str,
        message_id: &str,
    ) -> Result<Vec<DispatcherToolRunRecord>> {
        let db = self.clone();
        let root_run_id = root_run_id.to_string();
        let message_id = message_id.to_string();
        tokio::task::spawn_blocking(move || {
            db.attach_tool_run_tree_message(&root_run_id, &message_id)
        })
        .await
        .context("attach_tool_run_tree_message spawn_blocking")?
    }

    pub async fn delete_unattached_tool_run_tree_async(
        &self,
        workspace_id: &str,
        root_run_id: &str,
    ) -> Result<()> {
        let db = self.clone();
        let workspace_id = workspace_id.to_string();
        let root_run_id = root_run_id.to_string();
        tokio::task::spawn_blocking(move || {
            db.delete_unattached_tool_run_tree(&workspace_id, &root_run_id)
        })
        .await
        .context("delete_unattached_tool_run_tree spawn_blocking")?
    }
}

fn load_tool_run_on_conn(conn: &Connection, id: &str) -> Result<DispatcherToolRunRecord> {
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

/// 运行到达这些状态后生命周期即结束，不得被后续 finish 覆盖（单向推进）。
fn is_terminal_run_status(status: &str) -> bool {
    matches!(
        status,
        "succeeded"
            | "recoverable_error"
            | "fatal_error"
            | "cancelled"
            | "failed"
            | "internal_error"
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

    #[test]
    fn traced_runs_round_trip_and_tree_is_depth_first() {
        let db = test_db();
        let root = db.create_tool_run(new_run("ws")).expect("create root");
        assert_eq!(root.parent_run_id, None);
        assert_eq!(root.origin, TOOL_RUN_ORIGIN_MODEL);
        assert_eq!(root.step_id, None);
        assert_eq!(root.sequence, 0);

        let first = db
            .create_tool_run_with_trace(
                new_run("ws"),
                ToolRunTraceContext {
                    parent_run_id: Some(root.id.clone()),
                    origin: "tool_program".to_string(),
                    step_id: Some("search".to_string()),
                    sequence: 0,
                },
            )
            .expect("create first child");
        let grandchild = db
            .create_tool_run_with_trace(
                new_run("ws"),
                ToolRunTraceContext {
                    parent_run_id: Some(first.id.clone()),
                    origin: "tool_program".to_string(),
                    step_id: Some("read".to_string()),
                    sequence: 0,
                },
            )
            .expect("create grandchild");
        let second = db
            .create_tool_run_with_trace(
                new_run("ws"),
                ToolRunTraceContext {
                    parent_run_id: Some(root.id.clone()),
                    origin: "tool_program".to_string(),
                    step_id: Some("summarize".to_string()),
                    sequence: 1,
                },
            )
            .expect("create second child");

        assert_eq!(first.parent_run_id.as_deref(), Some(root.id.as_str()));
        assert_eq!(first.origin, "tool_program");
        assert_eq!(first.step_id.as_deref(), Some("search"));
        assert_eq!(first.sequence, 0);

        let tree = db
            .list_tool_run_tree("ws", &root.id)
            .expect("list tool run tree");
        assert_eq!(
            tree.iter().map(|run| run.id.as_str()).collect::<Vec<_>>(),
            vec![
                root.id.as_str(),
                first.id.as_str(),
                grandchild.id.as_str(),
                second.id.as_str()
            ]
        );
    }

    #[test]
    fn tree_for_call_selects_only_the_requested_root() {
        let db = test_db();
        let mut first_run = new_run("ws");
        first_run.tool_call_id = "shared-call".to_string();
        let first = db.create_tool_run(first_run).expect("create first root");
        let child = db
            .create_tool_run_with_trace(
                new_run("ws"),
                ToolRunTraceContext {
                    parent_run_id: Some(first.id.clone()),
                    origin: "tool_program".to_string(),
                    step_id: Some("read".to_string()),
                    sequence: 1,
                },
            )
            .expect("create child");
        let mut unrelated_run = new_run("other-ws");
        unrelated_run.tool_call_id = "shared-call".to_string();
        db.create_tool_run(unrelated_run)
            .expect("create unrelated root");

        let tree = db
            .list_tool_run_tree_for_call("ws", "shared-call", None)
            .expect("load by tool call");
        assert_eq!(
            tree.iter().map(|run| run.id.as_str()).collect::<Vec<_>>(),
            vec![first.id.as_str(), child.id.as_str()]
        );

        let explicit = db
            .list_tool_run_tree_for_call("ws", "ignored", Some(&first.id))
            .expect("load by root id");
        assert_eq!(explicit.len(), 2);
        assert!(db
            .list_tool_run_tree_for_call("other-ws", "ignored", Some(&first.id))
            .expect("cross-workspace root is invisible")
            .is_empty());
    }

    #[test]
    fn traced_run_rejects_cross_workspace_parent_and_duplicate_sequence() {
        let db = test_db();
        let root = db.create_tool_run(new_run("ws-a")).expect("create root");

        let cross_workspace = db
            .create_tool_run_with_trace(
                new_run("ws-b"),
                ToolRunTraceContext {
                    parent_run_id: Some(root.id.clone()),
                    origin: "tool_program".to_string(),
                    step_id: None,
                    sequence: 0,
                },
            )
            .expect_err("cross-workspace parent must fail");
        assert!(cross_workspace.to_string().contains("belongs to workspace"));

        let trace = ToolRunTraceContext {
            parent_run_id: Some(root.id.clone()),
            origin: "tool_program".to_string(),
            step_id: Some("step".to_string()),
            sequence: 0,
        };
        db.create_tool_run_with_trace(new_run("ws-a"), trace.clone())
            .expect("create first child");
        let duplicate = db
            .create_tool_run_with_trace(new_run("ws-a"), trace)
            .expect_err("duplicate sibling sequence must fail");
        assert!(duplicate.to_string().contains("create dispatcher tool run"));
    }

    #[test]
    fn deleting_parent_cascades_to_descendants() {
        let db = test_db();
        let root = db.create_tool_run(new_run("ws")).expect("create root");
        let child = db
            .create_tool_run_with_trace(
                new_run("ws"),
                ToolRunTraceContext {
                    parent_run_id: Some(root.id.clone()),
                    origin: "tool_program".to_string(),
                    step_id: None,
                    sequence: 0,
                },
            )
            .expect("create child");

        assert!(db
            .delete_unattached_tool_run_tree("other-ws", &root.id)
            .expect_err("cross-workspace delete must fail")
            .to_string()
            .contains("not found"));
        db.delete_unattached_tool_run_tree("ws", &root.id)
            .expect("delete parent tree");
        let conn = db.conn().expect("db conn");
        let remaining: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM dispatcher_tool_runs WHERE id IN (?1, ?2)",
                params![&root.id, &child.id],
                |row| row.get(0),
            )
            .expect("count remaining runs");
        assert_eq!(remaining, 0);
    }

    #[test]
    fn failed_and_internal_error_are_terminal() {
        let db = test_db();
        for status in ["failed", "internal_error"] {
            let run = db.create_tool_run(new_run("ws")).expect("create run");
            db.mark_tool_run_started(&run.id).expect("start run");
            let terminal = db
                .finish_tool_run(&run.id, finish(status))
                .expect("finish run");
            assert_eq!(terminal.status, status);

            let unchanged = db
                .finish_tool_run(&run.id, finish("succeeded"))
                .expect("second finish is no-op");
            assert_eq!(unchanged.status, status);
        }
    }
}
