pub mod db;

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chrono::Utc;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use ssh2::{Channel, Session};
use tauri::State;

pub use db::{SshDb, SharedPool};

const CONFIG_FILE_NAME: &str = "ssh-tools.json";
const AUDIT_FILE_NAME: &str = "audit.json";
const HOST_KEYS_FILE_NAME: &str = "host-keys.json";
const AUDIT_RECORD_LIMIT: usize = 100;
/// 单条审计记录里 stdout / stderr 各自保留的最大字符数（头尾各半）。
/// 完整输出仍随工具结果返回；这里只限制审计文件的落盘体积。
const AUDIT_OUTPUT_CHARS: usize = 8_000;
const DEFAULT_TIMEOUT_SECS: u64 = 30;
const DEFAULT_MAX_OUTPUT_BYTES: usize = 64 * 1024;
const MAX_TIMEOUT_SECS: u64 = 300;
const MAX_OUTPUT_BYTES: usize = 1024 * 1024;
const ID_MAX_LEN: usize = 64;
// 交互阻塞检测分两档：
// - 有证据（输出末尾命中密码 / 确认 / 分页器等提示符）→ 最快 PROMPT_IDLE_SECS 秒中止；
// - 纯静默（sleep、慢查询、写文件的备份等）→ 阈值随 timeout_secs 放大（timeout / 4），
//   避免误杀安静的长命令，上限 MAX_SILENT_IDLE_SECS。
const PROMPT_IDLE_SECS: u64 = 8;
const MAX_SILENT_IDLE_SECS: u64 = 60;
const IDLE_POLL_MS: u64 = 150;
const READ_CHUNK: usize = 8192;
/// stdin 的执行上限，同时也是安全审查的送审上限（见 agent::ssh_review）：
/// 凡执行的内容必须完整送审，不允许「执行一大段、只审开头」的盲区。
pub const MAX_STDIN_CHARS: usize = 32_000;
const TIMEOUT_EXIT_CODE: i32 = 124;
const INTERACTIVE_EXIT_CODE: i32 = -1;
/// 命令正常结束但协议未带回退出码（极少见）；与交互阻塞的 -1 区分开。
const UNKNOWN_EXIT_CODE: i32 = -2;
/// 旧版 SSH 凭据的按项目分键目录名（`~/.jkcodingagent/ssh-tools/<project-key>/`），
/// 现仅作为一次性迁移（schema v30）的扫描来源；权威存储已迁入 SQLite。
const SSH_STORE_DIR_NAME: &str = "ssh-tools";
/// 连接池中空闲连接的保留时长；正在执行命令的连接不受影响。
const CONNECTION_IDLE_SECS: u64 = 30 * 60;
const LIBSSH2_ERROR_SOCKET_SEND: i32 = -7;
const LIBSSH2_ERROR_SOCKET_DISCONNECT: i32 = -13;
const LIBSSH2_ERROR_CHANNEL_CLOSED: i32 = -26;
const LIBSSH2_ERROR_SOCKET_TIMEOUT: i32 = -30;
const LIBSSH2_ERROR_SOCKET_RECV: i32 = -43;
const LIBSSH2_ERROR_BAD_SOCKET: i32 = -45;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SshToolsConfig {
    #[serde(default)]
    pub servers: Vec<SshServerConfig>,
}

/// SSH 认证方式：密码或私钥文件。
/// 默认 Password 以兼容旧配置（缺少该字段时回退为密码认证）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum SshAuthMethod {
    #[default]
    Password,
    Key,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshServerConfig {
    pub id: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    pub host: String,
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub auth_method: SshAuthMethod,
    /// 私钥文件路径，支持 `~` 展开；仅 auth_method == Key 时生效。
    #[serde(default)]
    pub private_key_path: String,
    /// 私钥口令（加密私钥时需要，明文存储于配置文件，与 password 同等敏感）。
    #[serde(default)]
    pub private_key_passphrase: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
    /// 执行命令前是否经过安全审查 AI 评估（默认开启）。
    #[serde(default = "default_review_enabled")]
    pub review_enabled: bool,
    #[serde(default = "default_timeout_secs")]
    pub default_timeout_secs: u64,
    #[serde(default = "default_max_output_bytes")]
    pub max_output_bytes: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SshServerSummary {
    pub id: String,
    pub description: String,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SshExecResult {
    pub server_id: String,
    pub session_id: String,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u128,
    pub truncated: bool,
    pub interactive_blocked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SshAuditLog {
    #[serde(default)]
    pub records: Vec<SshAuditRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshAuditReview {
    /// 审查是否通过
    pub allowed: bool,
    /// 审查原因（拒绝时为拦截理由，通过时通常为空）
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshAuditRecord {
    pub created_at: String,
    pub workspace_path: String,
    pub workspace_id: String,
    pub session_title: String,
    pub server_id: String,
    pub session_id: String,
    pub command: String,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: Option<u128>,
    pub truncated: bool,
    #[serde(default)]
    pub interactive_blocked: bool,
    pub error: Option<String>,
    /// 命令执行前的安全审查结论；None 表示未审查（未配置审查 AI 或服务器关闭审查）。
    #[serde(default)]
    pub review: Option<SshAuditReview>,
}

/// SSH 连接池与全局配置仓库。
///
/// 配置（服务器凭据 / 主机密钥 / 审计）权威存储在全局 SQLite 库（`SshDb`），
/// 对所有项目与聊天上下文共享；本结构只额外持有跨命令复用的连接池。
///
/// 锁纪律（防止一条慢命令冻结全部 SSH 操作）：
/// - `sessions` 只保护 HashMap 本身：查找 / 插入瞬间完成，绝不持它做网络 I/O；
/// - 建立连接（TCP + 握手 + 认证 + 主机密钥校验，可达数十秒）在 `sessions` 锁外进行；
/// - 每条连接的 `session` 锁在整条命令期间独占（保证输出不交错、状态不串扰）；
///   空闲回收只读 `last_used_ms` 原子量，不会为了 reap 去碰正在执行命令的连接。
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
    /// 整条命令执行期间独占：channel 操作与阻塞模式切换都依赖它。
    session: Mutex<Session>,
    /// 最近一次命令结束的 Unix 毫秒时间戳；原子量供空闲回收无锁读取。
    last_used_ms: AtomicU64,
}

impl SshConnection {
    fn new(session: Session) -> Self {
        Self {
            session: Mutex::new(session),
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

enum SshCommandStartError {
    Channel(ssh2::Error),
    Exec(ssh2::Error),
}

#[tauri::command]
pub async fn ssh_tool_load_config(manager: State<'_, SshSessionManager>) -> Result<SshToolsConfig, String> {
    manager.load_config_async().await
}

#[tauri::command]
pub async fn ssh_tool_load_audit(manager: State<'_, SshSessionManager>) -> Result<SshAuditLog, String> {
    manager.load_audit_async().await
}

#[tauri::command]
pub async fn ssh_tool_save_config(
    manager: State<'_, SshSessionManager>,
    config: SshToolsConfig,
) -> Result<SshToolsConfig, String> {
    manager.save_config_async(config).await
}

#[tauri::command]
pub async fn ssh_tool_test_server_config(
    manager: State<'_, SshSessionManager>,
    server: SshServerConfig,
    reset_host_key: Option<bool>,
) -> Result<String, String> {
    let ssh_db = manager.db.clone();
    tokio::task::spawn_blocking(move || {
        let config = validate_single_server(server)?;
        if reset_host_key.unwrap_or(false) {
            ssh_db.remove_host_key_pin(&config.id)?;
        }
        let session = connect(&config, &ssh_db)?;
        session
            .disconnect(None, "connection test completed", None)
            .ok();
        Ok(format!("连接成功：{}", config.id))
    })
    .await
    .map_err(|error| error.to_string())?
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
    pub async fn save_config_async(&self, config: SshToolsConfig) -> Result<SshToolsConfig, String> {
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
        let manager = self.clone();
        tokio::task::spawn_blocking(move || {
            manager.execute_blocking(
                workspace_path,
                workspace_id,
                session_title,
                server_id,
                session_id,
                command,
                stdin,
                timeout_secs,
                review,
            )
        })
        .await
        .map_err(|error| error.to_string())?
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

    #[allow(clippy::too_many_arguments)]
    fn execute_blocking(
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
        let result = self.execute_command_blocking(
            server_id.clone(),
            session_id.clone(),
            command.clone(),
            stdin,
            timeout_secs,
        );
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
        if let Err(error) = self.db.append_audit_record(&audit_record) {
            // 命令已经执行过：审计失败不能伪装成命令失败。保留真实结果，
            // 把审计写入失败作为警告附在 stderr 里返回。
            eprintln!("[ssh-tool] 写入 SSH 审计记录失败：{error}");
            if let Ok(mut output) = result {
                output
                    .stderr
                    .push_str(&format!("\n[警告：审计记录写入失败：{error}]"));
                return Ok(output);
            }
        }
        result
    }

    fn execute_command_blocking(
        &self,
        server_id: String,
        session_id: String,
        command: String,
        stdin: Option<String>,
        timeout_secs: Option<u64>,
    ) -> Result<SshExecResult, String> {
        let server = self.db.find_enabled_server(&server_id)?;
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

        let connection = self.connection_for(key.clone(), &server)?;
        match run_command_on_connection(
            &connection,
            &server_id,
            &session_id,
            &command,
            stdin.as_deref(),
            timeout_secs,
            max_output_bytes,
            started,
        ) {
            Ok(result) => Ok(result),
            Err(error) if error.is_stale_connection() => {
                self.drop_session(&key);
                let connection = self.connection_for(key, &server)?;
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
                .map_err(|error| error.render())
            }
            Err(error) => Err(error.render()),
        }
    }

    /// 取指定 key 的连接；不存在则在锁外新建后入池。
    fn connection_for(
        &self,
        key: SshSessionKey,
        server: &SshServerConfig,
    ) -> Result<Arc<SshConnection>, String> {
        self.reap_idle_connections();
        if let Some(connection) = self.sessions.lock().get(&key).cloned() {
            // 缓存连接直接复用；连接若已死亡，由执行侧的 stale 错误码检测触发丢弃重连。
            return Ok(connection);
        }
        // 在 sessions 锁外建立连接（TCP + 握手 + 认证 + 主机密钥校验），
        // 否则一台服务器连不上会拖住所有服务器的 SSH 操作。
        let ssh_db = self.db.clone();
        let connection = Arc::new(SshConnection::new(connect(server, &ssh_db)?));
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

fn default_enabled() -> bool {
    true
}

fn default_review_enabled() -> bool {
    true
}

fn default_ssh_port() -> u16 {
    22
}

fn default_timeout_secs() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

fn default_max_output_bytes() -> usize {
    DEFAULT_MAX_OUTPUT_BYTES
}

impl SshAuditRecord {
    #[allow(clippy::too_many_arguments)]
    fn from_execution(
        workspace_path: &Path,
        workspace_id: String,
        session_title: String,
        server_id: String,
        session_id: String,
        command: String,
        result: &Result<SshExecResult, String>,
        review: Option<&SshAuditReview>,
    ) -> Self {
        let review = review.cloned();
        match result {
            Ok(output) => Self {
                created_at: Utc::now().to_rfc3339(),
                workspace_path: normalize_project_key(workspace_path),
                workspace_id,
                session_title,
                server_id,
                session_id,
                command,
                exit_code: Some(output.exit_code),
                stdout: truncate_for_audit(&output.stdout),
                stderr: truncate_for_audit(&output.stderr),
                duration_ms: Some(output.duration_ms),
                truncated: output.truncated,
                interactive_blocked: output.interactive_blocked,
                error: None,
                review,
            },
            Err(error) => Self {
                created_at: Utc::now().to_rfc3339(),
                workspace_path: normalize_project_key(workspace_path),
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
                error: Some(sanitize_error_text(error)),
                review,
            },
        }
    }
}

/// 审计落盘只保留头尾各半的输出，防止单条大输出把审计文件撑大；
/// 完整输出仍随工具结果返回给调用方。
fn truncate_for_audit(text: &str) -> String {
    let total = text.chars().count();
    if total <= AUDIT_OUTPUT_CHARS {
        return text.to_string();
    }
    let half = AUDIT_OUTPUT_CHARS / 2;
    let head: String = text.chars().take(half).collect();
    let tail: String = text.chars().skip(total - half).collect();
    format!(
        "{head}\n…[审计输出已截断，省略 {} 字符]…\n{tail}",
        total - half * 2
    )
}

impl SshCommandStartError {
    fn is_stale_connection(&self) -> bool {
        match self {
            Self::Channel(error) | Self::Exec(error) => is_stale_ssh_error(error),
        }
    }

    fn render(self) -> String {
        match self {
            Self::Channel(error) => sanitize_ssh_error("创建 SSH channel 失败", error),
            Self::Exec(error) => sanitize_ssh_error("执行远程命令失败", error),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_command_on_connection(
    connection: &Arc<SshConnection>,
    server_id: &str,
    session_id: &str,
    command: &str,
    stdin: Option<&str>,
    timeout_secs: u64,
    max_output_bytes: usize,
    started: Instant,
) -> Result<SshExecResult, SshCommandStartError> {
    let (session, mut channel) = open_exec_channel(connection, command)?;

    // 在阻塞模式下写入 stdin（若有）并关闭输入端。写入失败（如命令很快退出、
    // 不再读输入）不判为命令失败，但要把原因带到 stderr，避免无声丢失。
    let mut stdin_write_note = None;
    if let Some(input) = stdin {
        if !input.is_empty() {
            if let Err(error) = channel.write_all(input.as_bytes()) {
                stdin_write_note = Some(format!(
                    "\n[写入 stdin 失败（命令可能未读取标准输入）：{}]",
                    sanitize_error_text(&error.to_string())
                ));
            } else {
                let _ = channel.flush();
            }
        }
    }
    let _ = channel.send_eof();

    // 切换非阻塞读取：libssh2 的 socket 超时会污染整个会话（缓存的连接需丢弃），
    // 改用非阻塞 + 空转计时来检测交互阻塞，会话保持可复用。
    // 当前命令独占 session 互斥锁，中途切换阻塞模式是安全的。
    session.set_blocking(false);
    let (prompt_idle_secs, silent_idle_secs) = idle_thresholds(timeout_secs);
    let deadline = started + Duration::from_secs(timeout_secs);
    let (outcome, stdout_raw, stderr_raw, stdout_capped, stderr_capped) = drain_channel(
        &mut channel,
        max_output_bytes,
        prompt_idle_secs,
        silent_idle_secs,
        deadline,
    );
    session.set_blocking(true);

    let stdout = finalize_output(&stdout_raw, stdout_capped);
    let (exit_code, mut stderr) = match outcome {
        DrainOutcome::Completed => {
            let _ = channel.wait_close();
            let code = channel.exit_status().unwrap_or(UNKNOWN_EXIT_CODE);
            (code, finalize_output(&stderr_raw, stderr_capped))
        }
        DrainOutcome::TimedOut => {
            let mut text = finalize_output(&stderr_raw, stderr_capped);
            text.push_str(&format!(
                "\n[命令超过 {timeout_secs}s 仍未结束，已中止。若是长任务请提高 timeout_secs；若是交互阻塞请改用非交互形式。]"
            ));
            (TIMEOUT_EXIT_CODE, text)
        }
        DrainOutcome::InteractiveBlocked => {
            let mut text = finalize_output(&stderr_raw, stderr_capped);
            let hint =
                interactive_prompt_hint(&stdout_raw, &stderr_raw).unwrap_or("未匹配到明显提示符");
            text.push_str(&format!(
                "\n[工具检测到命令疑似在等待交互输入（连续 {silent_idle_secs}s 无输出且未退出；若命中密码/确认等提示符最快 {prompt_idle_secs}s 即中止。疑似：{hint}），已主动中止以免长时间挂起。最近输出结尾：{:?}。请改用非交互形式后重试：sudo→免密账号或 NOPASSWD；确认提示→加 -y/--yes；分页器→PAGER=cat、GIT_PAGER=cat；REPL→用 -e/-c 或通过 stdin 参数喂入。]",
                tail_snippet(&stdout_raw, &stderr_raw)
            ));
            (INTERACTIVE_EXIT_CODE, text)
        }
    };
    if let Some(note) = stdin_write_note {
        stderr.push_str(&note);
    }
    connection.touch();

    Ok(SshExecResult {
        server_id: server_id.to_string(),
        session_id: session_id.to_string(),
        exit_code,
        stdout,
        stderr,
        duration_ms: started.elapsed().as_millis(),
        truncated: stdout_capped || stderr_capped,
        interactive_blocked: matches!(outcome, DrainOutcome::InteractiveBlocked),
    })
}

/// 静默阈值随命令超时放大：短命令保持灵敏（8s），长命令（如 300s 备份）放宽到
/// 最多 60s，避免误杀安静的长任务；命中提示符证据时始终用快速阈值。
fn idle_thresholds(timeout_secs: u64) -> (u64, u64) {
    let silent_idle_secs = (timeout_secs / 4).clamp(PROMPT_IDLE_SECS, MAX_SILENT_IDLE_SECS);
    let prompt_idle_secs = PROMPT_IDLE_SECS.min(silent_idle_secs);
    (prompt_idle_secs, silent_idle_secs)
}

fn open_exec_channel<'a>(
    connection: &'a Arc<SshConnection>,
    command: &str,
) -> Result<(parking_lot::MutexGuard<'a, Session>, Channel), SshCommandStartError> {
    let session = connection.session.lock();
    let mut channel = session
        .channel_session()
        .map_err(SshCommandStartError::Channel)?;
    channel.exec(command).map_err(SshCommandStartError::Exec)?;
    Ok((session, channel))
}

fn is_stale_ssh_error(error: &ssh2::Error) -> bool {
    matches!(
        error.code(),
        ssh2::ErrorCode::Session(LIBSSH2_ERROR_SOCKET_SEND)
            | ssh2::ErrorCode::Session(LIBSSH2_ERROR_SOCKET_DISCONNECT)
            | ssh2::ErrorCode::Session(LIBSSH2_ERROR_CHANNEL_CLOSED)
            | ssh2::ErrorCode::Session(LIBSSH2_ERROR_SOCKET_TIMEOUT)
            | ssh2::ErrorCode::Session(LIBSSH2_ERROR_SOCKET_RECV)
            | ssh2::ErrorCode::Session(LIBSSH2_ERROR_BAD_SOCKET)
    )
}

pub fn render_ssh_audit_record_markdown(record: &SshAuditRecord) -> String {
    let mut output = String::new();
    output.push_str("## SSH 命令审查记录\n\n");
    output.push_str(&format!("- 时间: `{}`\n", record.created_at));
    output.push_str(&format!("- 服务器: `{}`\n", record.server_id));
    output.push_str(&format!("- 会话: `{}`\n", record.session_id));
    if let Some(review) = record.review.as_ref() {
        output.push_str(&format!(
            "- 审查结论: `{}`\n",
            if review.allowed { "通过" } else { "拦截" }
        ));
        output.push_str(&format!(
            "- 审查原因: {}\n",
            if review.reason.trim().is_empty() {
                if review.allowed {
                    "审查通过，允许执行。"
                } else {
                    "审查拒绝，命令未执行。"
                }
            } else {
                review.reason.trim()
            }
        ));
    } else {
        output.push_str("- 审查结论: `未审查`\n");
    }
    output.push_str(&format!(
        "- 执行状态: `{}`\n",
        audit_execution_status(record)
    ));
    output.push_str("\n### 命令\n\n```sh\n");
    output.push_str(&record.command);
    output.push_str("\n```\n");
    if !record.stdout.trim().is_empty() {
        output.push_str("\n### stdout\n\n```text\n");
        output.push_str(&record.stdout);
        output.push_str("\n```\n");
    }
    if !record.stderr.trim().is_empty() {
        output.push_str("\n### stderr\n\n```text\n");
        output.push_str(&record.stderr);
        output.push_str("\n```\n");
    }
    if let Some(error) = record
        .error
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        output.push_str("\n### 错误\n\n");
        output.push_str(error);
        output.push('\n');
    }
    output
}

fn audit_execution_status(record: &SshAuditRecord) -> String {
    if record.review.as_ref().is_some_and(|review| !review.allowed) {
        return "审查拦截，未执行".to_string();
    }
    if record.interactive_blocked {
        return "交互阻塞，已中止".to_string();
    }
    if record.error.is_some() {
        return "执行失败".to_string();
    }
    match record.exit_code {
        Some(code) => format!(
            "exit={code}, duration={}ms",
            record.duration_ms.unwrap_or(0)
        ),
        None => "未执行".to_string(),
    }
}

fn validate_config(mut config: SshToolsConfig) -> Result<SshToolsConfig, String> {
    let mut ids = std::collections::HashSet::new();
    for server in &mut config.servers {
        normalize_server(server);
        validate_single_server_ref(server)?;
        if !ids.insert(server.id.clone()) {
            return Err(format!("SSH server id 重复：{}", server.id));
        }
    }
    Ok(config)
}

fn validate_single_server(mut server: SshServerConfig) -> Result<SshServerConfig, String> {
    normalize_server(&mut server);
    validate_single_server_ref(&server)?;
    Ok(server)
}

fn normalize_server(server: &mut SshServerConfig) {
    server.id = server.id.trim().to_string();
    server.host = server.host.trim().to_string();
    server.username = server.username.trim().to_string();
    server.password = server.password.trim().to_string();
    server.private_key_path = server.private_key_path.trim().to_string();
    server.private_key_passphrase = server.private_key_passphrase.trim().to_string();
    server.description = server.description.trim().to_string();
    server.tags.retain(|tag| !tag.trim().is_empty());
    server.tags = server
        .tags
        .iter()
        .map(|tag| tag.trim().to_string())
        .collect();
    server.default_timeout_secs = server.default_timeout_secs.clamp(1, MAX_TIMEOUT_SECS);
    server.max_output_bytes = server.max_output_bytes.clamp(1, MAX_OUTPUT_BYTES);
}

fn validate_single_server_ref(server: &SshServerConfig) -> Result<(), String> {
    validate_server_id(&server.id)?;
    if server.host.is_empty() {
        return Err(format!("SSH server {} 缺少 host", server.id));
    }
    if server.username.is_empty() {
        return Err(format!("SSH server {} 缺少 username", server.id));
    }
    match server.auth_method {
        SshAuthMethod::Password => {
            if server.password.is_empty() {
                return Err(format!("SSH server {} 缺少 password", server.id));
            }
        }
        SshAuthMethod::Key => {
            if server.private_key_path.is_empty() {
                return Err(format!(
                    "SSH server {} 缺少 private_key_path（密钥文件路径）",
                    server.id
                ));
            }
            let resolved = expand_key_path(&server.private_key_path);
            if !resolved.is_file() {
                return Err(format!(
                    "SSH server {} 的密钥文件不存在或不是普通文件：{}",
                    server.id, server.private_key_path
                ));
            }
        }
    }
    Ok(())
}

fn validate_server_id(id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("SSH server id 不能为空".to_string());
    }
    if id.len() > ID_MAX_LEN {
        return Err(format!("SSH server id 不能超过 {ID_MAX_LEN} 个字符"));
    }
    if !id
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_')
    {
        return Err(format!(
            "SSH server id 仅支持小写字母、数字、短横线和下划线：{id}"
        ));
    }
    Ok(())
}

fn validate_session_id(id: &str) -> Result<(), String> {
    if id.trim().is_empty() {
        return Err("错误：session_id 不能为空".to_string());
    }
    if id.len() > ID_MAX_LEN {
        return Err(format!("错误：session_id 不能超过 {ID_MAX_LEN} 个字符"));
    }
    if !id
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.')
    {
        return Err("错误：session_id 仅支持字母、数字、短横线、下划线和点号".to_string());
    }
    Ok(())
}

fn validate_command(command: &str) -> Result<(), String> {
    if command.trim().is_empty() {
        return Err("错误：command 不能为空".to_string());
    }
    if command.len() > 8192 {
        return Err("错误：command 长度不能超过 8192 字符".to_string());
    }
    Ok(())
}

fn connect(server: &SshServerConfig, ssh_db: &SshDb) -> Result<Session, String> {
    let address = format!("{}:{}", server.host, server.port);
    let tcp = TcpStream::connect(&address).map_err(|error| {
        format!(
            "连接 SSH server {} 失败：{}",
            server.id,
            sanitize_error_text(&error.to_string())
        )
    })?;
    tcp.set_read_timeout(Some(Duration::from_secs(server.default_timeout_secs)))
        .map_err(|error| error.to_string())?;
    tcp.set_write_timeout(Some(Duration::from_secs(server.default_timeout_secs)))
        .map_err(|error| error.to_string())?;

    let mut session =
        Session::new().map_err(|error| sanitize_ssh_error("创建 SSH 会话失败", error))?;
    session.set_tcp_stream(tcp);
    session.set_timeout(server.default_timeout_secs.saturating_mul(1000) as u32);
    session
        .handshake()
        .map_err(|error| sanitize_ssh_error("SSH 握手失败", error))?;
    // 主机密钥校验必须在认证之前：否则密码/私钥会被交给伪造的服务端。
    verify_or_learn_host_key(ssh_db, server, &session)?;
    match server.auth_method {
        SshAuthMethod::Password => {
            session
                .userauth_password(&server.username, &server.password)
                .map_err(|error| sanitize_ssh_error("SSH 密码认证失败", error))?;
        }
        SshAuthMethod::Key => {
            let key_path = expand_key_path(&server.private_key_path);
            let passphrase = if server.private_key_passphrase.is_empty() {
                None
            } else {
                Some(server.private_key_passphrase.as_str())
            };
            session
                .userauth_pubkey_file(&server.username, None, &key_path, passphrase)
                .map_err(|error| sanitize_ssh_error("SSH 密钥认证失败", error))?;
        }
    }
    if !session.authenticated() {
        return Err(format!("SSH server {} 认证失败", server.id));
    }
    Ok(session)
}

/// 主机公钥指纹固定（TOFU：首次连接学习，之后指纹变更即拒绝）。
/// 存储在全局库的 ssh_host_keys 表，按 server_id 区分。
/// 服务器重建等合法变更时，可在应用的 SSH 设置中重试连接测试并选择更新指纹。
fn verify_or_learn_host_key(
    ssh_db: &SshDb,
    server: &SshServerConfig,
    session: &Session,
) -> Result<(), String> {
    let Some(raw) = session.host_key_hash(ssh2::HashType::Sha256) else {
        return Err(format!(
            "SSH server {} 未提供主机公钥，已拒绝连接",
            server.id
        ));
    };
    let fingerprint: String = raw.iter().map(|byte| format!("{byte:02x}")).collect();
    match ssh_db.host_key_pin(&server.id)? {
        Some(pinned) if pinned == fingerprint => Ok(()),
        Some(pinned) => Err(format!(
            "SSH server {} 的主机密钥与已固定指纹不一致（疑似中间人攻击，或服务器已重建）：\n固定指纹：{pinned}\n实际指纹：{fingerprint}\n如确认是合法变更，请在应用的 SSH 设置中重新测试连接并更新指纹。",
            server.id
        )),
        None => ssh_db.set_host_key_pin(&server.id, &fingerprint),
    }
}

/// drain 通道的最终结局：正常结束 / 疑似交互阻塞 / 超时
#[derive(Clone, Copy)]
enum DrainOutcome {
    Completed,
    InteractiveBlocked,
    TimedOut,
}

/// 在非阻塞模式下读取 channel 的 stdout 与 stderr。
/// 判定规则（按优先级）：
/// - 两端都 EOF → 正常结束；到达 `deadline` → 超时；
/// - 连续 `silent_idle_secs` 无输出且通道未关闭 → 疑似交互阻塞（给安静的长命令
///   留出随 timeout 放大的静默预算）；
/// - 连续 `prompt_idle_secs` 无输出且输出末尾命中密码/确认等提示符 → 立即中止
///   （有交互证据时不必等静默预算耗尽）。
#[allow(clippy::too_many_arguments)]
fn drain_channel(
    channel: &mut Channel,
    max_output_bytes: usize,
    prompt_idle_secs: u64,
    silent_idle_secs: u64,
    deadline: Instant,
) -> (DrainOutcome, Vec<u8>, Vec<u8>, bool, bool) {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut stdout_eof = false;
    let mut stderr_eof = false;
    let mut stdout_capped = false;
    let mut stderr_capped = false;
    let mut last_data_at = Instant::now();
    let mut chunk = [0u8; READ_CHUNK];
    let prompt_idle = Duration::from_secs(prompt_idle_secs);
    let silent_idle = Duration::from_secs(silent_idle_secs);
    let poll = Duration::from_millis(IDLE_POLL_MS);

    let outcome = loop {
        if stdout_eof && stderr_eof {
            break DrainOutcome::Completed;
        }
        let now = Instant::now();
        if now >= deadline {
            break DrainOutcome::TimedOut;
        }
        let mut got_data = false;

        if !stdout_eof {
            match channel.read(&mut chunk) {
                Ok(0) => stdout_eof = true,
                Ok(n) => {
                    append_limited(
                        &mut stdout,
                        &chunk[..n],
                        max_output_bytes,
                        &mut stdout_capped,
                    );
                    last_data_at = now;
                    got_data = true;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => {
                    stderr.extend_from_slice(
                        format!(
                            "\n[读取远程 stdout 失败：{}]",
                            sanitize_error_text(&error.to_string())
                        )
                        .as_bytes(),
                    );
                    stdout_eof = true;
                }
            }
        }
        if !stderr_eof {
            let mut stream = channel.stderr();
            match stream.read(&mut chunk) {
                Ok(0) => stderr_eof = true,
                Ok(n) => {
                    append_limited(
                        &mut stderr,
                        &chunk[..n],
                        max_output_bytes,
                        &mut stderr_capped,
                    );
                    last_data_at = now;
                    got_data = true;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => {
                    stderr.extend_from_slice(
                        format!(
                            "\n[读取远程 stderr 失败：{}]",
                            sanitize_error_text(&error.to_string())
                        )
                        .as_bytes(),
                    );
                    stderr_eof = true;
                }
            }
        }

        if got_data {
            continue;
        }
        let idle_for = now.duration_since(last_data_at);
        if idle_for >= silent_idle {
            break DrainOutcome::InteractiveBlocked;
        }
        if idle_for >= prompt_idle && interactive_prompt_hint(&stdout, &stderr).is_some() {
            break DrainOutcome::InteractiveBlocked;
        }
        std::thread::sleep(poll);
    };

    (outcome, stdout, stderr, stdout_capped, stderr_capped)
}

fn append_limited(out: &mut Vec<u8>, data: &[u8], max_bytes: usize, capped: &mut bool) {
    let remaining = max_bytes.saturating_sub(out.len());
    if remaining == 0 {
        if !data.is_empty() {
            *capped = true;
        }
        return;
    }
    let take = data.len().min(remaining);
    out.extend_from_slice(&data[..take]);
    if take < data.len() {
        *capped = true;
    }
}

fn finalize_output(buf: &[u8], capped: bool) -> String {
    let mut text = String::from_utf8_lossy(buf).into_owned();
    if capped {
        text.push_str("\n[输出已截断]");
    }
    text
}

/// 扫描输出末尾是否命中常见交互提示符，命中则返回中文标签。
fn interactive_prompt_hint(stdout: &[u8], stderr: &[u8]) -> Option<&'static str> {
    let stdout_s = String::from_utf8_lossy(stdout);
    let stderr_s = String::from_utf8_lossy(stderr);
    let combined: String = stdout_s.chars().chain(stderr_s.chars()).collect();
    let from = combined
        .char_indices()
        .rev()
        .nth(512)
        .map(|(index, _)| index)
        .unwrap_or(0);
    let lower = combined[from..].to_ascii_lowercase();

    let patterns: &[(&str, &str)] = &[
        ("passphrase", "口令提示"),
        ("password", "密码提示"),
        ("密码", "密码提示"),
        ("[y/n]", "y/n 确认"),
        ("(y/n)", "y/n 确认"),
        ("[yes/no]", "yes/no 确认"),
        ("(yes/no)", "yes/no 确认"),
        ("y/n", "y/n 确认"),
        ("are you sure", "确认提示"),
        ("do you want to continue", "确认提示"),
        ("continue?", "确认提示"),
        ("是否", "确认提示"),
        ("确认", "确认提示"),
        ("press any key", "按键继续"),
        ("press enter", "回车继续"),
        ("verification code", "验证码"),
        ("one-time", "验证码"),
        ("otp", "验证码"),
    ];
    patterns
        .iter()
        .find_map(|(needle, label)| lower.contains(needle).then_some(*label))
}

/// 取 stdout+stderr 末尾约 200 字符，用于诊断信息展示。
fn tail_snippet(stdout: &[u8], stderr: &[u8]) -> String {
    let mut combined = String::from_utf8_lossy(stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(stderr));
    let from = combined
        .char_indices()
        .rev()
        .nth(200)
        .map(|(index, _)| index)
        .unwrap_or(0);
    combined[from..].trim_end().to_string()
}

fn normalize_project_key(project_path: &Path) -> String {
    project_path
        .canonicalize()
        .unwrap_or_else(|_| project_path.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

/// 展开私钥路径开头的 `~/` 或 `~` 为用户主目录；已是绝对/相对路径时原样返回。
fn expand_key_path(raw: &str) -> PathBuf {
    if let Some(rest) = raw.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    } else if raw == "~" {
        if let Some(home) = dirs::home_dir() {
            return home;
        }
    }
    PathBuf::from(raw)
}

fn sanitize_ssh_error(prefix: &str, error: ssh2::Error) -> String {
    format!("{prefix}：{}", sanitize_error_text(&error.to_string()))
}

fn sanitize_error_text(error: &str) -> String {
    error
        .replace("password", "[redacted]")
        .replace("Password", "[redacted]")
        .replace("PASSWORD", "[redacted]")
        .replace("passphrase", "[redacted]")
        .replace("Passphrase", "[redacted]")
        .replace("PASSPHRASE", "[redacted]")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_thresholds_scale_with_timeout() {
        // 短命令保持 8s 灵敏阈值
        assert_eq!(idle_thresholds(30), (8, 8));
        // 长命令静默预算随 timeout 放大，最长 60s；提示符证据始终 8s
        assert_eq!(idle_thresholds(120), (8, 30));
        assert_eq!(idle_thresholds(300), (8, 60));
        // 超长 timeout 也不再超过静默上限
        assert_eq!(idle_thresholds(600), (8, 60));
    }

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
