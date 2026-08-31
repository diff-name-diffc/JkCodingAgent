use serde::{Deserialize, Serialize};

pub(super) const DEFAULT_TIMEOUT_SECS: u64 = 30;
pub(super) const DEFAULT_MAX_OUTPUT_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SshToolsConfig {
    #[serde(default)]
    pub servers: Vec<SshServerConfig>,
}

/// SSH 认证方式：密码或私钥文件。
/// 持久化行由应用全量序列化写入、恒携带该字段；`#[serde(default)]` 仅作
/// 反序列化容错（防御手工改库），默认密码认证。
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
    /// 显示名称（支持中文等任意字符），仅用于界面与智能体展示；留空时回退为 id。
    /// id 保持机器标识（小写字母/数字/-/_），不再由用户直接编辑。
    #[serde(default)]
    pub name: String,
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
    pub name: String,
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
