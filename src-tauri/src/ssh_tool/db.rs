//! SSH 全局配置的 SQLite 存取层。
//!
//! 设计原则（配置分层重构 P1）：SSH 服务器是应用生命周期资产，属于全局唯一
//! 权威源（`~/.jkcodingagent/jkbot.sqlite3`），对所有项目 / 聊天上下文可见；
//! 不再按项目路径分键存储。凭据入库后，Agent 文件工具对数据库文件的访问由
//! `agent::tools::builtin::common::is_protected_agent_path` 拒绝。

use std::sync::Arc;

use chrono::Utc;
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{params, Connection};

use super::{SshAuditLog, SshAuditRecord, SshServerConfig, SshToolsConfig, AUDIT_RECORD_LIMIT};

pub type SharedPool = Arc<Pool<SqliteConnectionManager>>;

/// SSH 配置 / 主机密钥 / 审计在全局库中的三张表。
/// 供 schema.rs 基线建库时调用（CREATE IF NOT EXISTS 幂等）。
pub(crate) fn ensure_ssh_tables_tx(tx: &rusqlite::Transaction<'_>) -> rusqlite::Result<()> {
    tx.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS ssh_servers (
            id TEXT PRIMARY KEY,
            config_json TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS ssh_host_keys (
            server_id TEXT PRIMARY KEY,
            fingerprint TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS ssh_audit_log (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            created_at TEXT NOT NULL,
            server_id TEXT NOT NULL,
            record_json TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_ssh_audit_log_server
        ON ssh_audit_log(server_id, id DESC);
        ",
    )
}

/// 基于共享连接池的 SSH 配置读写。所有方法阻塞且短小，调用方负责放入
/// `spawn_blocking`（与既有命令层约定一致）。
#[derive(Clone)]
pub struct SshDb {
    pool: SharedPool,
}

impl SshDb {
    pub fn new(pool: SharedPool) -> Self {
        Self { pool }
    }

    fn conn(&self) -> Result<r2d2::PooledConnection<SqliteConnectionManager>, String> {
        self.pool
            .get()
            .map_err(|error| format!("获取 SSH 配置数据库连接失败：{error}"))
    }

    pub fn load_config(&self) -> Result<SshToolsConfig, String> {
        let conn = self.conn()?;
        load_config_on(&conn)
    }

    /// 全量替换服务器列表（设置页保存语义：以提交的列表为准）。
    pub fn save_servers(&self, servers: &[SshServerConfig]) -> Result<(), String> {
        let mut conn = self.conn()?;
        let tx = conn
            .transaction()
            .map_err(|error| format!("保存 SSH 服务器配置失败：{error}"))?;
        tx.execute("DELETE FROM ssh_servers", [])
            .map_err(|error| format!("保存 SSH 服务器配置失败：{error}"))?;
        let now = Utc::now().to_rfc3339();
        for server in servers {
            let config_json = serde_json::to_string(server)
                .map_err(|error| format!("序列化 SSH 服务器配置失败：{error}"))?;
            tx.execute(
                "INSERT INTO ssh_servers (id, config_json, updated_at) VALUES (?1, ?2, ?3)",
                params![server.id, config_json, now],
            )
            .map_err(|error| format!("保存 SSH 服务器配置失败：{error}"))?;
        }
        tx.commit()
            .map_err(|error| format!("保存 SSH 服务器配置失败：{error}"))
    }

    /// 读取指定已启用服务器的完整配置（含 review_enabled 等字段）。
    pub fn find_enabled_server(&self, server_id: &str) -> Result<SshServerConfig, String> {
        let conn = self.conn()?;
        load_config_on(&conn)?
            .servers
            .into_iter()
            .find(|server| server.enabled && server.id == server_id)
            .ok_or_else(|| format!("未找到已启用的 SSH server：{server_id}"))
    }

    pub fn host_key_pin(&self, server_id: &str) -> Result<Option<String>, String> {
        let conn = self.conn()?;
        let pin = conn
            .query_row(
                "SELECT fingerprint FROM ssh_host_keys WHERE server_id = ?1",
                params![server_id],
                |row| row.get::<_, String>(0),
            )
            .map(Some)
            .or_else(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })
            .map_err(|error| format!("读取 SSH 主机密钥固定记录失败：{error}"))?;
        Ok(pin)
    }

    pub fn set_host_key_pin(&self, server_id: &str, fingerprint: &str) -> Result<(), String> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO ssh_host_keys (server_id, fingerprint, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(server_id) DO UPDATE SET fingerprint = ?2, updated_at = ?3",
            params![server_id, fingerprint, Utc::now().to_rfc3339()],
        )
        .map(|_| ())
        .map_err(|error| format!("写入 SSH 主机密钥固定记录失败：{error}"))
    }

    pub fn remove_host_key_pin(&self, server_id: &str) -> Result<(), String> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM ssh_host_keys WHERE server_id = ?1",
            params![server_id],
        )
        .map(|_| ())
        .map_err(|error| format!("删除 SSH 主机密钥固定记录失败：{error}"))
    }

    pub fn append_audit_record(&self, record: &SshAuditRecord) -> Result<(), String> {
        let conn = self.conn()?;
        append_audit_record_on(&conn, record)
    }

    pub fn list_audit(&self) -> Result<SshAuditLog, String> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT record_json FROM ssh_audit_log
                 ORDER BY id DESC LIMIT ?1",
            )
            .map_err(|error| format!("读取 SSH 审计记录失败：{error}"))?;
        let mut records = stmt
            .query_map(params![AUDIT_RECORD_LIMIT as i64], |row| {
                row.get::<_, String>(0)
            })
            .map_err(|error| format!("读取 SSH 审计记录失败：{error}"))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|error| format!("读取 SSH 审计记录失败：{error}"))?
            .into_iter()
            .map(|raw| serde_json::from_str::<SshAuditRecord>(&raw))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("解析 SSH 审计记录失败：{error}"))?;
        records.reverse();
        Ok(SshAuditLog { records })
    }
}

fn load_config_on(conn: &Connection) -> Result<SshToolsConfig, String> {
    let mut stmt = conn
        .prepare("SELECT config_json FROM ssh_servers ORDER BY id")
        .map_err(|error| format!("读取 SSH 服务器配置失败：{error}"))?;
    let servers = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| format!("读取 SSH 服务器配置失败：{error}"))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| format!("读取 SSH 服务器配置失败：{error}"))?
        .into_iter()
        .map(|raw| serde_json::from_str::<SshServerConfig>(&raw))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("解析 SSH 服务器配置失败：{error}"))?;
    Ok(SshToolsConfig { servers })
}

fn append_audit_record_on(conn: &Connection, record: &SshAuditRecord) -> Result<(), String> {
    let record_json = serde_json::to_string(record)
        .map_err(|error| format!("序列化 SSH 审计记录失败：{error}"))?;
    conn.execute(
        "INSERT INTO ssh_audit_log (created_at, server_id, record_json) VALUES (?1, ?2, ?3)",
        params![record.created_at, record.server_id, record_json],
    )
    .map_err(|error| format!("写入 SSH 审计记录失败：{error}"))?;
    // 修剪历史，保留最近 AUDIT_RECORD_LIMIT 条。
    conn.execute(
        "DELETE FROM ssh_audit_log
         WHERE id NOT IN (SELECT id FROM ssh_audit_log ORDER BY id DESC LIMIT ?1)",
        params![AUDIT_RECORD_LIMIT as i64],
    )
    .map(|_| ())
    .map_err(|error| format!("修剪 SSH 审计记录失败：{error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_root() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "aha-ssh-db-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn server(id: &str, host: &str, username: &str) -> SshServerConfig {
        SshServerConfig {
            id: id.to_string(),
            name: String::new(),
            enabled: true,
            host: host.to_string(),
            port: 22,
            username: username.to_string(),
            password: "secret".to_string(),
            auth_method: Default::default(),
            private_key_path: String::new(),
            private_key_passphrase: String::new(),
            description: String::new(),
            tags: Vec::new(),
            review_enabled: true,
            default_timeout_secs: 30,
            max_output_bytes: 64 * 1024,
        }
    }

    fn audit_record(created_at: &str, server_id: &str) -> SshAuditRecord {
        SshAuditRecord {
            created_at: created_at.to_string(),
            workspace_path: "/tmp/ws".to_string(),
            workspace_id: "ws".to_string(),
            session_title: "t".to_string(),
            server_id: server_id.to_string(),
            session_id: "s".to_string(),
            command: "ls".to_string(),
            exit_code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
            duration_ms: Some(1),
            truncated: false,
            interactive_blocked: false,
            error: None,
            review: None,
        }
    }

    #[test]
    fn db_roundtrip_servers_host_keys_and_audit() {
        let root = temp_root();
        let db = crate::agent::db::DispatcherDb::new(root.join("jkbot.sqlite3")).unwrap();
        let ssh_db = SshDb::new(db.pool());

        ssh_db
            .save_servers(&[
                server("alpha", "10.0.0.1", "root"),
                server("beta", "10.0.0.2", "ops"),
            ])
            .unwrap();
        let config = ssh_db.load_config().unwrap();
        assert_eq!(config.servers.len(), 2);
        assert_eq!(config.servers[0].id, "alpha");

        assert!(ssh_db.find_enabled_server("beta").is_ok());
        assert!(ssh_db.find_enabled_server("missing").is_err());

        // TOFU：学习 → 命中 → 重置
        assert_eq!(ssh_db.host_key_pin("alpha").unwrap(), None);
        ssh_db.set_host_key_pin("alpha", "aa:bb").unwrap();
        assert_eq!(ssh_db.host_key_pin("alpha").unwrap(), Some("aa:bb".into()));
        ssh_db.remove_host_key_pin("alpha").unwrap();
        assert_eq!(ssh_db.host_key_pin("alpha").unwrap(), None);

        for index in 0..(AUDIT_RECORD_LIMIT + 10) {
            let mut record = audit_record("2026-01-01T00:00:00Z", "alpha");
            record.command = format!("cmd-{index}");
            ssh_db.append_audit_record(&record).unwrap();
        }
        let log = ssh_db.list_audit().unwrap();
        assert_eq!(log.records.len(), AUDIT_RECORD_LIMIT);
        assert_eq!(log.records.last().unwrap().command, "cmd-109");

        std::fs::remove_dir_all(&root).ok();
    }
}
