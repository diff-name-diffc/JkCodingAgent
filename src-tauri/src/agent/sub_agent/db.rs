mod traces;

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
    /// 是否在全局启用集合（`global_sub_agents` 成员）。保真 `enabled=1 ∩
    /// 全局成员` 的交集语义由消费方按两字段过滤（原
    /// `sub_agent_get_global_enabled` 命令折叠进 `sub_agent_list`）。
    #[serde(default)]
    pub global_enabled: bool,
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
    /// 运行实际使用的模型（v5 列，UI-14 遗留）：老轨迹为 NULL，
    /// 前端显示「未记录」兜底。
    pub model: Option<String>,
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
            model TEXT,
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
    // 与 seed_browser_agent_force_tx 同源取 agent_id，避免硬编码字面量
    // 在默认配置改名后出现重复种子或误判；查询错误向上传播，
    // 不再用 unwrap_or(0) 把表缺失等底层问题当成「不存在」继续种子。
    let config = SubAgentConfig::browser_agent_default();
    let existing = tx
        .query_row(
            "SELECT COUNT(*) FROM sub_agents WHERE id = ?1",
            params![config.agent_id],
            |row| row.get::<_, i64>(0),
        )
        .context("check browser-agent existence")?;

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
        global_enabled: row.get::<_, i32>(7)? != 0,
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
                "SELECT sa.id, sa.name, sa.description, sa.config_json, sa.enabled,
                        sa.created_at, sa.updated_at,
                        gsa.sub_agent_id IS NOT NULL
                 FROM sub_agents sa
                 LEFT JOIN global_sub_agents gsa ON sa.id = gsa.sub_agent_id
                 ORDER BY sa.created_at",
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
            "SELECT sa.id, sa.name, sa.description, sa.config_json, sa.enabled,
                    sa.created_at, sa.updated_at,
                    gsa.sub_agent_id IS NOT NULL
             FROM sub_agents sa
             LEFT JOIN global_sub_agents gsa ON sa.id = gsa.sub_agent_id
             WHERE sa.id = ?1",
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
            // 新建时尚未加入全局启用集合；是否全局启用由用户在设置里勾选。
            global_enabled: false,
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

    pub fn get_enabled_agent_ids(&self, session_id: &str) -> Result<Vec<String>> {
        let conn = self.conn()?;
        // 启用来源（并集）：
        //   1. global_sub_agents —— 全局级，对所有会话（项目 + 聊天）生效
        //   2. chat 分类级 —— 通过 dispatcher_sessions.category
        //                      → chat_category_agent_configs.sub_agent_ids_json
        //                      解析，让不同聊天分类加载不同子智能体。
        let mut stmt = conn
            .prepare(
                "SELECT DISTINCT sa.id FROM sub_agents sa
                 WHERE sa.enabled = 1
                 AND (
                     sa.id IN (SELECT sub_agent_id FROM global_sub_agents)
                     OR EXISTS (
                         SELECT 1
                         FROM dispatcher_sessions s
                         INNER JOIN chat_category_agent_configs cfg
                             ON cfg.category_id = s.category
                            AND json_valid(cfg.sub_agent_ids_json)
                         CROSS JOIN json_each(cfg.sub_agent_ids_json) AS je
                         WHERE s.id = ?1 AND s.kind = 'chat' AND je.value = sa.id
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
mod tests;
