//! SSH 全局配置的 SQLite 存取层。
//!
//! 设计原则（配置分层重构 P1）：SSH 服务器是应用生命周期资产，属于全局唯一
//! 权威源（`~/.jkcodingagent/jkbot.sqlite3`），对所有项目 / 聊天上下文可见；
//! 不再按项目路径分键存储。凭据入库后，Agent 文件工具对数据库文件的访问由
//! `agent::tools::builtin::common::is_protected_agent_path` 拒绝。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Utc;
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{params, Connection};

use super::{
    SshAuditLog, SshAuditRecord, SshServerConfig, SshToolsConfig, AUDIT_RECORD_LIMIT,
    CONFIG_FILE_NAME, HOST_KEYS_FILE_NAME, AUDIT_FILE_NAME, SSH_STORE_DIR_NAME,
};

pub type SharedPool = Arc<Pool<SqliteConnectionManager>>;

/// SSH 配置 / 主机密钥 / 审计在全局库中的三张表。
/// 供 schema.rs 基线 DDL 与 v30 迁移共用（CREATE IF NOT EXISTS 幂等）。
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

/// 一次性迁移结果：导入统计 + 提交成功后需要删除的旧凭据文件
/// （明文凭据不宜在多处留存，导入入库后立即清理）。
pub(crate) struct LegacyImportOutcome {
    pub servers: usize,
    pub audit_records: usize,
    pub files_to_delete: Vec<PathBuf>,
}

/// 把旧的按项目分键文件存储迁移进全局库。在 schema v30 迁移事务内调用，
/// `files_to_delete` 由调用方在 commit 成功后清理。
///
/// 扫描来源（均只读）：
/// 1. `root_dir/ssh-tools/<slug>-<hash>/{ssh-tools.json,host-keys.json,audit.json}`
///    —— 旧版全局仓库（本身按项目分键）；
/// 2. `root_dir/projects.json` 中登记项目的 `<repo>/.jkcodingagent/` 下两处更早
///    的遗留位置（`local_env/ssh/` 与根下 `ssh-tools.json`）。
///
/// 合并规则：按 `(host, port, username)` 去重，先到先得；id 冲突时为后来者生成
/// 不冲突后缀；主机密钥跟随来源记录按原 server_id 归并；审计按时间排序截尾。
pub(crate) fn import_legacy_ssh_store(
    conn: &Connection,
    root_dir: &Path,
) -> Result<LegacyImportOutcome, String> {
    let existing: i64 = conn
        .query_row("SELECT COUNT(*) FROM ssh_servers", [], |row| row.get(0))
        .map_err(|error| format!("v30: 检查 ssh_servers 现有数据失败：{error}"))?;
    if existing > 0 {
        // 已导入过（user_version 门控下不应发生，防御性短路）。
        return Ok(LegacyImportOutcome {
            servers: 0,
            audit_records: 0,
            files_to_delete: Vec::new(),
        });
    }

    let mut sources: Vec<LegacySource> = Vec::new();

    let global_store = root_dir.join(SSH_STORE_DIR_NAME);
    if let Ok(entries) = std::fs::read_dir(&global_store) {
        let mut dirs: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect();
        dirs.sort();
        for dir in dirs {
            sources.push(LegacySource {
                config: dir.join(CONFIG_FILE_NAME),
                host_keys: Some(dir.join(HOST_KEYS_FILE_NAME)),
                audit: Some(dir.join(AUDIT_FILE_NAME)),
            });
        }
    }

    for project_dir in registered_project_dirs(root_dir) {
        let legacy_dir = project_dir.join(".jkcodingagent");
        sources.push(LegacySource {
            config: legacy_dir.join(CONFIG_FILE_NAME),
            host_keys: None,
            audit: Some(legacy_dir.join("local_env").join("ssh").join(AUDIT_FILE_NAME)),
        });
        sources.push(LegacySource {
            config: legacy_dir.join("local_env").join("ssh").join(CONFIG_FILE_NAME),
            host_keys: None,
            audit: None,
        });
    }

    struct MergedServer {
        server: SshServerConfig,
        source_index: usize,
        original_id: String,
    }

    let mut merged: Vec<MergedServer> = Vec::new();
    let mut used_ids: HashMap<String, ()> = HashMap::new();
    let mut audit_records: Vec<SshAuditRecord> = Vec::new();
    let mut files_to_delete: Vec<PathBuf> = Vec::new();
    // (来源序号, 原 server_id) → 指纹，迁移服务器时按来源回填。
    let mut host_pins: Vec<HashMap<String, String>> = Vec::new();

    for (source_index, source) in sources.iter().enumerate() {
        host_pins.push(load_host_pins(source.host_keys.as_deref(), &mut files_to_delete));

        let Ok(raw) = std::fs::read_to_string(&source.config) else {
            continue;
        };
        files_to_delete.push(source.config.clone());
        let Ok(config) = serde_json::from_str::<SshToolsConfig>(&raw) else {
            // 解析失败的坏文件不迁移内容，但仍列入清理（凭据残留）。
            eprintln!(
                "[ssh-tool] 迁移：跳过无法解析的旧配置 {}",
                source.config.display()
            );
            continue;
        };
        for mut server in config.servers {
            let dedupe_key = (server.host.clone(), server.port, server.username.clone());
            if merged
                .iter()
                .any(|item| (item.server.host.clone(), item.server.port, item.server.username.clone()) == dedupe_key)
            {
                continue;
            }
            let original_id = server.id.clone();
            if used_ids.contains_key(&server.id) {
                // 同 id 指向不同服务器（跨项目各自维护）：生成不冲突后缀。
                let mut suffix = 2;
                while used_ids.contains_key(&format!("{original_id}-{suffix}")) {
                    suffix += 1;
                }
                server.id = format!("{original_id}-{suffix}");
                server.description = if server.description.trim().is_empty() {
                    format!("迁移自 {original_id}")
                } else {
                    server.description.clone()
                };
            }
            used_ids.insert(server.id.clone(), ());
            merged.push(MergedServer {
                server,
                source_index,
                original_id,
            });
        }
    }

    for source in &sources {
        let Some(audit_path) = source.audit.as_deref() else {
            continue;
        };
        let Ok(raw) = std::fs::read_to_string(audit_path) else {
            continue;
        };
        files_to_delete.push(audit_path.to_path_buf());
        if let Ok(log) = serde_json::from_str::<SshAuditLog>(&raw) {
            audit_records.extend(log.records);
        }
    }

    let now = Utc::now().to_rfc3339();
    for item in &merged {
        let config_json = serde_json::to_string(&item.server)
            .map_err(|error| format!("v30: 序列化 SSH 服务器失败：{error}"))?;
        conn.execute(
            "INSERT INTO ssh_servers (id, config_json, updated_at) VALUES (?1, ?2, ?3)",
            params![item.server.id, config_json, now],
        )
        .map_err(|error| format!("v30: 导入 SSH 服务器失败：{error}"))?;
        if let Some(fingerprint) = host_pins
            .get(item.source_index)
            .and_then(|pins| pins.get(&item.original_id))
        {
            conn.execute(
                "INSERT INTO ssh_host_keys (server_id, fingerprint, updated_at)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(server_id) DO NOTHING",
                params![item.server.id, fingerprint, now],
            )
            .map_err(|error| format!("v30: 导入 SSH 主机密钥失败：{error}"))?;
        }
    }

    audit_records.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    let keep_from = audit_records.len().saturating_sub(AUDIT_RECORD_LIMIT);
    for record in &audit_records[keep_from..] {
        append_audit_record_on(conn, record)?;
    }

    Ok(LegacyImportOutcome {
        servers: merged.len(),
        audit_records: audit_records.len().saturating_sub(keep_from),
        files_to_delete,
    })
}

struct LegacySource {
    config: PathBuf,
    host_keys: Option<PathBuf>,
    audit: Option<PathBuf>,
}

fn load_host_pins(path: Option<&Path>, files_to_delete: &mut Vec<PathBuf>) -> HashMap<String, String> {
    let Some(path) = path else {
        return HashMap::new();
    };
    let Ok(raw) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    files_to_delete.push(path.to_path_buf());
    serde_json::from_str(&raw).unwrap_or_default()
}

/// 直接从 root_dir/projects.json 提取登记项目路径（不依赖 home 定位，
/// 保证迁移在任意 root_dir 下可测试、可重复）。
fn registered_project_dirs(root_dir: &Path) -> Vec<PathBuf> {
    let Ok(raw) = std::fs::read_to_string(root_dir.join("projects.json")) else {
        return Vec::new();
    };
    let Ok(values) = serde_json::from_str::<Vec<serde_json::Value>>(&raw) else {
        return Vec::new();
    };
    values
        .into_iter()
        .filter_map(|value| {
            value
                .get("path")
                .and_then(|path| path.as_str())
                .map(PathBuf::from)
        })
        .collect()
}

/// 提交成功后清理旧凭据文件与空目录。best-effort：失败仅告警。
pub(crate) fn cleanup_legacy_files(outcome: &LegacyImportOutcome, root_dir: &Path) {
    for file in &outcome.files_to_delete {
        if let Err(error) = std::fs::remove_file(file) {
            eprintln!(
                "[ssh-tool] 迁移后清理旧文件失败（{}）：{error}；建议手动删除。",
                file.display()
            );
        }
    }
    // 旧全局仓库的子目录现在应为空目录；清掉仍为空的（含 host-keys 等辅助文件）。
    let global_store = root_dir.join(SSH_STORE_DIR_NAME);
    if let Ok(entries) = std::fs::read_dir(&global_store) {
        for entry in entries.filter_map(|entry| entry.ok()) {
            let path = entry.path();
            if path.is_dir() && std::fs::read_dir(&path).is_ok_and(|mut it| it.next().is_none()) {
                let _ = std::fs::remove_dir(&path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            .save_servers(&[server("alpha", "10.0.0.1", "root"), server("beta", "10.0.0.2", "ops")])
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

    #[test]
    fn legacy_import_merges_and_dedupes_then_cleans_files() {
        let root = temp_root();

        // 旧全局仓库：两个项目键目录，其中一台服务器重复（同 host+port+username）。
        let store_a = root.join(SSH_STORE_DIR_NAME).join("proj-a-11111");
        let store_b = root.join(SSH_STORE_DIR_NAME).join("proj-b-22222");
        std::fs::create_dir_all(&store_a).unwrap();
        std::fs::create_dir_all(&store_b).unwrap();
        std::fs::write(
            store_a.join(CONFIG_FILE_NAME),
            serde_json::to_string(&SshToolsConfig {
                servers: vec![server("dup", "10.0.0.9", "root"), server("only-a", "10.0.0.10", "app")],
            })
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            store_b.join(CONFIG_FILE_NAME),
            serde_json::to_string(&SshToolsConfig {
                servers: vec![
                    server("dup", "10.0.0.9", "root"),
                    // 同 id 不同服务器：应生成 -2 后缀。
                    server("only-a", "10.0.0.11", "other"),
                ],
            })
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            store_a.join(HOST_KEYS_FILE_NAME),
            serde_json::to_string(&HashMap::from([("dup".to_string(), "ff:01".to_string())]))
                .unwrap(),
        )
        .unwrap();
        std::fs::write(
            store_a.join(AUDIT_FILE_NAME),
            serde_json::to_string(&SshAuditLog {
                records: vec![audit_record("2026-01-01T00:00:00Z", "dup")],
            })
            .unwrap(),
        )
        .unwrap();

        // 登记项目内更早的遗留位置。
        let project_dir = root.join("repo-x");
        let legacy_ssh = project_dir.join(".jkcodingagent").join("local_env").join("ssh");
        std::fs::create_dir_all(&legacy_ssh).unwrap();
        std::fs::write(
            legacy_ssh.join(CONFIG_FILE_NAME),
            serde_json::to_string(&SshToolsConfig {
                servers: vec![server("in-repo", "10.0.0.20", "deploy")],
            })
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            root.join("projects.json"),
            serde_json::to_string(&serde_json::json!([
                { "id": "p1", "name": "x", "path": project_dir.to_string_lossy() }
            ]))
            .unwrap(),
        )
        .unwrap();

        // 触发完整迁移（DispatcherDb::new 会执行 v30 导入）。
        let db = crate::agent::db::DispatcherDb::new(root.join("jkbot.sqlite3")).unwrap();
        let ssh_db = SshDb::new(db.pool());
        let config = ssh_db.load_config().unwrap();
        let mut ids: Vec<&str> = config.servers.iter().map(|s| s.id.as_str()).collect();
        ids.sort();
        // dup 只保留一份；only-a 与 only-a-2（同 id 不同服务器）共存；in-repo 来自项目遗留位置。
        assert_eq!(ids, vec!["dup", "in-repo", "only-a", "only-a-2"]);

        // 主机密钥跟随来源归并到迁移后的服务器 id。
        assert_eq!(ssh_db.host_key_pin("dup").unwrap(), Some("ff:01".into()));

        // 审计已入库。
        assert_eq!(ssh_db.list_audit().unwrap().records.len(), 1);

        // 旧凭据文件已被清理。
        assert!(!store_a.join(CONFIG_FILE_NAME).exists());
        assert!(!store_b.join(CONFIG_FILE_NAME).exists());
        assert!(!legacy_ssh.join(CONFIG_FILE_NAME).exists());

        std::fs::remove_dir_all(&root).ok();
    }
}
