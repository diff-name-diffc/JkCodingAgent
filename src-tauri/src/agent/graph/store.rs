//! PI 图计划、运行代、节点快照和 Agent 活动的 SQLite 存储。

use std::sync::Arc;

use anyhow::{Context, Result};
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{params, OptionalExtension, TransactionBehavior};

use super::types::{
    AgentActivity, GraphDefinition, GraphModelStat, GraphNodeRunRecord, GraphPlanRecord,
    GraphRunDetail, GraphRunSummary, NODE_PHASE_CACHED, NODE_SUCCEEDED, PLAN_DRAFT, PLAN_RUNNING,
    RUN_MODE_FULL, RUN_MODE_RESUME, VERDICT_UNKNOWN,
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

    /// 登记新计划。`requirement` 为提交时刻的需求快照；`initial_state_json` 为
    /// 初始共享 state（修复图为被继承 run 的 state 快照，普通图为 "{}"）。
    /// 初始快照单独落 `initial_state_json` 列：full 模式重跑修复图时据此恢复，
    /// 避免把上次失败运行残留的部分 state 带进新一轮执行。
    pub(crate) fn create_plan(
        &self,
        workspace_id: &str,
        definition: &GraphDefinition,
        requirement: &str,
        initial_state_json: &str,
    ) -> Result<GraphPlanRecord> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().timestamp_millis();
        let definition_json = serde_json::to_string(definition).context("序列化图定义失败")?;
        let inherits_plan_id = definition.inherits_from.as_ref().map(|i| i.plan_id.clone());
        let inherits_run_id = definition.inherits_from.as_ref().map(|i| i.run_id.clone());
        self.conn()?.execute(
            "INSERT INTO graph_plans (id,workspace_id,title,summary,definition_json,status,state_json,requirement,inherits_plan_id,inherits_run_id,initial_state_json,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?12)",
            params![id, workspace_id, definition.title.trim(), definition.summary.trim(), definition_json, PLAN_DRAFT, initial_state_json, requirement.trim(), inherits_plan_id, inherits_run_id, initial_state_json, now],
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

    /// 创建 full 模式运行。普通图清空共享 state；修复图（inherits_plan_id 非空）
    /// 恢复**提交时种入的初始继承快照**（initial_state_json 列），而不是 plan 当前
    /// state——运行中 persist_and_emit_state 会把部分产物写进 state_json，若修复图
    /// 首次 full 运行中途失败后重跑，直接沿用当前 state 会带入上次失败运行的残留。
    pub(crate) fn create_run(&self, plan_id: &str) -> Result<GraphRunSummary> {
        let mut conn = self.conn()?;
        // 先取得写锁，再读取 attempt_no；否则两个池连接可能读到相同的 MAX。
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let attempt_no: i64 = tx.query_row(
            "SELECT COALESCE(MAX(attempt_no),0)+1 FROM graph_runs WHERE plan_id=?1",
            params![plan_id],
            |row| row.get(0),
        )?;
        let now = chrono::Utc::now().timestamp_millis();
        let run = GraphRunSummary {
            id: uuid::Uuid::new_v4().to_string(),
            plan_id: plan_id.to_string(),
            attempt_no,
            status: PLAN_RUNNING.into(),
            mode: RUN_MODE_FULL.into(),
            verdict_status: VERDICT_UNKNOWN.into(),
            verdict_reason: String::new(),
            started_at: now,
            finished_at: None,
        };
        tx.execute(
            "INSERT INTO graph_runs (id,plan_id,attempt_no,status,mode,verdict_status,started_at) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![run.id, run.plan_id, run.attempt_no, run.status, run.mode, run.verdict_status, run.started_at],
        )?;
        tx.execute(
            "UPDATE graph_plans SET latest_run_id=?2,status=?3,
                state_json=CASE WHEN inherits_plan_id IS NOT NULL THEN initial_state_json ELSE '{}' END,
                updated_at=?4 WHERE id=?1",
            params![plan_id, run.id, PLAN_RUNNING, now],
        )?;
        tx.commit()?;
        Ok(run)
    }

    /// 断点续跑：单事务内创建 resume 运行、复制源运行全部成功节点（phase=cached）、
    /// 更新 latest_run_id。共享 state 保持 plan 当前值（上次运行结束时的状态），不重置。
    /// `from_run_id` 必须属于 `plan_id`：复制行写入目标 plan_id，避免跨 plan 误传时
    /// graph_node_runs.plan_id 与 graph_runs.plan_id 不一致，破坏报告关联与后续续跑。
    pub(crate) fn create_resume_run(&self, plan_id: &str, from_run_id: &str) -> Result<GraphRunSummary> {
        let mut conn = self.conn()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let from_belongs: i64 = tx.query_row(
            "SELECT COUNT(*) FROM graph_runs WHERE id=?1 AND plan_id=?2",
            params![from_run_id, plan_id],
            |row| row.get(0),
        )?;
        if from_belongs == 0 {
            anyhow::bail!("续跑源运行 {from_run_id} 不属于图计划 {plan_id}");
        }
        let attempt_no: i64 = tx.query_row(
            "SELECT COALESCE(MAX(attempt_no),0)+1 FROM graph_runs WHERE plan_id=?1",
            params![plan_id],
            |row| row.get(0),
        )?;
        let now = chrono::Utc::now().timestamp_millis();
        let run = GraphRunSummary {
            id: uuid::Uuid::new_v4().to_string(),
            plan_id: plan_id.to_string(),
            attempt_no,
            status: PLAN_RUNNING.into(),
            mode: RUN_MODE_RESUME.into(),
            verdict_status: VERDICT_UNKNOWN.into(),
            verdict_reason: String::new(),
            started_at: now,
            finished_at: None,
        };
        tx.execute(
            "INSERT INTO graph_runs (id,plan_id,attempt_no,status,mode,verdict_status,started_at) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![run.id, run.plan_id, run.attempt_no, run.status, run.mode, run.verdict_status, run.started_at],
        )?;
        // graph_node_runs 主键为 (run_id, node_id)，跨 run 复制只需替换 run_id；
        // plan_id 守卫确保只复制本计划的节点行，且复制行统一落目标 plan_id。
        tx.execute(
            "INSERT OR REPLACE INTO graph_node_runs
                (run_id,plan_id,node_id,status,phase,model_ref,model_label,model_category,
                 base_tool_group,special_tools_json,input_text,output_text,error_text,
                 started_at,finished_at,duration_ms,usage_json,affected_files_json,tool_call_count,retry_count)
             SELECT ?1,?4,node_id,status,?2,model_ref,model_label,model_category,
                 base_tool_group,special_tools_json,input_text,output_text,error_text,
                 started_at,finished_at,duration_ms,usage_json,affected_files_json,tool_call_count,retry_count
             FROM graph_node_runs WHERE run_id=?3 AND plan_id=?4 AND status=?5",
            params![run.id, NODE_PHASE_CACHED, from_run_id, plan_id, NODE_SUCCEEDED],
        )?;
        tx.execute(
            "UPDATE graph_plans SET latest_run_id=?2,status=?3,updated_at=?4 WHERE id=?1",
            params![plan_id, run.id, PLAN_RUNNING, now],
        )?;
        tx.commit()?;
        Ok(run)
    }

    /// 最近一次运行（attempt_no 最大），供图运行报告与断点续跑入口使用。
    pub(crate) fn get_latest_run(&self, plan_id: &str) -> Result<Option<GraphRunSummary>> {
        self.conn()?
            .query_row(
                "SELECT id,plan_id,attempt_no,status,mode,verdict_status,verdict_reason,started_at,finished_at FROM graph_runs WHERE plan_id=?1 ORDER BY attempt_no DESC LIMIT 1",
                params![plan_id],
                map_run,
            )
            .optional()
            .context("读取最近图运行失败")
    }

    /// 写入验收结论（run 收尾由 verifier 产出）。
    pub(crate) fn update_run_verdict(&self, run_id: &str, status: &str, reason: &str) -> Result<()> {
        self.conn()?.execute(
            "UPDATE graph_runs SET verdict_status=?2,verdict_reason=?3 WHERE id=?1",
            params![run_id, status, reason],
        )?;
        Ok(())
    }

    /// 模型×基础工具组的历史节点运行统计（仅统计已结算节点），供 Harness 目录回注。
    /// 按 workspace 限定范围，与按会话隔离的 Harness 目录保持一致，避免其他项目的
    /// 统计数据污染当前项目的选型信号；`phase='cached'` 的续跑复用节点不重复计数。
    pub(crate) fn node_run_stats(&self, workspace_id: &str) -> Result<Vec<GraphModelStat>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT nr.model_ref, nr.base_tool_group, COUNT(*),
                    SUM(CASE WHEN nr.status='failed' THEN 1 ELSE 0 END),
                    COALESCE(AVG(nr.duration_ms),0)
             FROM graph_node_runs nr
             JOIN graph_plans p ON p.id = nr.plan_id
             WHERE nr.status IN ('succeeded','failed')
               AND nr.phase <> ?2
               AND p.workspace_id = ?1
             GROUP BY nr.model_ref, nr.base_tool_group",
        )?;
        let rows = stmt
            .query_map(params![workspace_id, NODE_PHASE_CACHED], |row| {
                Ok(GraphModelStat {
                    model_ref: row.get(0)?,
                    base_tool_group: row.get(1)?,
                    runs: row.get(2)?,
                    failures: row.get(3)?,
                    avg_duration_ms: row.get::<_, f64>(4)? as i64,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
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
        let mut stmt = conn.prepare("SELECT id,plan_id,attempt_no,status,mode,verdict_status,verdict_reason,started_at,finished_at FROM graph_runs WHERE plan_id=?1 ORDER BY attempt_no DESC")?;
        let rows = stmt
            .query_map(params![plan_id], map_run)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub(crate) fn save_node_run(&self, run: &GraphNodeRunRecord) -> Result<()> {
        let affected = serde_json::to_string(&run.affected_files)?;
        self.conn()?.execute(
            "INSERT OR REPLACE INTO graph_node_runs (run_id,plan_id,node_id,status,phase,model_ref,model_label,model_category,base_tool_group,special_tools_json,input_text,output_text,error_text,started_at,finished_at,duration_ms,usage_json,affected_files_json,tool_call_count,retry_count) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20)",
            params![run.run_id,run.plan_id,run.node_id,run.status,run.phase,run.model_ref,run.model_label,run.model_category,run.base_tool_group,run.special_tools_json,run.input_text,run.output_text,run.error_text,run.started_at,run.finished_at,run.duration_ms,run.usage_json,affected,run.tool_call_count,run.retry_count],
        ).context("保存节点运行记录失败")?;
        Ok(())
    }

    pub(crate) fn list_node_runs(&self, run_id: &str) -> Result<Vec<GraphNodeRunRecord>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare("SELECT run_id,plan_id,node_id,status,phase,model_ref,model_label,model_category,base_tool_group,special_tools_json,input_text,output_text,error_text,started_at,finished_at,duration_ms,usage_json,affected_files_json,tool_call_count,retry_count FROM graph_node_runs WHERE run_id=?1 ORDER BY rowid")?;
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
        let run = conn.query_row("SELECT id,plan_id,attempt_no,status,mode,verdict_status,verdict_reason,started_at,finished_at FROM graph_runs WHERE id=?1", params![run_id], map_run).optional()?;
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
    pub(crate) async fn create_plan_async(
        &self,
        workspace_id: &str,
        definition: &GraphDefinition,
        requirement: &str,
        initial_state_json: &str,
    ) -> Result<GraphPlanRecord> {
        let s = self.clone();
        let workspace_id = workspace_id.to_string();
        let definition = definition.clone();
        let requirement = requirement.to_string();
        let initial_state_json = initial_state_json.to_string();
        tokio::task::spawn_blocking(move || {
            s.create_plan(&workspace_id, &definition, &requirement, &initial_state_json)
        })
        .await
        .context("创建图计划任务失败")?
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
    pub(crate) async fn create_resume_run_async(
        &self,
        plan_id: &str,
        from_run_id: &str,
    ) -> Result<GraphRunSummary> {
        let s = self.clone();
        let plan_id = plan_id.to_string();
        let from_run_id = from_run_id.to_string();
        tokio::task::spawn_blocking(move || s.create_resume_run(&plan_id, &from_run_id))
            .await
            .context("创建续跑运行任务失败")?
    }
    pub(crate) async fn get_latest_run_async(&self, plan_id: &str) -> Result<Option<GraphRunSummary>> {
        let s = self.clone();
        let plan_id = plan_id.to_string();
        tokio::task::spawn_blocking(move || s.get_latest_run(&plan_id))
            .await
            .context("读取最近图运行任务失败")?
    }
    pub(crate) async fn update_run_verdict_async(
        &self,
        run_id: &str,
        status: &str,
        reason: &str,
    ) -> Result<()> {
        let s = self.clone();
        let run_id = run_id.to_string();
        let status = status.to_string();
        let reason = reason.to_string();
        tokio::task::spawn_blocking(move || s.update_run_verdict(&run_id, &status, &reason))
            .await
            .context("写入验收结论任务失败")?
    }
    pub(crate) async fn node_run_stats_async(&self, workspace_id: &str) -> Result<Vec<GraphModelStat>> {
        let s = self.clone();
        let workspace_id = workspace_id.to_string();
        tokio::task::spawn_blocking(move || s.node_run_stats(&workspace_id))
            .await
            .context("统计节点运行历史任务失败")?
    }
}

fn query_plan(
    conn: &rusqlite::Connection,
    clause: &str,
    values: impl rusqlite::Params,
) -> Result<Option<GraphPlanRecord>> {
    let sql=format!("SELECT id,workspace_id,title,summary,definition_json,status,state_json,requirement,inherits_plan_id,inherits_run_id,latest_run_id,created_at,updated_at FROM graph_plans {clause}");
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
                requirement: row.get(7)?,
                inherits_plan_id: row.get(8)?,
                inherits_run_id: row.get(9)?,
                latest_run_id: row.get(10)?,
                created_at: row.get(11)?,
                updated_at: row.get(12)?,
                runs: vec![],
                node_runs: vec![],
            })
        })
        .optional()?)
}
fn map_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<GraphRunSummary> {
    // 严格读取：v25 起这些列均 NOT NULL DEFAULT，读取失败说明 schema 漂移，
    // 显式报错优于静默默认值（会把迁移异常掩盖成正常数据）。
    let verdict_status: String = row.get(5)?;
    Ok(GraphRunSummary {
        id: row.get(0)?,
        plan_id: row.get(1)?,
        attempt_no: row.get(2)?,
        status: row.get(3)?,
        mode: row.get(4)?,
        // 历史行可能存空串：归一为 unknown，保证「尚未/未能验收」只有一种表示。
        verdict_status: if verdict_status.is_empty() {
            VERDICT_UNKNOWN.to_string()
        } else {
            verdict_status
        },
        verdict_reason: row.get(6)?,
        started_at: row.get(7)?,
        finished_at: row.get(8)?,
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
        retry_count: row.get(19)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::graph::types::{BaseToolGroup, GraphInherits, GraphNode};
    fn test_db() -> crate::agent::db::DispatcherDb {
        crate::agent::db::DispatcherDb::new(
            std::env::temp_dir().join(format!("aha-graph-v3-{}.sqlite3", uuid::Uuid::new_v4())),
        )
        .unwrap()
    }
    fn node(id: &str) -> GraphNode {
        GraphNode {
            id: id.into(),
            title: id.into(),
            role: String::new(),
            model_ref: "m1".into(),
            base_tool_group: BaseToolGroup::Coding,
            special_tools: vec![],
            task: "task".into(),
            depends_on: vec![],
            inject_state_keys: vec![],
            output_key: format!("out_{id}"),
            expected_files: vec![],
            export_policy: Default::default(),
        }
    }
    fn definition() -> GraphDefinition {
        GraphDefinition {
            version: 3,
            title: "测试".into(),
            summary: String::new(),
            state_keys: vec![],
            nodes: vec![node("n1")],
            inherits_from: None,
        }
    }
    fn create_plain_plan(store: &GraphStore) -> GraphPlanRecord {
        store.create_plan("w", &definition(), "原始需求", "{}").unwrap()
    }
    #[test]
    fn preserves_run_attempts() {
        let db = test_db();
        let store = GraphStore::new(&db);
        let plan = create_plain_plan(&store);
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
        let plan = create_plain_plan(&store);
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
        let plan = create_plain_plan(&store);
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

    #[test]
    fn create_run_resets_state_for_plain_plan_but_keeps_inherited_state() {
        let db = test_db();
        let store = GraphStore::new(&db);

        // 普通图：full run 清空 state。
        let plain = create_plain_plan(&store);
        store.update_plan_state(&plain.id, r#"{"leftover":"x"}"#).unwrap();
        store.create_run(&plain.id).unwrap();
        assert_eq!(store.get_plan(&plain.id).unwrap().unwrap().state_json, "{}");

        // 修复图：full run 恢复提交时种入的初始继承快照——即使上一次运行
        // 中途失败后 state 残留了部分产物，重跑也从初始快照重新开始。
        let mut inherited_def = definition();
        inherited_def.inherits_from = Some(GraphInherits {
            plan_id: plain.id.clone(),
            run_id: "r-old".into(),
        });
        let inherited = store
            .create_plan("w", &inherited_def, "修复需求", r#"{"auth_analysis":"结论"}"#)
            .unwrap();
        store
            .update_plan_state(&inherited.id, r#"{"auth_analysis":"结论","partial":"失败运行残留"}"#)
            .unwrap();
        store.create_run(&inherited.id).unwrap();
        let reloaded = store.get_plan(&inherited.id).unwrap().unwrap();
        assert_eq!(reloaded.state_json, r#"{"auth_analysis":"结论"}"#);
        assert_eq!(reloaded.requirement, "修复需求");
        assert_eq!(reloaded.inherits_plan_id.as_deref(), Some(plain.id.as_str()));
    }

    #[test]
    fn resume_run_copies_only_succeeded_nodes_and_keeps_state() {
        let db = test_db();
        let store = GraphStore::new(&db);
        let plan = create_plain_plan(&store);
        let base = store.create_run(&plan.id).unwrap();

        let mut succeeded = GraphNodeRunRecord::pending(&base.id, &plan.id, &node("n1"));
        succeeded.status = "succeeded".into();
        succeeded.output_text = "产出".into();
        succeeded.usage_json = r#"{"prompt_tokens":10,"completion_tokens":5}"#.into();
        store.save_node_run(&succeeded).unwrap();
        let mut failed = GraphNodeRunRecord::pending(&base.id, &plan.id, &node("n2"));
        failed.status = "failed".into();
        failed.error_text = Some("boom".into());
        store.save_node_run(&failed).unwrap();
        store.finish_run(&base.id, "failed").unwrap();
        store.update_plan_state(&plan.id, r#"{"out_n1":"产出"}"#).unwrap();

        let resumed = store.create_resume_run(&plan.id, &base.id).unwrap();
        assert_eq!(resumed.mode, RUN_MODE_RESUME);
        assert_eq!(resumed.attempt_no, 2);

        let copied = store.list_node_runs(&resumed.id).unwrap();
        assert_eq!(copied.len(), 1, "只复制成功节点");
        assert_eq!(copied[0].node_id, "n1");
        assert_eq!(copied[0].status, "succeeded");
        assert_eq!(copied[0].phase, NODE_PHASE_CACHED);
        assert_eq!(copied[0].output_text, "产出");

        // state 与 plan 状态：state 保留、plan 进入 running、latest_run 指向新 run。
        let reloaded = store.get_plan(&plan.id).unwrap().unwrap();
        assert_eq!(reloaded.state_json, r#"{"out_n1":"产出"}"#);
        assert_eq!(reloaded.status, "running");
        assert_eq!(reloaded.latest_run_id.as_deref(), Some(resumed.id.as_str()));
        assert_eq!(
            store.get_latest_run(&plan.id).unwrap().unwrap().id,
            resumed.id
        );
    }

    #[test]
    fn node_run_stats_aggregates_settled_nodes() {
        let db = test_db();
        let store = GraphStore::new(&db);
        let plan = create_plain_plan(&store);
        let run = store.create_run(&plan.id).unwrap();

        let mut ok = GraphNodeRunRecord::pending(&run.id, &plan.id, &node("n1"));
        ok.status = "succeeded".into();
        ok.duration_ms = Some(1000);
        store.save_node_run(&ok).unwrap();
        let mut bad = GraphNodeRunRecord::pending(&run.id, &plan.id, &node("n2"));
        bad.status = "failed".into();
        bad.duration_ms = Some(3000);
        store.save_node_run(&bad).unwrap();
        let mut pending = GraphNodeRunRecord::pending(&run.id, &plan.id, &node("n3"));
        pending.status = "pending".into();
        store.save_node_run(&pending).unwrap();
        // 续跑复用节点（phase=cached）不应重复计数。
        let mut cached = GraphNodeRunRecord::pending(&run.id, &plan.id, &node("n4"));
        cached.status = "succeeded".into();
        cached.phase = NODE_PHASE_CACHED.into();
        cached.duration_ms = Some(9999);
        store.save_node_run(&cached).unwrap();

        let stats = store.node_run_stats("w").unwrap();
        assert_eq!(stats.len(), 1, "按 model×group 聚合");
        let stat = &stats[0];
        assert_eq!(stat.model_ref, "m1");
        assert_eq!(stat.base_tool_group, "coding");
        assert_eq!(stat.runs, 2, "pending 与 cached 不计入");
        assert_eq!(stat.failures, 1);
        assert_eq!(stat.avg_duration_ms, 2000);

        // 按 workspace 隔离：其他会话的节点统计不混入当前会话。
        let other_plan = store
            .create_plan("other-ws", &definition(), "其他会话需求", "{}")
            .unwrap();
        let other_run = store.create_run(&other_plan.id).unwrap();
        let mut other_node = GraphNodeRunRecord::pending(&other_run.id, &other_plan.id, &node("x"));
        other_node.status = "succeeded".into();
        other_node.duration_ms = Some(500);
        store.save_node_run(&other_node).unwrap();
        let stats = store.node_run_stats("w").unwrap();
        assert_eq!(stats[0].runs, 2, "其他 workspace 的节点不计入");
    }

    #[test]
    fn resume_run_rejects_source_run_from_other_plan() {
        let db = test_db();
        let store = GraphStore::new(&db);
        let plan_a = create_plain_plan(&store);
        let plan_b = create_plain_plan(&store);
        let run_a = store.create_run(&plan_a.id).unwrap();
        store.finish_run(&run_a.id, "failed").unwrap();

        let error = store
            .create_resume_run(&plan_b.id, &run_a.id)
            .unwrap_err();
        assert!(format!("{error:#}").contains("不属于"));
    }

    /// v25→v26 升级回归：旧 v25 库（无 initial_state_json 列、带冗余
    /// idx_graph_activities_node、无 plan_id 索引）打开后应原子升级到 v26。
    #[test]
    fn migrates_legacy_v25_graph_tables_to_v26() {
        let path = std::env::temp_dir().join(format!(
            "aha-graph-v25-legacy-{}.sqlite3",
            uuid::Uuid::new_v4()
        ));
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            // 按旧 v25 迁移的落库形态建图表（无 initial_state_json、含冗余索引）。
            conn.execute_batch(
                "
                CREATE TABLE graph_plans (
                    id TEXT PRIMARY KEY,
                    workspace_id TEXT NOT NULL,
                    title TEXT NOT NULL,
                    summary TEXT NOT NULL DEFAULT '',
                    definition_json TEXT NOT NULL,
                    status TEXT NOT NULL DEFAULT 'draft',
                    state_json TEXT NOT NULL DEFAULT '{}',
                    requirement TEXT NOT NULL DEFAULT '',
                    inherits_plan_id TEXT,
                    inherits_run_id TEXT,
                    latest_run_id TEXT,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                );
                CREATE TABLE graph_runs (
                    id TEXT PRIMARY KEY,
                    plan_id TEXT NOT NULL,
                    attempt_no INTEGER NOT NULL,
                    status TEXT NOT NULL,
                    mode TEXT NOT NULL DEFAULT 'full',
                    verdict_status TEXT NOT NULL DEFAULT '',
                    verdict_reason TEXT NOT NULL DEFAULT '',
                    started_at INTEGER NOT NULL,
                    finished_at INTEGER,
                    UNIQUE(plan_id, attempt_no)
                );
                CREATE TABLE graph_node_runs (
                    run_id TEXT NOT NULL,
                    plan_id TEXT NOT NULL,
                    node_id TEXT NOT NULL,
                    status TEXT NOT NULL DEFAULT 'pending',
                    phase TEXT NOT NULL DEFAULT 'starting',
                    model_ref TEXT NOT NULL,
                    model_label TEXT NOT NULL,
                    model_category TEXT NOT NULL,
                    base_tool_group TEXT NOT NULL,
                    special_tools_json TEXT NOT NULL DEFAULT '[]',
                    input_text TEXT NOT NULL DEFAULT '',
                    output_text TEXT NOT NULL DEFAULT '',
                    error_text TEXT,
                    started_at INTEGER,
                    finished_at INTEGER,
                    duration_ms INTEGER,
                    usage_json TEXT NOT NULL DEFAULT '{}',
                    affected_files_json TEXT NOT NULL DEFAULT '[]',
                    tool_call_count INTEGER NOT NULL DEFAULT 0,
                    retry_count INTEGER NOT NULL DEFAULT 0,
                    PRIMARY KEY(run_id, node_id)
                );
                CREATE TABLE graph_node_activities (
                    id TEXT PRIMARY KEY,
                    run_id TEXT NOT NULL,
                    node_id TEXT NOT NULL,
                    sequence INTEGER NOT NULL,
                    kind TEXT NOT NULL,
                    status TEXT NOT NULL,
                    title TEXT NOT NULL DEFAULT '',
                    content TEXT NOT NULL DEFAULT '',
                    payload_json TEXT NOT NULL DEFAULT '{}',
                    started_at INTEGER NOT NULL,
                    finished_at INTEGER,
                    UNIQUE(run_id, node_id, sequence)
                );
                CREATE INDEX idx_graph_activities_node
                    ON graph_node_activities(run_id, node_id, sequence);
                INSERT INTO graph_plans (id,workspace_id,title,definition_json,state_json,created_at,updated_at)
                    VALUES ('p-plain','w','普通图','{}','{\"leftover\":\"x\"}',1,1);
                INSERT INTO graph_plans (id,workspace_id,title,definition_json,state_json,inherits_plan_id,inherits_run_id,created_at,updated_at)
                    VALUES ('p-fix','w','修复图','{}','{\"auth\":\"结论\"}','p-plain','r-old',1,1);
                PRAGMA user_version = 25;
                ",
            )
            .unwrap();
        }

        // 重新打开触发 v25→v26 迁移。
        let db = crate::agent::db::DispatcherDb::new(path).unwrap();
        let conn = db.conn().unwrap();
        let version: i32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 26);

        let index_exists = |name: &str| -> i64 {
            conn.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name=?1",
                params![name],
                |row| row.get(0),
            )
            .unwrap()
        };
        assert_eq!(index_exists("idx_graph_node_runs_plan"), 1, "新增 plan_id 索引");
        assert_eq!(index_exists("idx_graph_activities_node"), 0, "冗余索引被清理");

        // initial_state_json 回填：普通图为空，修复图沿用当前 state（近似）。
        let initial = |plan_id: &str| -> String {
            conn.query_row(
                "SELECT initial_state_json FROM graph_plans WHERE id=?1",
                params![plan_id],
                |row| row.get(0),
            )
            .unwrap()
        };
        assert_eq!(initial("p-plain"), "{}");
        assert_eq!(initial("p-fix"), "{\"auth\":\"结论\"}");

        // 历史空串 verdict 读取时归一为 unknown。
        conn.execute(
            "INSERT INTO graph_runs (id,plan_id,attempt_no,status,mode,started_at) VALUES ('r1','p-plain',1,'completed','full',1)",
            params![],
        )
        .unwrap();
        let store = GraphStore::new(&db);
        let latest = store.get_latest_run("p-plain").unwrap().unwrap();
        assert_eq!(latest.verdict_status, "unknown");
    }

    #[test]
    fn verdict_round_trips() {
        let db = test_db();
        let store = GraphStore::new(&db);
        let plan = create_plain_plan(&store);
        let run = store.create_run(&plan.id).unwrap();
        store.update_run_verdict(&run.id, "partial", "部分节点失败但有产出").unwrap();
        let latest = store.get_latest_run(&plan.id).unwrap().unwrap();
        assert_eq!(latest.verdict_status, "partial");
        assert_eq!(latest.verdict_reason, "部分节点失败但有产出");
    }
}
