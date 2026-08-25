//! MCP（Model Context Protocol）子系统：配置类型、作用域注册表、传输层。
//!
//! 两层配置模型：
//! - 全局注册表：SQLite `mcp_servers` 表，与应用生命周期相同，对所有聊天与项目生效；
//! - 项目级覆盖：`<repo>/.jkcodingagent/mcp.json`，随仓库走；同名服务器项目覆盖全局。
//!
//! 运行时入口是 `registry::McpRegistry`：按作用域缓存服务器健康状态与工具清单，
//! 工具通过 `agent/tools/mcp.rs` 的桥接进入工具注册表。

pub(crate) mod commands;
pub(crate) mod project_file;
pub(crate) mod registry;
pub(crate) mod transport;

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(crate) use project_file::ensure_project_mcp_file;
pub(crate) use registry::McpRegistry;

/// MCP 服务器配置（全局注册表与项目级 mcp.json 共用同一形状）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct McpServerConfig {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default, alias = "type")]
    pub transport: Option<String>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default, alias = "uri")]
    pub url: Option<String>,
    #[serde(default, alias = "socket_path")]
    pub socket_path: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default, alias = "startup_timeout_seconds")]
    pub startup_timeout_seconds: Option<u64>,
}

/// 一组 MCP 服务器配置。全局注册表与项目级 `mcp.json` 共用此形状。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpConfig {
    #[serde(default, rename = "mcpServers", alias = "servers")]
    pub servers: BTreeMap<String, McpServerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpAggregateStatus {
    NotConfigured,
    Healthy,
    Degraded,
    InvalidConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpServerState {
    Disabled,
    Healthy,
    InvalidConfig,
    SpawnFailed,
    ConnectionFailed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpToolTaskSupport {
    Forbidden,
    Optional,
    Required,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolStatus {
    pub name: String,
    pub exposed_name: String,
    pub description: String,
    pub task_support: McpToolTaskSupport,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerStatus {
    pub name: String,
    pub transport: String,
    pub enabled: bool,
    pub state: McpServerState,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub tool_count: usize,
    pub tools: Vec<McpToolStatus>,
}

/// 一次作用域检查的结果：全部服务器的健康状态与聚合。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpStatus {
    pub project_path: String,
    pub config_path: String,
    pub aggregate: McpAggregateStatus,
    pub checked_at: i64,
    pub server_count: usize,
    pub enabled_server_count: usize,
    pub healthy_server_count: usize,
    pub servers: Vec<McpServerStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_error: Option<String>,
}

/// 通过校验并解析出传输配置的单个工具。
#[derive(Debug, Clone)]
pub struct ResolvedMcpTool {
    pub canonical_name: String,
    pub original_name: String,
    pub server_name: String,
    pub description: String,
    pub parameters: Value,
    pub task_support: McpToolTaskSupport,
}

/// 已解析的服务器传输配置。
#[derive(Debug, Clone)]
pub enum ResolvedMcpTransport {
    Stdio {
        command: String,
        args: Vec<String>,
        env: BTreeMap<String, String>,
        cwd: Option<std::path::PathBuf>,
    },
    StreamableHttp {
        url: String,
        headers: HashMap<reqwest::header::HeaderName, reqwest::header::HeaderValue>,
    },
    UnixSocketHttp {
        socket_path: String,
        url: String,
        headers: HashMap<reqwest::header::HeaderName, reqwest::header::HeaderValue>,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedMcpServerConfig {
    pub(crate) transport_label: String,
    pub(crate) transport: ResolvedMcpTransport,
    pub(crate) startup_timeout: Duration,
}

/// 一个作用域的检查快照：状态 + 已解析工具/配置（执行期复用，避免重新连接）。
#[derive(Debug, Clone)]
pub struct McpSnapshot {
    pub status: McpStatus,
    tools_by_name: HashMap<String, ResolvedMcpTool>,
    server_configs: HashMap<String, ResolvedMcpServerConfig>,
}

impl McpSnapshot {
    pub(crate) fn new(
        status: McpStatus,
        tools_by_name: HashMap<String, ResolvedMcpTool>,
        server_configs: HashMap<String, ResolvedMcpServerConfig>,
    ) -> Self {
        Self {
            status,
            tools_by_name,
            server_configs,
        }
    }

    pub fn tools(&self) -> impl Iterator<Item = &ResolvedMcpTool> {
        self.tools_by_name.values()
    }

    pub fn tool_by_name(&self, name: &str) -> Option<&ResolvedMcpTool> {
        self.tools_by_name.get(name)
    }

    pub(crate) fn server_config(&self, name: &str) -> Option<&ResolvedMcpServerConfig> {
        self.server_configs.get(name)
    }
}

/// 把快照内的工具整理为排序后的定义列表（供工具注册表桥接）。
pub fn tool_definitions_from_snapshot(snapshot: Option<&McpSnapshot>) -> Vec<ResolvedMcpTool> {
    let Some(snapshot) = snapshot else {
        return Vec::new();
    };

    let mut tools = snapshot.tools().cloned().collect::<Vec<_>>();
    tools.sort_by(|left, right| left.canonical_name.cmp(&right.canonical_name));
    tools
}
