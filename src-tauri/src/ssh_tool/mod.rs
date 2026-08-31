pub mod db;

mod audit;
mod command_exec;
pub(crate) mod commands;
pub(crate) mod config_import;
mod connection;
mod types;
mod validation;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use chrono::Utc;
use parking_lot::Mutex;
use russh::client::Handle;

use audit::normalize_project_key;
pub use audit::render_ssh_audit_record_markdown;
#[cfg(test)]
use audit::{truncate_for_audit, AUDIT_OUTPUT_CHARS};
use command_exec::run_command_on_connection;
pub use command_exec::MAX_STDIN_CHARS;
use connection::{connect, SshClientHandler};
pub use db::{SharedPool, SshDb};
pub use types::{
    SshAuditLog, SshAuditRecord, SshAuditReview, SshAuthMethod, SshExecResult, SshServerConfig,
    SshServerSummary, SshToolsConfig,
};
use validation::{
    validate_command, validate_config, validate_session_id, MAX_OUTPUT_BYTES, MAX_TIMEOUT_SECS,
};

const AUDIT_RECORD_LIMIT: usize = 100;

/// 连接池中空闲连接的保留时长；正在执行命令的连接不受影响。
const CONNECTION_IDLE_SECS: u64 = 30 * 60;

/// SSH 连接池与全局配置仓库。
///
/// 配置（服务器凭据 / 主机密钥 / 审计）权威存储在全局 SQLite 库（`SshDb`），
/// 对所有项目与聊天上下文共享；本结构只额外持有跨命令复用的连接池。
///
/// 并发模型（russh 多路复用）：
/// - `sessions` 只保护 HashMap 本身：查找 / 插入瞬间完成，绝不持它做网络 I/O；
/// - 建立连接（TCP + 握手 + 认证 + 主机密钥校验，可达数十秒）在 `sessions` 锁外进行；
/// - 每条连接是一个 russh `Handle`（Clone、Send、内部已序列化）：同一连接上的并发
///   命令各自打开独立 channel，输出按 channel 隔离、天然不交错，无需逐命令互斥锁；
/// - 空闲回收只读 `last_used_ms` 原子量，不会为了 reap 去碰正在执行命令的连接。
#[derive(Clone)]
pub struct SshSessionManager {
    db: SshDb,
    sessions: Arc<Mutex<HashMap<SshSessionKey, Arc<SshConnection>>>>,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct SshSessionKey {
    server_id: String,
    session_id: String,
}

struct SshConnection {
    /// russh 会话句柄：可克隆、线程安全，命令通过它打开独立 channel 执行。
    handle: Handle<SshClientHandler>,
    /// 最近一次命令结束的 Unix 毫秒时间戳；原子量供空闲回收无锁读取。
    last_used_ms: AtomicU64,
}

impl SshConnection {
    fn new(handle: Handle<SshClientHandler>) -> Self {
        Self {
            handle,
            last_used_ms: AtomicU64::new(unix_now_ms()),
        }
    }

    fn touch(&self) {
        self.last_used_ms.store(unix_now_ms(), Ordering::Relaxed);
    }

    fn idle_secs(&self) -> u64 {
        unix_now_ms().saturating_sub(self.last_used_ms.load(Ordering::Relaxed)) / 1000
    }
}

fn unix_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

impl SshSessionManager {
    pub fn new(pool: SharedPool) -> Self {
        Self {
            db: SshDb::new(pool),
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn load_config_async(&self) -> Result<SshToolsConfig, String> {
        let ssh_db = self.db.clone();
        tokio::task::spawn_blocking(move || ssh_db.load_config())
            .await
            .map_err(|error| error.to_string())?
    }

    pub async fn load_audit_async(&self) -> Result<SshAuditLog, String> {
        let ssh_db = self.db.clone();
        tokio::task::spawn_blocking(move || ssh_db.list_audit())
            .await
            .map_err(|error| error.to_string())?
    }

    /// 保存全局服务器列表：校验 → 全量替换入库 → 丢弃全部复用连接
    /// （凭据可能已变更，旧连接不可继续使用）。
    pub async fn save_config_async(
        &self,
        config: SshToolsConfig,
    ) -> Result<SshToolsConfig, String> {
        let ssh_db = self.db.clone();
        let cleaned = tokio::task::spawn_blocking(move || {
            let cleaned = validate_config(config)?;
            ssh_db.save_servers(&cleaned.servers)?;
            Ok::<_, String>(cleaned)
        })
        .await
        .map_err(|error| error.to_string())??;
        self.drop_all();
        Ok(cleaned)
    }

    /// 列出已启用的 SSH server（不含凭据等敏感字段）。
    pub async fn list_servers_async(&self) -> Result<Vec<SshServerSummary>, String> {
        let ssh_db = self.db.clone();
        tokio::task::spawn_blocking(move || {
            let config = ssh_db.load_config()?;
            Ok(config
                .servers
                .into_iter()
                .filter(|server| server.enabled)
                .map(|server| SshServerSummary {
                    id: server.id,
                    name: server.name,
                    description: server.description,
                    tags: server.tags,
                })
                .collect())
        })
        .await
        .map_err(|error| error.to_string())?
    }

    /// 读取指定已启用服务器的完整配置（含 review_enabled 等字段），供审查门禁使用。
    pub async fn server_config_async(&self, server_id: String) -> Result<SshServerConfig, String> {
        let ssh_db = self.db.clone();
        tokio::task::spawn_blocking(move || ssh_db.find_enabled_server(&server_id))
            .await
            .map_err(|error| error.to_string())?
    }

    /// `workspace_path` 仅作为审计元数据记录（配置已全局化，不再决定存储位置）。
    #[allow(clippy::too_many_arguments)]
    pub async fn execute(
        &self,
        workspace_path: PathBuf,
        workspace_id: String,
        session_title: String,
        server_id: String,
        session_id: String,
        command: String,
        stdin: Option<String>,
        timeout_secs: Option<u64>,
        review: Option<SshAuditReview>,
    ) -> Result<SshExecResult, String> {
        let mut result = self
            .execute_command(
                server_id.clone(),
                session_id.clone(),
                command.clone(),
                stdin,
                timeout_secs,
            )
            .await;
        let audit_record = SshAuditRecord::from_execution(
            &workspace_path,
            workspace_id,
            session_title,
            server_id,
            session_id,
            command,
            &result,
            review.as_ref(),
        );
        let ssh_db = self.db.clone();
        let audit_write =
            tokio::task::spawn_blocking(move || ssh_db.append_audit_record(&audit_record))
                .await
                .map_err(|error| error.to_string())?;
        if let Err(error) = audit_write {
            // 命令已经执行过：审计失败不能伪装成命令失败。保留真实结果，
            // 把审计写入失败作为警告附在 stderr 里返回。
            eprintln!("[ssh-tool] 写入 SSH 审计记录失败：{error}");
            if let Ok(output) = &mut result {
                output
                    .stderr
                    .push_str(&format!("\n[警告：审计记录写入失败：{error}]"));
            }
        }
        result
    }

    /// 记录一条「被安全审查拦截、未执行」的审计记录。
    #[allow(clippy::too_many_arguments)]
    pub async fn record_review_blocked(
        &self,
        workspace_path: PathBuf,
        workspace_id: String,
        session_title: String,
        server_id: String,
        session_id: String,
        command: String,
        review: SshAuditReview,
    ) -> Result<SshAuditRecord, String> {
        let ssh_db = self.db.clone();
        tokio::task::spawn_blocking(move || {
            let record = SshAuditRecord {
                created_at: Utc::now().to_rfc3339(),
                workspace_path: normalize_project_key(&workspace_path),
                workspace_id,
                session_title,
                server_id,
                session_id,
                command,
                exit_code: None,
                stdout: String::new(),
                stderr: String::new(),
                duration_ms: None,
                truncated: false,
                interactive_blocked: false,
                error: None,
                review: Some(review),
            };
            ssh_db.append_audit_record(&record)?;
            Ok(record)
        })
        .await
        .map_err(|error| error.to_string())?
    }

    async fn execute_command(
        &self,
        server_id: String,
        session_id: String,
        command: String,
        stdin: Option<String>,
        timeout_secs: Option<u64>,
    ) -> Result<SshExecResult, String> {
        let ssh_db = self.db.clone();
        let lookup_id = server_id.clone();
        let server = tokio::task::spawn_blocking(move || ssh_db.find_enabled_server(&lookup_id))
            .await
            .map_err(|error| error.to_string())??;
        let timeout_secs = timeout_secs.unwrap_or(server.default_timeout_secs);
        let timeout_secs = timeout_secs.clamp(1, MAX_TIMEOUT_SECS);
        let max_output_bytes = server.max_output_bytes.clamp(1, MAX_OUTPUT_BYTES);
        validate_session_id(&session_id)?;
        validate_command(&command)?;
        if let Some(input) = stdin.as_deref() {
            let chars = input.chars().count();
            if chars > MAX_STDIN_CHARS {
                return Err(format!(
                    "错误：stdin 长度 {chars} 字符超过上限 {MAX_STDIN_CHARS}（与安全审查送审上限一致，不允许执行未完整送审的内容；请改用分批写入）"
                ));
            }
        }

        let key = SshSessionKey {
            server_id: server.id.clone(),
            session_id: session_id.clone(),
        };
        let started = Instant::now();

        let connection = self.connection_for(key.clone(), &server).await?;
        match run_command_on_connection(
            &connection,
            &server_id,
            &session_id,
            &command,
            stdin.as_deref(),
            timeout_secs,
            max_output_bytes,
            started,
        )
        .await
        {
            Ok(result) => Ok(result),
            Err(failure) if failure.stale => {
                // 缓存连接已断：丢弃入池记录，重连后重试一次。
                self.drop_session(&key);
                let connection = self.connection_for(key, &server).await?;
                run_command_on_connection(
                    &connection,
                    &server_id,
                    &session_id,
                    &command,
                    stdin.as_deref(),
                    timeout_secs,
                    max_output_bytes,
                    started,
                )
                .await
                .map_err(|failure| failure.message)
            }
            Err(failure) => Err(failure.message),
        }
    }

    /// 取指定 key 的连接；不存在或已断开则在锁外新建后入池。
    async fn connection_for(
        &self,
        key: SshSessionKey,
        server: &SshServerConfig,
    ) -> Result<Arc<SshConnection>, String> {
        self.reap_idle_connections();
        if let Some(cached) = self.sessions.lock().get(&key).cloned() {
            if !cached.handle.is_closed() {
                return Ok(cached);
            }
            // 缓存连接已被对端/网络断开：移除后重建（drop 即释放会话任务）。
            self.sessions.lock().remove(&key);
        }
        // 在 sessions 锁外建立连接（TCP + 握手 + 认证 + 主机密钥校验），
        // 否则一台服务器连不上会拖住所有服务器的 SSH 操作。
        let connection = Arc::new(SshConnection::new(connect(server, &self.db).await?));
        let mut sessions = self.sessions.lock();
        if let Some(existing) = sessions.get(&key).cloned() {
            // 并发下同 key 先连者获胜：复用已入池连接，丢弃本次新建（drop 即断开）。
            return Ok(existing);
        }
        sessions.insert(key, connection.clone());
        Ok(connection)
    }

    /// 回收空闲连接。只读原子时间戳，不碰任何连接的 session 锁，
    /// 因此正在执行命令（最长 300s）的连接不会阻塞回收或其他连接的建立。
    fn reap_idle_connections(&self) {
        self.sessions
            .lock()
            .retain(|_, connection| connection.idle_secs() < CONNECTION_IDLE_SECS);
    }

    /// 配置保存后丢弃全部复用连接（凭据可能已变更）。
    fn drop_all(&self) {
        self.sessions.lock().clear();
    }

    fn drop_session(&self, key: &SshSessionKey) {
        self.sessions.lock().remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_truncation_keeps_head_and_tail() {
        let short = "abc".repeat(100);
        assert_eq!(truncate_for_audit(&short), short);

        let long = "x".repeat(AUDIT_OUTPUT_CHARS + 5000);
        let truncated = truncate_for_audit(&long);
        assert!(truncated.contains("审计输出已截断"));
        assert!(truncated.chars().count() < long.chars().count());
        // 头尾各半保留
        assert!(truncated.starts_with(&"x".repeat(AUDIT_OUTPUT_CHARS / 2)));
        assert!(truncated.ends_with(&"x".repeat(AUDIT_OUTPUT_CHARS / 2)));
    }
}
