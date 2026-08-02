//! `graph_plans` / `graph_node_runs` 的 CRUD。
//!
//! 复用 `DispatcherDb` 的 r2d2 连接池（`DispatcherDb::pool()`），不新建连接。
//! 同步方法走连接池直接执行；`*_async` 变体用 `spawn_blocking` 包裹，
//! 遵守「Tauri async 命令内禁止直接阻塞」的约束。

use std::sync::Arc;

use anyhow::{Context, Result};
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{params, OptionalExtension};

use super::types::{GraphDefinition, GraphNodeRunRecord, GraphPlanRecord, PLAN_DRAFT};

#[derive(Debug, Clone)]
pub(crate) struct GraphStore {
    pool: Arc<Pool<SqliteConnectionManager>>,
}

impl GraphStore {
    pub(crate) fn new(db: &crate::agent::db::DispatcherDb) -> Self {
        Self { pool: db.pool() }
    }

    fn conn(&self) -> Result<r2d2::PooledConnection<SqliteConnectionManager>> {
        self.pool.get().context("获取数据库连接")
    }

    // ── graph_plans ─────────────────────────────────────────────────────────

    /// 以 draft 状态创建图计划；title/summary 从定义中提取冗余存储，便于列表展示。
    pub(crate) fn create_plan(
        &self,
        workspace_id: &str,
        definition: &GraphDefinition,
    ) -> Result<GraphPlanRecord> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().timestamp_millis();
        let definition_json = serde_json::to_string(definition).context("序列化图定义失败")?;
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO graph_plans
             (id, workspace_id, title, summary, definition_json, status, state_json, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, '{}', ?7, ?7)",
            params![
                &id,
                workspace_id,
                definition.title.trim(),
                definition.summary.trim(),
                &definition_json,
                PLAN_DRAFT,
                now,
            ],
        )
        .context("创建图计划失败")?;

        Ok(GraphPlanRecord {
            id,
            workspace_id: workspace_id.to_string(),
            title: definition.title.trim().to_string(),
            summary: definition.summary.trim().to_string(),
            definition_json,
            status: PLAN_DRAFT.to_string(),
            state_json: "{}".to_string(),
            created_at: now,
            updated_at: now,
            node_runs: Vec::new(),
        })
    }

    /// 读取图计划（含节点运行记录）。
    pub(crate) fn get_plan(&self, plan_id: &str) -> Result<Option<GraphPlanRecord>> {
        let conn = self.conn()?;
        let plan = query_plan(&conn, "WHERE id = ?1", params![plan_id])?;
        drop(conn);
        match plan {
            Some(mut plan) => {
                plan.node_runs = self.list_node_runs(plan_id)?;
                Ok(Some(plan))
            }
            None => Ok(None),
        }
    }

    /// 会话最近一次更新的图计划（会话头部入口）。
    pub(crate) fn latest_plan_for_workspace(
        &self,
        workspace_id: &str,
    ) -> Result<Option<GraphPlanRecord>> {
        let conn = self.conn()?;
        let plan = query_plan(
            &conn,
            "WHERE workspace_id = ?1 ORDER BY updated_at DESC LIMIT 1",
            params![workspace_id],
        )?;
        drop(conn);
        match plan {
            Some(mut plan) => {
                let plan_id = plan.id.clone();
                plan.node_runs = self.list_node_runs(&plan_id)?;
                Ok(Some(plan))
            }
            None => Ok(None),
        }
    }

    /// 用户确认前编辑图定义（仅 draft 态允许，状态校验在调用方）。
    pub(crate) fn update_plan_definition(
        &self,
        plan_id: &str,
        definition: &GraphDefinition,
    ) -> Result<()> {
        let definition_json = serde_json::to_string(definition).context("序列化图定义失败")?;
        let now = chrono::Utc::now().timestamp_millis();
        let conn = self.conn()?;
        conn.execute(
            "UPDATE graph_plans
             SET title = ?2, summary = ?3, definition_json = ?4, updated_at = ?5
             WHERE id = ?1",
            params![
                plan_id,
                definition.title.trim(),
                definition.summary.trim(),
                &definition_json,
                now
            ],
        )
        .context("更新图计划定义失败")?;
        Ok(())
    }

    pub(crate) fn update_plan_status(&self, plan_id: &str, status: &str) -> Result<()> {
        let now = chrono::Utc::now().timestamp_millis();
        let conn = self.conn()?;
        conn.execute(
            "UPDATE graph_plans SET status = ?2, updated_at = ?3 WHERE id = ?1",
            params![plan_id, status, now],
        )
        .context("更新图计划状态失败")?;
        Ok(())
    }

    pub(crate) fn update_plan_state(&self, plan_id: &str, state_json: &str) -> Result<()> {
        let now = chrono::Utc::now().timestamp_millis();
        let conn = self.conn()?;
        conn.execute(
            "UPDATE graph_plans SET state_json = ?2, updated_at = ?3 WHERE id = ?1",
            params![plan_id, state_json, now],
        )
        .context("更新图共享状态失败")?;
        Ok(())
    }

    // ── graph_node_runs ─────────────────────────────────────────────────────

    /// 覆盖式写入节点运行记录（主键 (plan_id, node_id)，重跑覆盖，v1 不保留历史代）。
    pub(crate) fn save_node_run(&self, run: &GraphNodeRunRecord) -> Result<()> {
        let affected_files_json =
            serde_json::to_string(&run.affected_files).context("序列化节点影响文件失败")?;
        let conn = self.conn()?;
        conn.execute(
            "INSERT OR REPLACE INTO graph_node_runs
             (plan_id, node_id, agent_kind, agent_id, status, input_text, output_text,
              error_text, trace_tool_call_id, started_at, finished_at, duration_ms,
              affected_files_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                run.plan_id,
                run.node_id,
                run.agent_kind,
                run.agent_id,
                run.status,
                run.input_text,
                run.output_text,
                run.error_text,
                run.trace_tool_call_id,
                run.started_at,
                run.finished_at,
                run.duration_ms,
                affected_files_json,
            ],
        )
        .context("保存节点运行记录失败")?;
        Ok(())
    }

    pub(crate) fn list_node_runs(&self, plan_id: &str) -> Result<Vec<GraphNodeRunRecord>> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT plan_id, node_id, agent_kind, agent_id, status, input_text, output_text,
                        error_text, trace_tool_call_id, started_at, finished_at, duration_ms,
                        affected_files_json
                 FROM graph_node_runs
                 WHERE plan_id = ?1
                 ORDER BY rowid",
            )
            .context("准备节点运行查询失败")?;
        let runs = stmt
            .query_map(params![plan_id], |row| {
                let affected_files_json: String = row.get(12)?;
                Ok(GraphNodeRunRecord {
                    plan_id: row.get(0)?,
                    node_id: row.get(1)?,
                    agent_kind: row.get(2)?,
                    agent_id: row.get(3)?,
                    status: row.get(4)?,
                    input_text: row.get(5)?,
                    output_text: row.get(6)?,
                    error_text: row.get(7)?,
                    trace_tool_call_id: row.get(8)?,
                    started_at: row.get(9)?,
                    finished_at: row.get(10)?,
                    duration_ms: row.get(11)?,
                    affected_files: serde_json::from_str(&affected_files_json).unwrap_or_default(),
                })
            })
            .context("查询节点运行记录失败")?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("读取节点运行记录失败")?;
        Ok(runs)
    }

    // ── async 变体（spawn_blocking 包裹，避免阻塞 Tauri 主线程） ─────────────

    pub(crate) async fn get_plan_async(&self, plan_id: &str) -> Result<Option<GraphPlanRecord>> {
        let store = self.clone();
        let plan_id = plan_id.to_string();
        tokio::task::spawn_blocking(move || store.get_plan(&plan_id))
            .await
            .context("读取图计划任务失败")?
    }

    pub(crate) async fn update_plan_definition_async(
        &self,
        plan_id: &str,
        definition: &GraphDefinition,
    ) -> Result<()> {
        let store = self.clone();
        let plan_id = plan_id.to_string();
        let definition = definition.clone();
        tokio::task::spawn_blocking(move || store.update_plan_definition(&plan_id, &definition))
            .await
            .context("更新图计划定义任务失败")?
    }

    pub(crate) async fn latest_plan_for_workspace_async(
        &self,
        workspace_id: &str,
    ) -> Result<Option<GraphPlanRecord>> {
        let store = self.clone();
        let workspace_id = workspace_id.to_string();
        tokio::task::spawn_blocking(move || store.latest_plan_for_workspace(&workspace_id))
            .await
            .context("读取会话图计划任务失败")?
    }

    pub(crate) async fn update_plan_status_async(&self, plan_id: &str, status: &str) -> Result<()> {
        let store = self.clone();
        let plan_id = plan_id.to_string();
        let status = status.to_string();
        tokio::task::spawn_blocking(move || store.update_plan_status(&plan_id, &status))
            .await
            .context("更新图计划状态任务失败")?
    }

    pub(crate) async fn update_plan_state_async(&self, plan_id: &str, state_json: &str) -> Result<()> {
        let store = self.clone();
        let plan_id = plan_id.to_string();
        let state_json = state_json.to_string();
        tokio::task::spawn_blocking(move || store.update_plan_state(&plan_id, &state_json))
            .await
            .context("更新图共享状态任务失败")?
    }

    pub(crate) async fn save_node_run_async(&self, run: &GraphNodeRunRecord) -> Result<()> {
        let store = self.clone();
        let run = run.clone();
        tokio::task::spawn_blocking(move || store.save_node_run(&run))
            .await
            .context("保存节点运行记录任务失败")?
    }

    pub(crate) async fn list_node_runs_async(
        &self,
        plan_id: &str,
    ) -> Result<Vec<GraphNodeRunRecord>> {
        let store = self.clone();
        let plan_id = plan_id.to_string();
        tokio::task::spawn_blocking(move || store.list_node_runs(&plan_id))
            .await
            .context("查询节点运行记录任务失败")?
    }
}

fn query_plan(
    conn: &rusqlite::Connection,
    where_clause: &str,
    params: impl rusqlite::Params,
) -> Result<Option<GraphPlanRecord>> {
    let sql = format!(
        "SELECT id, workspace_id, title, summary, definition_json, status, state_json,
                created_at, updated_at
         FROM graph_plans
         {where_clause}"
    );
    let plan = conn
        .query_row(
            &sql,
            params,
            |row| {
                Ok(GraphPlanRecord {
                    id: row.get(0)?,
                    workspace_id: row.get(1)?,
                    title: row.get(2)?,
                    summary: row.get(3)?,
                    definition_json: row.get(4)?,
                    status: row.get(5)?,
                    state_json: row.get(6)?,
                    created_at: row.get(7)?,
                    updated_at: row.get(8)?,
                    node_runs: Vec::new(),
                })
            },
        )
        .optional()
        .context("查询图计划失败")?;
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::graph::types::{GraphNode, GraphNodeAgent, PLAN_CONFIRMED};

    fn test_db() -> crate::agent::db::DispatcherDb {
        let path = std::env::temp_dir().join(format!(
            "jkcodingagent-graph-store-test-{}.sqlite3",
            uuid::Uuid::new_v4(),
        ));
        crate::agent::db::DispatcherDb::new(path).expect("create test dispatcher db")
    }

    fn definition() -> GraphDefinition {
        GraphDefinition {
            title: "测试图".to_string(),
            summary: "摘要".to_string(),
            state_keys: Vec::new(),
            nodes: vec![GraphNode {
                id: "n1".to_string(),
                title: "节点一".to_string(),
                role: "角色".to_string(),
                agent: GraphNodeAgent::Claude,
                task: "任务".to_string(),
                depends_on: Vec::new(),
                inject_state_keys: Vec::new(),
                output_key: "out_1".to_string(),
            }],
        }
    }

    #[test]
    fn schema_v23_creates_graph_tables() {
        let db = test_db();
        let conn = db.conn().expect("conn");
        for table in ["graph_plans", "graph_node_runs"] {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name = ?1",
                    rusqlite::params![table],
                    |row| row.get(0),
                )
                .expect("check table");
            assert_eq!(count, 1, "table {table} should exist");
        }
        let version: i32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("read schema version");
        assert_eq!(version, 23);
    }

    #[test]
    fn plan_crud_round_trip() {
        let db = test_db();
        let store = GraphStore::new(&db);

        let plan = store.create_plan("ws-1", &definition()).expect("create plan");
        assert_eq!(plan.status, PLAN_DRAFT);
        assert_eq!(plan.title, "测试图");

        let loaded = store
            .get_plan(&plan.id)
            .expect("get plan")
            .expect("plan exists");
        assert_eq!(loaded.workspace_id, "ws-1");
        let parsed: GraphDefinition =
            serde_json::from_str(&loaded.definition_json).expect("parse definition");
        assert_eq!(parsed.nodes.len(), 1);

        let latest = store
            .latest_plan_for_workspace("ws-1")
            .expect("latest plan")
            .expect("latest exists");
        assert_eq!(latest.id, plan.id);

        store
            .update_plan_status(&plan.id, PLAN_CONFIRMED)
            .expect("update status");
        store
            .update_plan_state(&plan.id, r#"{"out_1":"done"}"#)
            .expect("update state");
        let updated = store
            .get_plan(&plan.id)
            .expect("get plan")
            .expect("plan exists");
        assert_eq!(updated.status, PLAN_CONFIRMED);
        assert_eq!(updated.state_json, r#"{"out_1":"done"}"#);
    }

    #[test]
    fn node_runs_upsert_and_list() {
        let db = test_db();
        let store = GraphStore::new(&db);
        let plan = store.create_plan("ws-1", &definition()).expect("create plan");
        let node = &definition().nodes[0];

        let mut run = GraphNodeRunRecord::pending(&plan.id, node);
        store.save_node_run(&run).expect("save pending");
        run.status = "succeeded".to_string();
        run.output_text = "输出".to_string();
        run.started_at = Some(1);
        run.finished_at = Some(2);
        run.duration_ms = Some(1);
        store.save_node_run(&run).expect("overwrite run");

        let runs = store.list_node_runs(&plan.id).expect("list runs");
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, "succeeded");
        assert_eq!(runs[0].output_text, "输出");
        assert_eq!(runs[0].duration_ms, Some(1));
    }
}

