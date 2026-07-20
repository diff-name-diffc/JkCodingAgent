use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::Utc;
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::config::SubAgentConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubAgentRecord {
    pub id: String,
    pub name: String,
    pub description: String,
    pub config_json: String,
    pub enabled: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubAgentRunTraceRecord {
    pub workspace_id: String,
    pub tool_call_id: String,
    pub agent_id: String,
    pub status: String,
    pub events_json: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
}

pub fn ensure_sub_agent_tables_tx(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS sub_agents (
            id          TEXT PRIMARY KEY,
            name        TEXT NOT NULL,
            description TEXT NOT NULL,
            config_json TEXT NOT NULL,
            enabled     INTEGER NOT NULL DEFAULT 1,
            created_at  INTEGER NOT NULL,
            updated_at  INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS global_sub_agents (
            sub_agent_id TEXT PRIMARY KEY,
            FOREIGN KEY (sub_agent_id) REFERENCES sub_agents(id) ON DELETE CASCADE
        );
        ",
    )
    .context("create sub_agent tables")
}

pub fn ensure_sub_agent_trace_table_tx(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS sub_agent_run_traces (
            workspace_id TEXT NOT NULL,
            tool_call_id TEXT NOT NULL,
            agent_id TEXT NOT NULL,
            status TEXT NOT NULL,
            events_json TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (workspace_id, tool_call_id),
            FOREIGN KEY (workspace_id) REFERENCES dispatcher_sessions(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_sub_agent_run_traces_workspace_updated
            ON sub_agent_run_traces(workspace_id, updated_at DESC);
        ",
    )
    .context("create sub_agent_run_traces table")
}

pub fn seed_browser_agent_force_tx(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    let config = SubAgentConfig::browser_agent_default();
    let config_json = serde_json::to_string(&config).context("serialize browser-agent config")?;

    tx.execute(
        "INSERT OR REPLACE INTO sub_agents (id, name, description, config_json, enabled, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            config.agent_id,
            config.agent_name,
            config.description,
            config_json,
            config.enabled as i32,
            config.created_at,
            config.updated_at,
        ],
    )
    .context("seed browser-agent")?;

    tx.execute(
        "INSERT OR IGNORE INTO global_sub_agents (sub_agent_id) VALUES (?1)",
        params![config.agent_id],
    )
    .context("seed browser-agent global association")?;

    Ok(())
}

pub fn seed_browser_agent_if_missing_tx(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    let existing = tx
        .query_row(
            "SELECT COUNT(*) FROM sub_agents WHERE id = 'browser-agent'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0);

    if existing > 0 {
        return Ok(());
    }

    seed_browser_agent_force_tx(tx)
}

fn row_to_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<SubAgentRecord> {
    Ok(SubAgentRecord {
        id: row.get(0)?,
        name: row.get(1)?,
        description: row.get(2)?,
        config_json: row.get(3)?,
        enabled: row.get::<_, i32>(4)? != 0,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

pub struct SubAgentDb {
    pool: Arc<Pool<SqliteConnectionManager>>,
}

impl SubAgentDb {
    pub fn new(pool: Arc<Pool<SqliteConnectionManager>>) -> Self {
        Self { pool }
    }

    fn conn(&self) -> Result<r2d2::PooledConnection<SqliteConnectionManager>> {
        self.pool
            .get()
            .with_context(|| "get sub_agent db connection")
    }

    pub fn list_all(&self) -> Result<Vec<SubAgentRecord>> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, name, description, config_json, enabled, created_at, updated_at
                 FROM sub_agents ORDER BY created_at",
            )
            .context("prepare list sub_agents")?;

        let rows = stmt
            .query_map([], row_to_record)
            .context("query sub_agents")?;

        let mut records = Vec::new();
        for row in rows {
            records.push(row?);
        }
        Ok(records)
    }

    pub fn get(&self, id: &str) -> Result<Option<SubAgentRecord>> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT id, name, description, config_json, enabled, created_at, updated_at
             FROM sub_agents WHERE id = ?1",
            params![id],
            row_to_record,
        )
        .optional()
        .context("get sub_agent")
    }

    pub fn create(&self, config: &SubAgentConfig) -> Result<SubAgentRecord> {
        let conn = self.conn()?;
        let config_json = serde_json::to_string(config).context("serialize sub_agent config")?;
        let now = Utc::now().timestamp_millis();

        let created_at = if config.created_at > 0 {
            config.created_at
        } else {
            now
        };
        let updated_at = if config.updated_at > 0 {
            config.updated_at
        } else {
            now
        };

        conn.execute(
            "INSERT INTO sub_agents (id, name, description, config_json, enabled, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                config.agent_id,
                config.agent_name,
                config.description,
                config_json,
                config.enabled as i32,
                created_at,
                updated_at,
            ],
        )
        .context("insert sub_agent")?;

        Ok(SubAgentRecord {
            id: config.agent_id.clone(),
            name: config.agent_name.clone(),
            description: config.description.clone(),
            config_json,
            enabled: config.enabled,
            created_at,
            updated_at,
        })
    }

    pub fn update(&self, id: &str, config: &SubAgentConfig) -> Result<SubAgentRecord> {
        let conn = self.conn()?;
        let config_json = serde_json::to_string(config).context("serialize sub_agent config")?;
        let now = Utc::now().timestamp_millis();

        let affected = conn
            .execute(
                "UPDATE sub_agents SET name = ?1, description = ?2, config_json = ?3,
                 enabled = ?4, updated_at = ?5 WHERE id = ?6",
                params![
                    config.agent_name,
                    config.description,
                    config_json,
                    config.enabled as i32,
                    now,
                    id,
                ],
            )
            .context("update sub_agent")?;

        if affected == 0 {
            anyhow::bail!("子智能体 '{}' 不存在", id);
        }

        self.get(id)?
            .ok_or_else(|| anyhow::anyhow!("子智能体更新后未找到: {}", id))
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction().context("begin delete tx")?;
        tx.execute(
            "DELETE FROM global_sub_agents WHERE sub_agent_id = ?1",
            params![id],
        )
        .context("delete global_sub_agents")?;
        tx.execute("DELETE FROM sub_agents WHERE id = ?1", params![id])
            .context("delete sub_agent")?;
        tx.commit().context("commit delete tx")?;
        Ok(())
    }

    pub fn seed_browser_force(&self) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction().context("begin seed tx")?;
        seed_browser_agent_force_tx(&tx)?;
        tx.commit().context("commit seed tx")?;
        Ok(())
    }

    pub fn set_global_enabled(&self, sub_agent_ids: &[String]) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction().context("begin set_global_enabled tx")?;
        tx.execute("DELETE FROM global_sub_agents", [])
            .context("clear global_sub_agents")?;
        for agent_id in sub_agent_ids {
            tx.execute(
                "INSERT OR IGNORE INTO global_sub_agents (sub_agent_id) VALUES (?1)",
                params![agent_id],
            )
            .context("insert global_sub_agents")?;
        }
        tx.commit().context("commit set_global_enabled tx")?;
        Ok(())
    }

    pub fn get_global_enabled(&self) -> Result<Vec<SubAgentRecord>> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT sa.id, sa.name, sa.description, sa.config_json, sa.enabled, sa.created_at, sa.updated_at
                 FROM sub_agents sa
                 INNER JOIN global_sub_agents gsa ON sa.id = gsa.sub_agent_id
                 WHERE sa.enabled = 1
                 ORDER BY sa.created_at",
            )
            .context("prepare get global sub_agents")?;

        let rows = stmt
            .query_map([], row_to_record)
            .context("query global sub_agents")?;

        let mut records = Vec::new();
        for row in rows {
            records.push(row?);
        }
        Ok(records)
    }

    pub fn save_run_trace(
        &self,
        workspace_id: &str,
        tool_call_id: &str,
        agent_id: &str,
        status: &str,
        events_json: &str,
    ) -> Result<SubAgentRunTraceRecord> {
        if workspace_id.trim().is_empty() || tool_call_id.trim().is_empty() {
            anyhow::bail!("workspace_id 和 tool_call_id 不能为空");
        }
        serde_json::from_str::<serde_json::Value>(events_json)
            .context("validate sub-agent trace events_json")?;
        let conn = self.conn()?;
        let timestamp = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO sub_agent_run_traces (
                workspace_id, tool_call_id, agent_id, status, events_json, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
             ON CONFLICT(workspace_id, tool_call_id) DO UPDATE SET
                agent_id = excluded.agent_id,
                status = excluded.status,
                events_json = excluded.events_json,
                updated_at = excluded.updated_at",
            params![
                workspace_id,
                tool_call_id,
                agent_id,
                status,
                events_json,
                timestamp
            ],
        )
        .context("save sub-agent run trace")?;
        self.get_run_trace(workspace_id, tool_call_id)?
            .context("sub-agent run trace missing after save")
    }

    pub fn get_run_trace(
        &self,
        workspace_id: &str,
        tool_call_id: &str,
    ) -> Result<Option<SubAgentRunTraceRecord>> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT workspace_id, tool_call_id, agent_id, status, events_json, created_at, updated_at
             FROM sub_agent_run_traces
             WHERE workspace_id = ?1 AND tool_call_id = ?2",
            params![workspace_id, tool_call_id],
            |row| {
                Ok(SubAgentRunTraceRecord {
                    workspace_id: row.get(0)?,
                    tool_call_id: row.get(1)?,
                    agent_id: row.get(2)?,
                    status: row.get(3)?,
                    events_json: row.get(4)?,
                    created_at: row.get(5)?,
                    updated_at: row.get(6)?,
                })
            },
        )
        .optional()
        .context("get sub-agent run trace")
    }

    pub fn get_enabled_agent_ids(&self, session_id: &str) -> Result<Vec<String>> {
        let conn = self.conn()?;
        // 启用来源（并集）：
        //   1. global_sub_agents —— 全局级，对所有会话（项目 + 聊天）生效
        //   2. chat 分类级 —— 通过 chat_sessions.category → chat_category_agent_configs
        //                      .sub_agent_ids_json 解析，让不同聊天分类加载不同子智能体。
        let mut stmt = conn
            .prepare(
                "SELECT DISTINCT sa.id FROM sub_agents sa
                 WHERE sa.enabled = 1
                 AND (
                     sa.id IN (SELECT sub_agent_id FROM global_sub_agents)
                     OR EXISTS (
                         SELECT 1
                         FROM chat_sessions s
                         INNER JOIN chat_category_agent_configs cfg ON cfg.category_id = s.category
                         CROSS JOIN json_each(cfg.sub_agent_ids_json) AS je
                         WHERE s.id = ?1 AND je.value = sa.id
                     )
                 )
                 ORDER BY sa.created_at",
            )
            .context("prepare get enabled agent ids")?;

        let rows = stmt
            .query_map(params![session_id], |row| row.get::<_, String>(0))
            .context("query enabled agent ids")?;

        let mut ids = Vec::new();
        for row in rows {
            ids.push(row?);
        }
        Ok(ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::db::DispatcherDb;
    use crate::agent::sub_agent::config::SubAgentModelConfig;

    fn setup() -> (DispatcherDb, SubAgentDb) {
        let path = std::env::temp_dir().join(format!(
            "jkcodingagent-subagent-category-{}.sqlite3",
            uuid::Uuid::new_v4(),
        ));
        let dispatcher = DispatcherDb::new(path).expect("create dispatcher db");
        let sub_agent = SubAgentDb::new(dispatcher.pool());
        (dispatcher, sub_agent)
    }

    fn seed_sub_agent(sub_agent: &SubAgentDb, id: &str) {
        let config = SubAgentConfig {
            agent_id: id.to_string(),
            agent_name: id.to_string(),
            description: "test".to_string(),
            system_prompt: "test".to_string(),
            user_prompt_template: "{{task}}".to_string(),
            allowed_tools: vec!["notify_user_progress".to_string()],
            model_config: SubAgentModelConfig::default(),
            max_iterations: 1,
            max_output_tokens: 256,
            temperature: 0.7,
            timeout_secs: 10,
            enabled: true,
            created_at: 0,
            updated_at: 0,
        };
        sub_agent.create(&config).expect("seed sub_agent");
    }

    fn set_category_sub_agents(dispatcher: &DispatcherDb, category: &str, ids: &[&str]) {
        let conn = dispatcher.conn().expect("dispatcher conn");
        let json = serde_json::to_string(ids).unwrap();
        conn.execute(
            "UPDATE chat_category_agent_configs SET sub_agent_ids_json = ?1 WHERE category_id = ?2",
            params![json, category],
        )
        .expect("update category sub_agent_ids");
    }

    #[test]
    fn loads_sub_agents_per_chat_category() {
        let (dispatcher, sub_agent) = setup();
        // schema 初始化会自动 seed browser-agent 到 global，这里清空 global 以隔离分类级配置。
        sub_agent.set_global_enabled(&[]).expect("clear global");
        seed_sub_agent(&sub_agent, "tech-agent");
        seed_sub_agent(&sub_agent, "life-agent");

        set_category_sub_agents(&dispatcher, "tech", &["tech-agent"]);
        set_category_sub_agents(&dispatcher, "life", &["life-agent"]);

        let tech_session = dispatcher
            .create_chat_session("技术", Some("tech"))
            .expect("create tech session");
        let life_session = dispatcher
            .create_chat_session("生活", Some("life"))
            .expect("create life session");

        let tech_ids = sub_agent
            .get_enabled_agent_ids(&tech_session.id)
            .expect("enabled for tech");
        assert_eq!(tech_ids, vec!["tech-agent".to_string()]);

        let life_ids = sub_agent
            .get_enabled_agent_ids(&life_session.id)
            .expect("enabled for life");
        assert_eq!(life_ids, vec!["life-agent".to_string()]);
    }

    #[test]
    fn empty_category_sub_agents_yields_only_global() {
        let (dispatcher, sub_agent) = setup();
        sub_agent.set_global_enabled(&[]).expect("clear global");
        seed_sub_agent(&sub_agent, "tech-agent");

        set_category_sub_agents(&dispatcher, "tech", &[]);

        let session = dispatcher
            .create_chat_session("技术", Some("tech"))
            .expect("create session");

        let ids = sub_agent
            .get_enabled_agent_ids(&session.id)
            .expect("enabled ids");
        assert!(ids.is_empty(), "no sub-agents for empty category config");
    }

    #[test]
    fn global_sub_agents_apply_to_all_sessions() {
        let (dispatcher, sub_agent) = setup();
        seed_sub_agent(&sub_agent, "global-agent");
        // 显式设置 global（覆盖 schema 自动 seed 的 browser-agent），以隔离断言。
        sub_agent
            .set_global_enabled(&["global-agent".to_string()])
            .expect("set global");

        set_category_sub_agents(&dispatcher, "tech", &[]);

        let session = dispatcher
            .create_chat_session("技术", Some("tech"))
            .expect("create session");

        let ids = sub_agent
            .get_enabled_agent_ids(&session.id)
            .expect("enabled ids");
        assert_eq!(ids, vec!["global-agent".to_string()]);
    }

    #[test]
    fn run_traces_are_isolated_and_removed_with_session_resources() {
        let (dispatcher, sub_agent) = setup();
        let session = dispatcher
            .create_chat_session("轨迹测试", Some("tech"))
            .expect("create trace session");

        sub_agent
            .save_run_trace(
                &session.id,
                "tool-call-1",
                "browser-agent",
                "completed",
                r#"[{"event":"Started","data":{"agentId":"browser-agent"}}]"#,
            )
            .expect("save first trace");
        sub_agent
            .save_run_trace(
                &session.id,
                "tool-call-2",
                "browser-agent",
                "failed",
                r#"[{"event":"Failed","data":{"agentId":"browser-agent"}}]"#,
            )
            .expect("save second trace");

        let first = sub_agent
            .get_run_trace(&session.id, "tool-call-1")
            .expect("get first trace")
            .expect("first trace exists");
        let second = sub_agent
            .get_run_trace(&session.id, "tool-call-2")
            .expect("get second trace")
            .expect("second trace exists");
        assert_eq!(first.status, "completed");
        assert_eq!(second.status, "failed");

        dispatcher
            .clear_messages(&session.id)
            .expect("clear session resources");
        assert!(sub_agent
            .get_run_trace(&session.id, "tool-call-1")
            .expect("get cleared trace")
            .is_none());

        sub_agent
            .save_run_trace(
                &session.id,
                "tool-call-3",
                "browser-agent",
                "completed",
                "[]",
            )
            .expect("save trace before delete");
        dispatcher
            .delete_chat_session(&session.id)
            .expect("delete trace session");
        assert!(sub_agent
            .get_run_trace(&session.id, "tool-call-3")
            .expect("get deleted trace")
            .is_none());
    }

    #[test]
    fn run_trace_rejects_invalid_json() {
        let (dispatcher, sub_agent) = setup();
        let session = dispatcher
            .create_chat_session("无效轨迹", Some("tech"))
            .expect("create session");
        let error = sub_agent
            .save_run_trace(
                &session.id,
                "tool-call-invalid",
                "browser-agent",
                "failed",
                "not-json",
            )
            .expect_err("invalid trace json must fail");
        assert!(error.to_string().contains("validate sub-agent trace"));
    }

    #[test]
    fn schema_v19_migrates_to_sub_agent_trace_table() {
        let path = std::env::temp_dir().join(format!(
            "jkcodingagent-subagent-trace-migration-{}.sqlite3",
            uuid::Uuid::new_v4(),
        ));
        let conn = rusqlite::Connection::open(&path).expect("open v19 db");
        conn.execute_batch("PRAGMA user_version = 19;")
            .expect("mark v19");
        drop(conn);

        let dispatcher = DispatcherDb::new(path).expect("migrate v19 db");
        let conn = dispatcher.conn().expect("migrated conn");
        let version: i32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("read schema version");
        let table_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='sub_agent_run_traces'",
                [],
                |row| row.get(0),
            )
            .expect("check trace table");
        assert_eq!(version, 20);
        assert_eq!(table_count, 1);
    }
}
