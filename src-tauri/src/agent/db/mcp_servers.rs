//! 全局 MCP 服务器注册表：mcp_servers 表的读写与旧文件迁移。
//!
//! MCP 支持两级配置：全局注册表（本表，所有项目与聊天共享）+ 项目级
//! `<repo>/.jkcodingagent/mcp.json`（随仓库走）。同名时项目级覆盖全局。
//! 历史上普通聊天的「全局」MCP 存放在 plain-chat-browser 工作区的
//! mcp.json，v32 迁移将其导入本表。

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{params, Transaction};

use super::DispatcherDb;
use crate::project::mcp::{ProjectMcpConfig, ProjectMcpServerConfig};

const MCP_SERVERS_DDL: &str = "CREATE TABLE IF NOT EXISTS mcp_servers (
    name TEXT PRIMARY KEY,
    enabled INTEGER NOT NULL DEFAULT 1,
    config_json TEXT NOT NULL,
    sort_order INTEGER NOT NULL DEFAULT 0
)";

/// 建表（幂等）。基线 DDL 与 v32 迁移共用，保持同文。
pub(crate) fn ensure_mcp_servers_table_tx(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(MCP_SERVERS_DDL).context("create mcp_servers table")
}

/// v32 迁移：把普通聊天工作区的 mcp.json（plain-chat-browser/.jkcodingagent/
/// mcp.json）一次性导入全局表。表非空短路（幂等）；文件不存在/解析失败按
/// 空配置处理。返回 (导入条数, 是否命中旧文件)。
pub(crate) fn import_legacy_global_mcp_json(
    tx: &Transaction<'_>,
    root_dir: &Path,
) -> Result<(usize, bool)> {
    let existing: i64 = tx
        .query_row("SELECT COUNT(*) FROM mcp_servers", [], |row| row.get(0))
        .context("count mcp_servers")?;
    if existing > 0 {
        return Ok((0, false));
    }
    let legacy_path = root_dir
        .join("plain-chat-browser")
        .join(".jkcodingagent")
        .join("mcp.json");
    let Ok(raw) = std::fs::read_to_string(&legacy_path) else {
        return Ok((0, false));
    };
    let config: ProjectMcpConfig = serde_json::from_str(&raw)
        .with_context(|| format!("parse {}", legacy_path.display()))?;
    let mut imported = 0usize;
    for (sort_order, (name, server)) in config.servers.iter().enumerate() {
        tx.execute(
            "INSERT OR IGNORE INTO mcp_servers (name, enabled, config_json, sort_order)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                name,
                server.enabled.unwrap_or(true) as i64,
                serde_json::to_string(server).unwrap_or_else(|_| "{}".to_string()),
                sort_order as i64
            ],
        )
        .with_context(|| format!("import mcp server {name}"))?;
        imported += 1;
    }
    Ok((imported, true))
}

/// 提交成功后删除旧文件（best-effort）。
pub(crate) fn cleanup_legacy_global_mcp_file(root_dir: &Path) {
    let legacy_path = root_dir
        .join("plain-chat-browser")
        .join(".jkcodingagent")
        .join("mcp.json");
    if legacy_path.exists() {
        if let Err(error) = std::fs::remove_file(&legacy_path) {
            eprintln!(
                "[mcp] 迁移后清理旧全局配置失败（{}）：{error}；建议手动删除。",
                legacy_path.display()
            );
        }
    }
}

impl DispatcherDb {
    /// 全局 MCP 配置（与项目级 mcp.json 同构，enabled 落到每个 server 上）。
    pub fn get_global_mcp_config(&self) -> Result<ProjectMcpConfig> {
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
            let mut server: ProjectMcpServerConfig = serde_json::from_str(&raw)
                .with_context(|| format!("parse mcp server {name} config"))?;
            server.enabled = Some(enabled != 0);
            servers.insert(name, server);
        }
        Ok(ProjectMcpConfig { servers })
    }

    /// 整列表同步保存（先清空再按顺序插入），与前端「重写整个列表」语义一致。
    pub fn save_global_mcp_config(&self, config: &ProjectMcpConfig) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM mcp_servers", [])
            .context("clear mcp_servers")?;
        for (sort_order, (name, server)) in config.servers.iter().enumerate() {
            tx.execute(
                "INSERT INTO mcp_servers (name, enabled, config_json, sort_order)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    name,
                    server.enabled.unwrap_or(true) as i64,
                    serde_json::to_string(server).unwrap_or_else(|_| "{}".to_string()),
                    sort_order as i64
                ],
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
    fn migrates_plain_chat_mcp_json_on_first_init() {
        let (db, root) = test_db();
        assert_eq!(db.get_global_mcp_config().unwrap().servers.len(), 0);

        drop(db);
        std::fs::remove_file(root.join("jkbot.sqlite3")).unwrap();
        let legacy_dir = root.join("plain-chat-browser").join(".jkcodingagent");
        std::fs::create_dir_all(&legacy_dir).unwrap();
        std::fs::write(
            legacy_dir.join("mcp.json"),
            r#"{"mcpServers": {
                "fetch": {"enabled": true, "transport": "streamable_http", "url": "http://localhost:3331/mcp"},
                "notes": {"command": "npx", "args": ["-y", "notes-mcp"], "enabled": false}
            }}"#,
        )
        .unwrap();
        let db = DispatcherDb::new(root.join("jkbot.sqlite3")).unwrap();
        let config = db.get_global_mcp_config().unwrap();
        assert_eq!(config.servers.len(), 2);
        assert_eq!(config.servers["fetch"].url.as_deref(), Some("http://localhost:3331/mcp"));
        assert_eq!(config.servers["fetch"].enabled, Some(true));
        assert_eq!(config.servers["notes"].enabled, Some(false));
        // 旧文件已清理。
        assert!(!legacy_dir.join("mcp.json").exists());
    }

    #[test]
    fn save_global_config_roundtrips_and_replaces() {
        let (db, _root) = test_db();
        let mut config = ProjectMcpConfig::default();
        config.servers.insert(
            "a".to_string(),
            ProjectMcpServerConfig {
                command: Some("node".to_string()),
                args: vec!["a.js".to_string()],
                ..Default::default()
            },
        );
        db.save_global_mcp_config(&config).unwrap();

        let mut next = ProjectMcpConfig::default();
        next.servers.insert(
            "b".to_string(),
            ProjectMcpServerConfig {
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
}
