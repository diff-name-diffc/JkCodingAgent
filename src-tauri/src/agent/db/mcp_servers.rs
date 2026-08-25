//! 全局 MCP 服务器注册表：mcp_servers 表的读写。
//!
//! MCP 支持两级配置：全局注册表（本表，所有项目与聊天共享）+ 项目级
//! `<repo>/.jkcodingagent/mcp.json`（随仓库走）。同名时项目级覆盖全局。

use anyhow::{Context, Result};
use rusqlite::{params, Transaction};

use super::DispatcherDb;
use crate::mcp::{McpConfig, McpServerConfig};

const MCP_SERVERS_DDL: &str = "CREATE TABLE IF NOT EXISTS mcp_servers (
    name TEXT PRIMARY KEY,
    enabled INTEGER NOT NULL DEFAULT 1,
    config_json TEXT NOT NULL,
    sort_order INTEGER NOT NULL DEFAULT 0
)";

/// 建表（幂等）。schema.rs 基线建库时调用，DDL 单一出处。
pub(crate) fn ensure_mcp_servers_table_tx(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(MCP_SERVERS_DDL)
        .context("create mcp_servers table")
}

impl DispatcherDb {
    /// 全局 MCP 配置（与项目级 mcp.json 同构，enabled 落到每个 server 上）。
    pub fn get_global_mcp_config(&self) -> Result<McpConfig> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare("SELECT name, enabled, config_json FROM mcp_servers ORDER BY sort_order, name")
            .context("prepare list mcp servers")?;
        let mut servers = std::collections::BTreeMap::new();
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("load mcp servers")?;
        for (name, enabled, raw) in rows {
            let mut server: McpServerConfig = serde_json::from_str(&raw)
                .with_context(|| format!("parse mcp server {name} config"))?;
            server.enabled = Some(enabled != 0);
            servers.insert(name, server);
        }
        Ok(McpConfig { servers })
    }

    /// 整列表同步保存（先清空再按顺序插入），与前端「重写整个列表」语义一致。
    pub fn save_global_mcp_config(&self, config: &McpConfig) -> Result<()> {
        // 序列化在事务外先行完成：任何条目无法序列化都整体失败，
        // 绝不把 `{}` 静默写进注册表（大声失败）。
        let mut rows = Vec::with_capacity(config.servers.len());
        for (sort_order, (name, server)) in config.servers.iter().enumerate() {
            let config_json = serde_json::to_string(server)
                .with_context(|| format!("serialize mcp server {name}"))?;
            rows.push((
                name.clone(),
                server.enabled.unwrap_or(true),
                config_json,
                sort_order,
            ));
        }
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM mcp_servers", [])
            .context("clear mcp_servers")?;
        for (name, enabled, config_json, sort_order) in rows {
            tx.execute(
                "INSERT INTO mcp_servers (name, enabled, config_json, sort_order)
                 VALUES (?1, ?2, ?3, ?4)",
                params![name, enabled as i64, config_json, sort_order as i64],
            )
            .with_context(|| format!("insert mcp server {name}"))?;
        }
        tx.commit().context("commit save mcp servers")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> (DispatcherDb, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "aha-mcp-db-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        (DispatcherDb::new(root.join("jkbot.sqlite3")).unwrap(), root)
    }

    #[test]
    fn save_global_config_roundtrips_and_replaces() {
        let (db, _root) = test_db();
        let mut config = McpConfig::default();
        config.servers.insert(
            "a".to_string(),
            McpServerConfig {
                command: Some("node".to_string()),
                args: vec!["a.js".to_string()],
                ..Default::default()
            },
        );
        db.save_global_mcp_config(&config).unwrap();

        let mut next = McpConfig::default();
        next.servers.insert(
            "b".to_string(),
            McpServerConfig {
                url: Some("http://x/mcp".to_string()),
                enabled: Some(false),
                ..Default::default()
            },
        );
        db.save_global_mcp_config(&next).unwrap();

        let loaded = db.get_global_mcp_config().unwrap();
        assert_eq!(loaded.servers.len(), 1);
        assert!(loaded.servers.contains_key("b"));
        assert_eq!(loaded.servers["b"].enabled, Some(false));
    }

    #[test]
    fn corrupt_config_json_fails_loudly_on_read() {
        let (db, _root) = test_db();
        db.conn()
            .unwrap()
            .execute(
                "INSERT INTO mcp_servers (name, enabled, config_json, sort_order)
                 VALUES ('broken', 1, '{not json', 0)",
                [],
            )
            .unwrap();

        let error = db.get_global_mcp_config().unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("broken"), "错误应指明服务器名：{message}");
    }
}
