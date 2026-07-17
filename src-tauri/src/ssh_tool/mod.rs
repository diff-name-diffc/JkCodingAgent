use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use ssh2::{Channel, Session};
use tauri::State;

use crate::project::storage::atomic_write;

const CONFIG_FILE_NAME: &str = "ssh-tools.json";
const AUDIT_FILE_NAME: &str = "audit.json";
const AUDIT_RECORD_LIMIT: usize = 100;
const DEFAULT_TIMEOUT_SECS: u64 = 30;
const DEFAULT_MAX_OUTPUT_BYTES: usize = 64 * 1024;
const MAX_TIMEOUT_SECS: u64 = 300;
const MAX_OUTPUT_BYTES: usize = 1024 * 1024;
const ID_MAX_LEN: usize = 64;
// 交互阻塞检测：连续 N 秒无输出且 channel 未关闭 → 疑似等待交互输入
const IDLE_DETECT_SECS: u64 = 8;
const MIN_IDLE_SECS: u64 = 3;
const IDLE_POLL_MS: u64 = 150;
const READ_CHUNK: usize = 8192;
const MAX_STDIN_BYTES: usize = 512 * 1024;
const TIMEOUT_EXIT_CODE: i32 = 124;
const INTERACTIVE_EXIT_CODE: i32 = -1;
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
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SshAuthMethod {
    Password,
    Key,
}

impl Default for SshAuthMethod {
    fn default() -> Self {
        Self::Password
    }
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

#[derive(Clone, Default)]
pub struct SshSessionManager {
    sessions: Arc<Mutex<HashMap<SshSessionKey, Arc<Mutex<SshConnection>>>>>,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct SshSessionKey {
    project_path: String,
    server_id: String,
    session_id: String,
}

struct SshConnection {
    session: Session,
    last_used_at: Instant,
}

enum SshCommandStartError {
    Channel(ssh2::Error),
    Exec(ssh2::Error),
}

#[tauri::command]
pub async fn ssh_tool_load_config(project_path: String) -> Result<SshToolsConfig, String> {
    tokio::task::spawn_blocking(move || read_config(&PathBuf::from(project_path)))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn ssh_tool_load_audit(project_path: String) -> Result<SshAuditLog, String> {
    tokio::task::spawn_blocking(move || read_audit(&PathBuf::from(project_path)))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn ssh_tool_save_config(
    manager: State<'_, SshSessionManager>,
    project_path: String,
    config: SshToolsConfig,
) -> Result<SshToolsConfig, String> {
    let project_path_buf = PathBuf::from(project_path.clone());
    let cleaned = tokio::task::spawn_blocking(move || {
        let cleaned = validate_config(config)?;
        write_config(&project_path_buf, &cleaned)?;
        Ok::<_, String>(cleaned)
    })
    .await
    .map_err(|error| error.to_string())??;

    manager.drop_project_sessions(&project_path);
    Ok(cleaned)
}

#[tauri::command]
pub async fn ssh_tool_test_server_config(server: SshServerConfig) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        let config = validate_single_server(server)?;
        let session = connect(&config)?;
        session
            .disconnect(None, "connection test completed", None)
            .ok();
        Ok(format!("连接成功：{}", config.id))
    })
    .await
    .map_err(|error| error.to_string())?
}

impl SshSessionManager {
    pub fn list_servers(&self, project_path: &Path) -> Result<Vec<SshServerSummary>, String> {
        let config = read_config(project_path)?;
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
    }

    pub async fn list_servers_async(
        &self,
        project_path: PathBuf,
    ) -> Result<Vec<SshServerSummary>, String> {
        tokio::task::spawn_blocking(move || {
            let manager = SshSessionManager::default();
            manager.list_servers(&project_path)
        })
        .await
        .map_err(|error| error.to_string())?
    }

    /// 读取指定已启用服务器的完整配置（含 review_enabled 等字段），供审查门禁使用。
    pub async fn server_config_async(
        &self,
        project_path: PathBuf,
        server_id: String,
    ) -> Result<SshServerConfig, String> {
        tokio::task::spawn_blocking(move || find_enabled_server(&project_path, &server_id))
            .await
            .map_err(|error| error.to_string())?
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn execute(
        &self,
        project_path: PathBuf,
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
                project_path,
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
        project_path: PathBuf,
        workspace_id: String,
        session_title: String,
        server_id: String,
        session_id: String,
        command: String,
        review: SshAuditReview,
    ) -> Result<SshAuditRecord, String> {
        tokio::task::spawn_blocking(move || {
            let record = SshAuditRecord {
                created_at: Utc::now().to_rfc3339(),
                workspace_path: normalize_project_key(&project_path),
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
            write_audit_record(&project_path, record.clone())?;
            Ok(record)
        })
        .await
        .map_err(|error| error.to_string())?
    }

    #[allow(clippy::too_many_arguments)]
    fn execute_blocking(
        &self,
        project_path: PathBuf,
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
            &project_path,
            server_id.clone(),
            session_id.clone(),
            command.clone(),
            stdin,
            timeout_secs,
        );
        let audit_result = write_audit_record(
            &project_path,
            SshAuditRecord::from_execution(
                &project_path,
                workspace_id,
                session_title,
                server_id,
                session_id,
                command,
                &result,
                review.as_ref(),
            ),
        );
        if let Err(error) = audit_result {
            return Err(format!("写入 SSH 审计记录失败：{error}"));
        }
        result
    }

    fn execute_command_blocking(
        &self,
        project_path: &Path,
        server_id: String,
        session_id: String,
        command: String,
        stdin: Option<String>,
        timeout_secs: Option<u64>,
    ) -> Result<SshExecResult, String> {
        let server = find_enabled_server(project_path, &server_id)?;
        let timeout_secs = timeout_secs.unwrap_or(server.default_timeout_secs);
        let timeout_secs = timeout_secs.clamp(1, MAX_TIMEOUT_SECS);
        let max_output_bytes = server.max_output_bytes.clamp(1, MAX_OUTPUT_BYTES);
        validate_session_id(&session_id)?;
        validate_command(&command)?;
        if let Some(input) = stdin.as_deref() {
            if input.len() > MAX_STDIN_BYTES {
                return Err(format!("错误：stdin 长度不能超过 {MAX_STDIN_BYTES} 字节"));
            }
        }

        let key = SshSessionKey {
            project_path: normalize_project_key(project_path),
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

    fn connection_for(
        &self,
        key: SshSessionKey,
        server: &SshServerConfig,
    ) -> Result<Arc<Mutex<SshConnection>>, String> {
        let mut sessions = self.sessions.lock();
        sessions.retain(|_, connection| {
            connection.lock().last_used_at.elapsed() < Duration::from_secs(30 * 60)
        });

        if let Some(connection) = sessions.get(&key) {
            if connection.lock().session.authenticated() {
                return Ok(Arc::clone(connection));
            }
        }

        let session = connect(server)?;
        let connection = Arc::new(Mutex::new(SshConnection {
            session,
            last_used_at: Instant::now(),
        }));
        sessions.insert(key, Arc::clone(&connection));
        Ok(connection)
    }

    fn drop_project_sessions(&self, project_path: &str) {
        let project_key = normalize_project_key(Path::new(project_path));
        self.sessions
            .lock()
            .retain(|key, _| key.project_path != project_key);
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
                stdout: output.stdout.clone(),
                stderr: output.stderr.clone(),
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
    connection: &Arc<Mutex<SshConnection>>,
    server_id: &str,
    session_id: &str,
    command: &str,
    stdin: Option<&str>,
    timeout_secs: u64,
    max_output_bytes: usize,
    started: Instant,
) -> Result<SshExecResult, SshCommandStartError> {
    let (mut guard, mut channel) = open_exec_channel(connection, command)?;

    // 在阻塞模式下写入 stdin（若有）并关闭输入端
    if let Some(input) = stdin {
        if !input.is_empty() {
            let _ = channel.write_all(input.as_bytes());
            let _ = channel.flush();
        }
    }
    let _ = channel.send_eof();

    // 切换非阻塞读取：libssh2 的 socket 超时会污染整个会话（缓存的连接需丢弃），
    // 改用非阻塞 + 空转计时来检测交互阻塞，会话保持可复用。
    // 当前命令独占连接互斥锁，中途切换阻塞模式是安全的。
    guard.session.set_blocking(false);
    let idle_secs = IDLE_DETECT_SECS.min(timeout_secs / 2).max(MIN_IDLE_SECS);
    let deadline = started + Duration::from_secs(timeout_secs);
    let (outcome, stdout_raw, stderr_raw, stdout_capped, stderr_capped) =
        drain_channel(&mut channel, max_output_bytes, idle_secs, deadline);
    guard.session.set_blocking(true);

    let stdout = finalize_output(&stdout_raw, stdout_capped);
    let (exit_code, stderr) = match outcome {
        DrainOutcome::Completed => {
            let _ = channel.wait_close();
            let code = channel.exit_status().unwrap_or(INTERACTIVE_EXIT_CODE);
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
                "\n[工具检测到命令疑似在等待交互输入（连续 {idle_secs}s 无输出且未退出，疑似：{hint}），已主动中止以免长时间挂起。最近输出结尾：{:?}。请改用非交互形式后重试：sudo→免密账号或 NOPASSWD；确认提示→加 -y/--yes；分页器→PAGER=cat、GIT_PAGER=cat；REPL→用 -e/-c 或通过 stdin 参数喂入。]",
                tail_snippet(&stdout_raw, &stderr_raw)
            ));
            (INTERACTIVE_EXIT_CODE, text)
        }
    };
    guard.last_used_at = Instant::now();

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

fn open_exec_channel<'a>(
    connection: &'a Arc<Mutex<SshConnection>>,
    command: &str,
) -> Result<(parking_lot::MutexGuard<'a, SshConnection>, Channel), SshCommandStartError> {
    let guard = connection.lock();
    let mut channel = guard
        .session
        .channel_session()
        .map_err(SshCommandStartError::Channel)?;
    channel.exec(command).map_err(SshCommandStartError::Exec)?;
    Ok((guard, channel))
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

fn ssh_dir(project_path: &Path) -> PathBuf {
    project_path
        .join(".jkcodingagent")
        .join("local_env")
        .join("ssh")
}

fn config_path(project_path: &Path) -> PathBuf {
    ssh_dir(project_path).join(CONFIG_FILE_NAME)
}

fn legacy_config_path(project_path: &Path) -> PathBuf {
    project_path.join(".jkcodingagent").join(CONFIG_FILE_NAME)
}

fn audit_path(project_path: &Path) -> PathBuf {
    ssh_dir(project_path).join(AUDIT_FILE_NAME)
}

fn read_config(project_path: &Path) -> Result<SshToolsConfig, String> {
    let mut path = config_path(project_path);
    if !path.exists() {
        let legacy = legacy_config_path(project_path);
        if legacy.exists() {
            path = legacy;
        }
    }
    if !path.exists() {
        return Ok(SshToolsConfig::default());
    }
    let raw = fs::read_to_string(&path)
        .map_err(|error| format!("读取 SSH 工具配置失败（{}）：{error}", path.display()))?;
    let config: SshToolsConfig = serde_json::from_str(&raw)
        .map_err(|error| format!("解析 SSH 工具配置失败（{}）：{error}", path.display()))?;
    validate_config(config)
}

fn read_audit(project_path: &Path) -> Result<SshAuditLog, String> {
    let path = audit_path(project_path);
    if !path.exists() {
        return Ok(SshAuditLog::default());
    }
    let raw = fs::read_to_string(&path)
        .map_err(|error| format!("读取 SSH 审计记录失败（{}）：{error}", path.display()))?;
    serde_json::from_str(&raw)
        .map_err(|error| format!("解析 SSH 审计记录失败（{}）：{error}", path.display()))
}

fn write_config(project_path: &Path, config: &SshToolsConfig) -> Result<(), String> {
    let path = config_path(project_path);
    let dir = path
        .parent()
        .ok_or_else(|| format!("无法解析 SSH 工具配置目录：{}", path.display()))?;
    fs::create_dir_all(dir).map_err(|error| error.to_string())?;
    let raw = serde_json::to_string_pretty(config).map_err(|error| error.to_string())?;
    atomic_write(&path, &raw).map_err(|error| error.to_string())
}

fn write_audit_record(project_path: &Path, record: SshAuditRecord) -> Result<(), String> {
    let path = audit_path(project_path);
    let dir = path
        .parent()
        .ok_or_else(|| format!("无法解析 SSH 审计目录：{}", path.display()))?;
    fs::create_dir_all(dir).map_err(|error| error.to_string())?;

    let mut audit = read_audit(project_path)?;
    audit.records.push(record);
    if audit.records.len() > AUDIT_RECORD_LIMIT {
        let overflow = audit.records.len() - AUDIT_RECORD_LIMIT;
        audit.records.drain(0..overflow);
    }
    let raw = serde_json::to_string_pretty(&audit).map_err(|error| error.to_string())?;
    atomic_write(&path, &raw).map_err(|error| error.to_string())
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

fn find_enabled_server(project_path: &Path, server_id: &str) -> Result<SshServerConfig, String> {
    let config = read_config(project_path)?;
    config
        .servers
        .into_iter()
        .find(|server| server.enabled && server.id == server_id)
        .ok_or_else(|| format!("未找到已启用的 SSH server：{server_id}"))
}

fn connect(server: &SshServerConfig) -> Result<Session, String> {
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

/// drain 通道的最终结局：正常结束 / 疑似交互阻塞 / 超时
#[derive(Clone, Copy)]
enum DrainOutcome {
    Completed,
    InteractiveBlocked,
    TimedOut,
}

/// 在非阻塞模式下读取 channel 的 stdout 与 stderr。
/// 连续 `idle_secs` 秒无任何输出且通道未关闭 → 判定为交互阻塞；
/// 到达 `deadline` 仍未结束 → 超时；两端都 EOF → 正常结束。
fn drain_channel(
    channel: &mut Channel,
    max_output_bytes: usize,
    idle_secs: u64,
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
    let idle = Duration::from_secs(idle_secs);
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
        if now.duration_since(last_data_at) >= idle {
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
