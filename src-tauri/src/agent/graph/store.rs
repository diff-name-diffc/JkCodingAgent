//! PI 图计划、运行代、节点快照和 Agent 活动的 SQLite 存储。

use std::sync::Arc;

use anyhow::{Context, Result};
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{params, OptionalExtension, TransactionBehavior};

use super::types::{
    AgentActivity, GraphDefinition, GraphNodeRunRecord, GraphPlanRecord, GraphRunDetail,
    GraphRunSummary, PLAN_DRAFT,
};

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

    pub(crate) fn create_plan(
        &self,
        workspace_id: &str,
        definition: &GraphDefinition,
    ) -> Result<GraphPlanRecord> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().timestamp_millis();
        let definition_json = serde_json::to_string(definition).context("序列化图定义失败")?;
        self.conn()?.execute(
            "INSERT INTO graph_plans (id,workspace_id,title,summary,definition_json,status,state_json,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,'{}',?7,?7)",
            params![id, workspace_id, definition.title.trim(), definition.summary.trim(), definition_json, PLAN_DRAFT, now],
        ).context("创建图计划失败")?;
        self.get_plan(&id)?.context("读取刚创建的图计划")
    }

    pub(crate) fn get_plan(&self, plan_id: &str) -> Result<Option<GraphPlanRecord>> {
        let conn = self.conn()?;
        let mut plan = query_plan(&conn, "WHERE id=?1", params![plan_id])?;
        drop(conn);
        if let Some(record) = plan.as_mut() {
            record.runs = self.list_runs(plan_id)?;
            if let Some(run_id) = &record.latest_run_id {
                record.node_runs = self.list_node_runs(run_id)?;
            }
        }
        Ok(plan)
    }

    pub(crate) fn latest_plan_for_workspace(
        &self,
        workspace_id: &str,
    ) -> Result<Option<GraphPlanRecord>> {
        let conn = self.conn()?;
        let plan = query_plan(
            &conn,
            "WHERE workspace_id=?1 ORDER BY updated_at DESC LIMIT 1",
            params![workspace_id],
        )?;
        drop(conn);
        match plan {
            Some(plan) => self.get_plan(&plan.id),
            None => Ok(None),
        }
    }

    pub(crate) fn update_plan_definition(
        &self,
        plan_id: &str,
        definition: &GraphDefinition,
    ) -> Result<()> {
        let json = serde_json::to_string(definition)?;
        self.conn()?.execute(
            "UPDATE graph_plans SET title=?2,summary=?3,definition_json=?4,updated_at=?5 WHERE id=?1",
            params![plan_id, definition.title.trim(), definition.summary.trim(), json, chrono::Utc::now().timestamp_millis()],
        ).context("更新图计划定义失败")?;
        Ok(())
    }

    pub(crate) fn update_plan_status(&self, plan_id: &str, status: &str) -> Result<()> {
        self.conn()?.execute(
            "UPDATE graph_plans SET status=?2,updated_at=?3 WHERE id=?1",
            params![plan_id, status, chrono::Utc::now().timestamp_millis()],
        )?;
        Ok(())
    }

    pub(crate) fn update_plan_state(&self, plan_id: &str, state_json: &str) -> Result<()> {
        self.conn()?.execute(
            "UPDATE graph_plans SET state_json=?2,updated_at=?3 WHERE id=?1",
            params![plan_id, state_json, chrono::Utc::now().timestamp_millis()],
        )?;
        Ok(())
    }

    pub(crate) fn create_run(&self, plan_id: &str) -> Result<GraphRunSummary> {
        let mut conn = self.conn()?;
        // 先取得写锁，再读取 attempt_no；否则两个池连接可能读到相同的 MAX。
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let attempt_no: i64 = tx.query_row(
            "SELECT COALESCE(MAX(attempt_no),0)+1 FROM graph_runs WHERE plan_id=?1",
            params![plan_id],
            |row| row.get(0),
        )?;
        let run = GraphRunSummary {
            id: uuid::Uuid::new_v4().to_string(),
            plan_id: plan_id.to_string(),
            attempt_no,
            status: "running".into(),
            started_at: chrono::Utc::now().timestamp_millis(),
            finished_at: None,
        };
        tx.execute("INSERT INTO graph_runs (id,plan_id,attempt_no,status,started_at) VALUES (?1,?2,?3,?4,?5)", params![run.id,run.plan_id,run.attempt_no,run.status,run.started_at])?;
        tx.execute("UPDATE graph_plans SET latest_run_id=?2,status='running',state_json='{}',updated_at=?3 WHERE id=?1", params![plan_id,run.id,run.started_at])?;
        tx.commit()?;
        Ok(run)
    }

    pub(crate) fn finish_run(&self, run_id: &str, status: &str) -> Result<()> {
        self.conn()?.execute(
            "UPDATE graph_runs SET status=?2,finished_at=?3 WHERE id=?1",
            params![run_id, status, chrono::Utc::now().timestamp_millis()],
        )?;
        Ok(())
    }

    pub(crate) fn fail_interrupted_runs(&self, plan_id: Option<&str>) -> Result<usize> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        let now = chrono::Utc::now().timestamp_millis();
        let reason = "进程中断";
        tx.execute(
            "UPDATE graph_plans SET status='failed',updated_at=?2
             WHERE id IN (SELECT plan_id FROM graph_runs
                 WHERE status='running' AND (?1 IS NULL OR plan_id=?1))",
            params![plan_id, now],
        )?;
        tx.execute(
            "UPDATE graph_node_runs
             SET status='failed',phase='finalizing',error_text=?2,
                 finished_at=COALESCE(finished_at,?3),
                 duration_ms=CASE WHEN started_at IS NULL THEN duration_ms ELSE ?3-started_at END
             WHERE status IN ('pending','running')
               AND run_id IN (SELECT id FROM graph_runs
                   WHERE status='running' AND (?1 IS NULL OR plan_id=?1))",
            params![plan_id, reason, now],
        )?;
        let changed = tx.execute(
            "UPDATE graph_runs SET status='failed',finished_at=?2
             WHERE status='running' AND (?1 IS NULL OR plan_id=?1)",
            params![plan_id, now],
        )?;
        tx.commit()?;
        Ok(changed)
    }

    pub(crate) fn list_runs(&self, plan_id: &str) -> Result<Vec<GraphRunSummary>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare("SELECT id,plan_id,attempt_no,status,started_at,finished_at FROM graph_runs WHERE plan_id=?1 ORDER BY attempt_no DESC")?;
        let rows = stmt
            .query_map(params![plan_id], map_run)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub(crate) fn save_node_run(&self, run: &GraphNodeRunRecord) -> Result<()> {
        let affected = serde_json::to_string(&run.affected_files)?;
        self.conn()?.execute(
            "INSERT OR REPLACE INTO graph_node_runs (run_id,plan_id,node_id,status,phase,model_ref,model_label,model_category,base_tool_group,special_tools_json,input_text,output_text,error_text,started_at,finished_at,duration_ms,usage_json,affected_files_json,tool_call_count) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19)",
            params![run.run_id,run.plan_id,run.node_id,run.status,run.phase,run.model_ref,run.model_label,run.model_category,run.base_tool_group,run.special_tools_json,run.input_text,run.output_text,run.error_text,run.started_at,run.finished_at,run.duration_ms,run.usage_json,affected,run.tool_call_count],
        ).context("保存节点运行记录失败")?;
        Ok(())
    }

    pub(crate) fn list_node_runs(&self, run_id: &str) -> Result<Vec<GraphNodeRunRecord>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare("SELECT run_id,plan_id,node_id,status,phase,model_ref,model_label,model_category,base_tool_group,special_tools_json,input_text,output_text,error_text,started_at,finished_at,duration_ms,usage_json,affected_files_json,tool_call_count FROM graph_node_runs WHERE run_id=?1 ORDER BY rowid")?;
        let rows = stmt
            .query_map(params![run_id], map_node_run)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub(crate) fn save_activity(&self, activity: &AgentActivity) -> Result<()> {
        self.conn()?.execute(
            "INSERT INTO graph_node_activities (id,run_id,node_id,sequence,kind,status,title,content,payload_json,started_at,finished_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11) ON CONFLICT(run_id,node_id,sequence) DO UPDATE SET status=excluded.status,title=excluded.title,content=excluded.content,payload_json=excluded.payload_json,finished_at=excluded.finished_at",
            params![activity.id,activity.run_id,activity.node_id,activity.sequence,activity.kind,activity.status,activity.title,activity.content,activity.payload_json,activity.started_at,activity.finished_at],
        )?;
        Ok(())
    }

    pub(crate) fn get_run_detail(&self, run_id: &str) -> Result<Option<GraphRunDetail>> {
        let conn = self.conn()?;
        let run = conn.query_row("SELECT id,plan_id,attempt_no,status,started_at,finished_at FROM graph_runs WHERE id=?1", params![run_id], map_run).optional()?;
        let Some(run) = run else { return Ok(None) };
        let mut stmt = conn.prepare("SELECT id,run_id,node_id,sequence,kind,status,title,content,payload_json,started_at,finished_at FROM graph_node_activities WHERE run_id=?1 ORDER BY node_id,sequence")?;
        let activities = stmt
            .query_map(params![run_id], |row| {
                Ok(AgentActivity {
                    id: row.get(0)?,
                    run_id: row.get(1)?,
                    node_id: row.get(2)?,
                    sequence: row.get(3)?,
                    kind: row.get(4)?,
                    status: row.get(5)?,
                    title: row.get(6)?,
                    content: row.get(7)?,
                    payload_json: row.get(8)?,
                    started_at: row.get(9)?,
                    finished_at: row.get(10)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);
        drop(conn);
        Ok(Some(GraphRunDetail {
            run,
            node_runs: self.list_node_runs(run_id)?,
            activities,
        }))
    }

    pub(crate) async fn get_plan_async(&self, id: &str) -> Result<Option<GraphPlanRecord>> {
        let s = self.clone();
        let id = id.to_string();
        tokio::task::spawn_blocking(move || s.get_plan(&id))
            .await
            .context("读取图计划任务失败")?
    }
    pub(crate) async fn latest_plan_for_workspace_async(
        &self,
        id: &str,
    ) -> Result<Option<GraphPlanRecord>> {
        let s = self.clone();
        let id = id.to_string();
        tokio::task::spawn_blocking(move || s.latest_plan_for_workspace(&id))
            .await
            .context("读取会话图计划任务失败")?
    }
    pub(crate) async fn update_plan_definition_async(
        &self,
        id: &str,
        d: &GraphDefinition,
    ) -> Result<()> {
        let s = self.clone();
        let id = id.to_string();
        let d = d.clone();
        tokio::task::spawn_blocking(move || s.update_plan_definition(&id, &d))
            .await
            .context("更新图定义任务失败")?
    }
    pub(crate) async fn update_plan_status_async(&self, id: &str, status: &str) -> Result<()> {
        let s = self.clone();
        let id = id.to_string();
        let status = status.to_string();
        tokio::task::spawn_blocking(move || s.update_plan_status(&id, &status))
            .await
            .context("更新图状态任务失败")?
    }
    pub(crate) async fn update_plan_state_async(&self, id: &str, state: &str) -> Result<()> {
        let s = self.clone();
        let id = id.to_string();
        let state = state.to_string();
        tokio::task::spawn_blocking(move || s.update_plan_state(&id, &state))
            .await
            .context("更新图 state 任务失败")?
    }
    pub(crate) async fn create_run_async(&self, id: &str) -> Result<GraphRunSummary> {
        let s = self.clone();
        let id = id.to_string();
        tokio::task::spawn_blocking(move || s.create_run(&id))
            .await
            .context("创建图运行任务失败")?
    }
    pub(crate) async fn finish_run_async(&self, id: &str, status: &str) -> Result<()> {
        let s = self.clone();
        let id = id.to_string();
        let status = status.to_string();
        tokio::task::spawn_blocking(move || s.finish_run(&id, &status))
            .await
            .context("结束图运行任务失败")?
    }
    pub(crate) async fn fail_interrupted_runs_async(&self, plan_id: Option<&str>) -> Result<usize> {
        let s = self.clone();
        let plan_id = plan_id.map(str::to_string);
        tokio::task::spawn_blocking(move || s.fail_interrupted_runs(plan_id.as_deref()))
            .await
            .context("恢复中断图运行任务失败")?
    }
    pub(crate) async fn save_node_run_async(&self, run: &GraphNodeRunRecord) -> Result<()> {
        let s = self.clone();
        let run = run.clone();
        tokio::task::spawn_blocking(move || s.save_node_run(&run))
            .await
            .context("保存节点任务失败")?
    }
    pub(crate) async fn list_node_runs_async(&self, id: &str) -> Result<Vec<GraphNodeRunRecord>> {
        let s = self.clone();
        let id = id.to_string();
        tokio::task::spawn_blocking(move || s.list_node_runs(&id))
            .await
            .context("查询节点任务失败")?
    }
    pub(crate) async fn save_activity_async(&self, a: &AgentActivity) -> Result<()> {
        let s = self.clone();
        let a = a.clone();
        tokio::task::spawn_blocking(move || s.save_activity(&a))
            .await
            .context("保存活动任务失败")?
    }
    pub(crate) async fn get_run_detail_async(&self, id: &str) -> Result<Option<GraphRunDetail>> {
        let s = self.clone();
        let id = id.to_string();
        tokio::task::spawn_blocking(move || s.get_run_detail(&id))
            .await
            .context("读取运行详情任务失败")?
    }
}

fn query_plan(
    conn: &rusqlite::Connection,
    clause: &str,
    values: impl rusqlite::Params,
) -> Result<Option<GraphPlanRecord>> {
    let sql=format!("SELECT id,workspace_id,title,summary,definition_json,status,state_json,latest_run_id,created_at,updated_at FROM graph_plans {clause}");
    Ok(conn
        .query_row(&sql, values, |row| {
            Ok(GraphPlanRecord {
                id: row.get(0)?,
                workspace_id: row.get(1)?,
                title: row.get(2)?,
                summary: row.get(3)?,
                definition_json: row.get(4)?,
                status: row.get(5)?,
                state_json: row.get(6)?,
                latest_run_id: row.get(7)?,
                created_at: row.get(8)?,
                updated_at: row.get(9)?,
                runs: vec![],
                node_runs: vec![],
            })
        })
        .optional()?)
}
fn map_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<GraphRunSummary> {
    Ok(GraphRunSummary {
        id: row.get(0)?,
        plan_id: row.get(1)?,
        attempt_no: row.get(2)?,
        status: row.get(3)?,
        started_at: row.get(4)?,
        finished_at: row.get(5)?,
    })
}
fn map_node_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<GraphNodeRunRecord> {
    let affected: String = row.get(17)?;
    Ok(GraphNodeRunRecord {
        run_id: row.get(0)?,
        plan_id: row.get(1)?,
        node_id: row.get(2)?,
        status: row.get(3)?,
        phase: row.get(4)?,
        model_ref: row.get(5)?,
        model_label: row.get(6)?,
        model_category: row.get(7)?,
        base_tool_group: row.get(8)?,
        special_tools_json: row.get(9)?,
        input_text: row.get(10)?,
        output_text: row.get(11)?,
        error_text: row.get(12)?,
        started_at: row.get(13)?,
        finished_at: row.get(14)?,
        duration_ms: row.get(15)?,
        usage_json: row.get(16)?,
        affected_files: serde_json::from_str(&affected).unwrap_or_default(),
        tool_call_count: row.get(18)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::graph::types::{BaseToolGroup, GraphNode};
    fn test_db() -> crate::agent::db::DispatcherDb {
        crate::agent::db::DispatcherDb::new(
            std::env::temp_dir().join(format!("aha-graph-v2-{}.sqlite3", uuid::Uuid::new_v4())),
        )
        .unwrap()
    }
    fn definition() -> GraphDefinition {
        GraphDefinition {
            version: 2,
            title: "测试".into(),
            summary: String::new(),
            state_keys: vec![],
            nodes: vec![GraphNode {
                id: "n1".into(),
                title: "节点".into(),
                role: String::new(),
                model_ref: "m1".into(),
                base_tool_group: BaseToolGroup::Coding,
                special_tools: vec![],
                task: "task".into(),
                depends_on: vec![],
                inject_state_keys: vec![],
                output_key: "out".into(),
            }],
        }
    }
    #[test]
    fn preserves_run_attempts() {
        let db = test_db();
        let store = GraphStore::new(&db);
        let plan = store.create_plan("w", &definition()).unwrap();
        let a = store.create_run(&plan.id).unwrap();
        store.finish_run(&a.id, "completed").unwrap();
        let b = store.create_run(&plan.id).unwrap();
        assert_eq!(b.attempt_no, 2);
        assert_eq!(store.get_plan(&plan.id).unwrap().unwrap().runs.len(), 2);
    }

    #[test]
    fn allocates_unique_attempts_concurrently() {
        let db = test_db();
        let store = GraphStore::new(&db);
        let plan = store.create_plan("w", &definition()).unwrap();
        let worker_count = 8;
        let barrier = Arc::new(std::sync::Barrier::new(worker_count));
        let handles = (0..worker_count)
            .map(|_| {
                let store = store.clone();
                let plan_id = plan.id.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    store.create_run(&plan_id).unwrap().attempt_no
                })
            })
            .collect::<Vec<_>>();

        let mut attempts = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        attempts.sort_unstable();
        assert_eq!(attempts, (1..=worker_count as i64).collect::<Vec<_>>());
    }

    #[test]
    fn recovers_interrupted_run() {
        let db = test_db();
        let store = GraphStore::new(&db);
        let plan = store.create_plan("w", &definition()).unwrap();
        let run = store.create_run(&plan.id).unwrap();
        store
            .save_node_run(&GraphNodeRunRecord::pending(
                &run.id,
                &plan.id,
                &definition().nodes[0],
            ))
            .unwrap();

        assert_eq!(store.fail_interrupted_runs(None).unwrap(), 1);
        let detail = store.get_run_detail(&run.id).unwrap().unwrap();
        assert_eq!(detail.run.status, "failed");
        assert_eq!(detail.node_runs[0].status, "failed");
        assert_eq!(detail.node_runs[0].error_text.as_deref(), Some("进程中断"));
    }
}
