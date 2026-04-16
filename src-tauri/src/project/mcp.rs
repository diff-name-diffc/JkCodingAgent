use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::{Mutex, RwLock};
use reqwest::header::{HeaderName, HeaderValue};
use rmcp::model::{CallToolRequestParams, JsonObject, TaskSupport, Tool};
use rmcp::transport::{
    streamable_http_client::StreamableHttpClientTransportConfig, ConfigureCommandExt,
    StreamableHttpClientTransport, TokioChildProcess,
};
use rmcp::ServiceExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::State;
use tokio::io::AsyncReadExt;

use super::storage::atomic_write;

const DEFAULT_PROJECT_MCP_CONFIG: &str = r#"{
  "mcpServers": {}
}
"#;

const MCP_REFRESH_MAX_AGE: Duration = Duration::from_secs(300);
const DEFAULT_MCP_STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
const STDERR_CAPTURE_LIMIT: usize = 8 * 1024;
const STDERR_CAPTURE_SETTLE_TIMEOUT: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectMcpConfig {
    #[serde(default, rename = "mcpServers", alias = "servers")]
    pub servers: BTreeMap<String, ProjectMcpServerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProjectMcpServerConfig {
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectMcpAggregateStatus {
    NotConfigured,
    Healthy,
    Degraded,
    InvalidConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectMcpServerState {
    Disabled,
    Healthy,
    InvalidConfig,
    SpawnFailed,
    ConnectionFailed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectMcpToolTaskSupport {
    Forbidden,
    Optional,
    Required,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectMcpToolStatus {
    pub name: String,
    pub exposed_name: String,
    pub description: String,
    pub task_support: ProjectMcpToolTaskSupport,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectMcpServerStatus {
    pub name: String,
    pub transport: String,
    pub enabled: bool,
    pub state: ProjectMcpServerState,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub tool_count: usize,
    pub tools: Vec<ProjectMcpToolStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectMcpStatus {
    pub project_path: String,
    pub config_path: String,
    pub aggregate: ProjectMcpAggregateStatus,
    pub checked_at: i64,
    pub server_count: usize,
    pub enabled_server_count: usize,
    pub healthy_server_count: usize,
    pub servers: Vec<ProjectMcpServerStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ResolvedMcpTool {
    pub canonical_name: String,
    pub original_name: String,
    pub server_name: String,
    pub description: String,
    pub parameters: Value,
    pub task_support: ProjectMcpToolTaskSupport,
}

#[derive(Debug, Clone)]
pub struct WorkspaceMcpSnapshot {
    pub status: ProjectMcpStatus,
    tools_by_name: HashMap<String, ResolvedMcpTool>,
    server_configs: HashMap<String, ResolvedProjectMcpServerConfig>,
}

impl WorkspaceMcpSnapshot {
    pub fn tools(&self) -> impl Iterator<Item = &ResolvedMcpTool> {
        self.tools_by_name.values()
    }

    pub fn tool_by_name(&self, name: &str) -> Option<&ResolvedMcpTool> {
        self.tools_by_name.get(name)
    }

    pub(crate) fn server_config(&self, name: &str) -> Option<&ResolvedProjectMcpServerConfig> {
        self.server_configs.get(name)
    }
}

#[derive(Clone, Default)]
pub struct ProjectMcpRegistry {
    cache: Arc<RwLock<HashMap<String, WorkspaceMcpSnapshot>>>,
}

#[derive(Debug, Clone)]
enum ResolvedProjectMcpTransport {
    Stdio {
        command: String,
        args: Vec<String>,
        env: BTreeMap<String, String>,
        cwd: Option<PathBuf>,
    },
    StreamableHttp {
        url: String,
        headers: HashMap<HeaderName, HeaderValue>,
    },
    UnixSocketHttp {
        socket_path: String,
        url: String,
        headers: HashMap<HeaderName, HeaderValue>,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedProjectMcpServerConfig {
    transport_label: String,
    transport: ResolvedProjectMcpTransport,
    startup_timeout: Duration,
}

#[derive(Debug)]
struct CheckedServer {
    status: ProjectMcpServerStatus,
    resolved_config: Option<ResolvedProjectMcpServerConfig>,
    resolved_tools: Vec<ResolvedMcpTool>,
}

struct SpawnedStdioMcpProcess {
    transport: TokioChildProcess,
    stderr_buffer: Arc<Mutex<String>>,
    stderr_task: Option<tokio::task::JoinHandle<()>>,
}

#[derive(Debug)]
struct LoadedProjectMcpConfig {
    config_path: PathBuf,
    config: Result<ProjectMcpConfig, String>,
}

#[tauri::command]
pub async fn refresh_project_mcp_status(
    registry: State<'_, ProjectMcpRegistry>,
    project_path: String,
) -> Result<ProjectMcpStatus, String> {
    registry
        .refresh(&project_path)
        .await
        .map(|snapshot| snapshot.status)
}

#[tauri::command]
pub async fn set_project_mcp_server_enabled(
    registry: State<'_, ProjectMcpRegistry>,
    project_path: String,
    server_name: String,
    enabled: bool,
) -> Result<ProjectMcpStatus, String> {
    let project_path_buf = PathBuf::from(&project_path);
    tokio::task::spawn_blocking({
        let project_path_buf = project_path_buf.clone();
        let server_name = server_name.clone();
        move || set_project_mcp_server_enabled_sync(&project_path_buf, &server_name, enabled)
    })
    .await
    .map_err(|error| error.to_string())??;

    registry
        .refresh(&project_path)
        .await
        .map(|snapshot| snapshot.status)
}

pub fn ensure_project_mcp_file(project_path: &str) -> Result<(), String> {
    let config_dir = Path::new(project_path).join(".jkcodingagent");
    std::fs::create_dir_all(&config_dir).map_err(|error| error.to_string())?;
    let config_path = config_dir.join("mcp.json");
    if config_path.exists() {
        return Ok(());
    }
    atomic_write(&config_path, DEFAULT_PROJECT_MCP_CONFIG)
}

impl ProjectMcpRegistry {
    pub async fn ensure_recent(&self, project_path: &Path) -> Result<WorkspaceMcpSnapshot, String> {
        let key = workspace_cache_key(project_path);
        if let Some(snapshot) = self.cached(&key) {
            let age_ms = current_timestamp_millis().saturating_sub(snapshot.status.checked_at);
            if age_ms <= MCP_REFRESH_MAX_AGE.as_millis() as i64 {
                return Ok(snapshot);
            }
        }

        self.refresh(project_path.to_string_lossy().as_ref()).await
    }

    pub fn cached_for_workspace(&self, workspace: &Path) -> Option<WorkspaceMcpSnapshot> {
        self.cached(&workspace_cache_key(workspace))
    }

    pub async fn refresh(&self, project_path: &str) -> Result<WorkspaceMcpSnapshot, String> {
        let project_path = PathBuf::from(project_path);
        let cache_key = workspace_cache_key(&project_path);
        let loaded = tokio::task::spawn_blocking({
            let project_path = project_path.clone();
            move || read_project_mcp_config_sync(&project_path)
        })
        .await
        .map_err(|error| error.to_string())??;

        let checked_at = current_timestamp_millis();
        let snapshot = match loaded.config {
            Ok(config) => {
                let mut statuses = Vec::new();
                let mut resolved_tools = HashMap::new();
                let mut resolved_servers = HashMap::new();

                for (server_name, server_config) in config.servers {
                    let checked =
                        check_project_mcp_server(&project_path, &server_name, server_config).await;
                    if let Some(resolved_config) = checked.resolved_config {
                        resolved_servers.insert(server_name.clone(), resolved_config);
                    }
                    for tool in checked.resolved_tools {
                        resolved_tools.insert(tool.canonical_name.clone(), tool);
                    }
                    statuses.push(checked.status);
                }

                statuses.sort_by(|left, right| left.name.cmp(&right.name));
                let (aggregate, enabled_server_count, healthy_server_count) =
                    aggregate_server_statuses(&statuses);

                WorkspaceMcpSnapshot {
                    status: ProjectMcpStatus {
                        project_path: project_path.to_string_lossy().into_owned(),
                        config_path: loaded.config_path.to_string_lossy().into_owned(),
                        aggregate,
                        checked_at,
                        server_count: statuses.len(),
                        enabled_server_count,
                        healthy_server_count,
                        servers: statuses,
                        config_error: None,
                    },
                    tools_by_name: resolved_tools,
                    server_configs: resolved_servers,
                }
            }
            Err(config_error) => WorkspaceMcpSnapshot {
                status: ProjectMcpStatus {
                    project_path: project_path.to_string_lossy().into_owned(),
                    config_path: loaded.config_path.to_string_lossy().into_owned(),
                    aggregate: ProjectMcpAggregateStatus::InvalidConfig,
                    checked_at,
                    server_count: 0,
                    enabled_server_count: 0,
                    healthy_server_count: 0,
                    servers: vec![],
                    config_error: Some(config_error),
                },
                tools_by_name: HashMap::new(),
                server_configs: HashMap::new(),
            },
        };

        self.cache.write().insert(cache_key, snapshot.clone());
        Ok(snapshot)
    }

    pub async fn execute_tool(
        &self,
        workspace: &Path,
        tool_name: &str,
        arguments: &Value,
    ) -> Result<String, String> {
        let snapshot = self.ensure_recent(workspace).await?;
        let Some(tool) = snapshot.tool_by_name(tool_name).cloned() else {
            return Err(format!("错误：未找到 MCP 工具 '{tool_name}'"));
        };
        let Some(server_config) = snapshot.server_config(&tool.server_name).cloned() else {
            return Err(format!(
                "错误：未找到 MCP server '{}' 的有效配置",
                tool.server_name
            ));
        };

        let mut call = CallToolRequestParams::new(tool.original_name.clone());
        if let Some(arguments) = value_to_json_object(arguments)? {
            call = call.with_arguments(arguments);
        }
        if matches!(tool.task_support, ProjectMcpToolTaskSupport::Required) {
            call = call.with_task(JsonObject::new());
        }

        let result = match &server_config.transport {
            ResolvedProjectMcpTransport::Stdio {
                command,
                args,
                env,
                cwd,
            } => {
                let spawned = match spawn_stdio_mcp_process(command, args, env, cwd) {
                    Ok(spawned) => spawned,
                    Err(error) => return Err(error),
                };
                let SpawnedStdioMcpProcess {
                    transport,
                    stderr_buffer,
                    stderr_task,
                } = spawned;

                let result = tokio::time::timeout(server_config.startup_timeout, async move {
                    let client = ().serve(transport).await.map_err(|error| error.to_string())?;
                    let result = client
                        .call_tool(call)
                        .await
                        .map_err(|error| error.to_string());
                    let _ = client.cancel().await;
                    result
                })
                .await;
                let stderr_output = collect_captured_stderr(stderr_buffer, stderr_task).await;

                match result {
                    Ok(Ok(value)) => Ok(value),
                    Ok(Err(error)) => Err((
                        ProjectMcpServerState::ConnectionFailed,
                        enrich_stdio_error(&server_config, error, stderr_output.as_deref()),
                    )),
                    Err(_) => Err((
                        ProjectMcpServerState::ConnectionFailed,
                        build_stdio_timeout_error(
                            &server_config,
                            "工具调用",
                            stderr_output.as_deref(),
                        ),
                    )),
                }
            }
            ResolvedProjectMcpTransport::StreamableHttp { url, headers } => {
                timeout_tool_call(
                    server_config.startup_timeout,
                    async {
                        let transport = build_streamable_http_transport(url, headers)?;
                        let client = ().serve(transport).await.map_err(|error| error.to_string())?;
                        let result = client
                            .call_tool(call)
                            .await
                            .map_err(|error| error.to_string())?;
                        let _ = client.cancel().await;
                        Ok::<_, String>(result)
                    },
                    build_timeout_error("MCP 工具调用", server_config.startup_timeout),
                )
                .await
            }
            ResolvedProjectMcpTransport::UnixSocketHttp {
                socket_path,
                url,
                headers,
            } => {
                timeout_tool_call(
                    server_config.startup_timeout,
                    async {
                        let transport = build_unix_socket_transport(socket_path, url, headers)?;
                        let client = ().serve(transport).await.map_err(|error| error.to_string())?;
                        let result = client
                            .call_tool(call)
                            .await
                            .map_err(|error| error.to_string())?;
                        let _ = client.cancel().await;
                        Ok::<_, String>(result)
                    },
                    build_timeout_error("MCP 工具调用", server_config.startup_timeout),
                )
                .await
            }
        }
        .map_err(|error| error.1)?;

        serde_json::to_string_pretty(&result).map_err(|error| error.to_string())
    }

    fn cached(&self, key: &str) -> Option<WorkspaceMcpSnapshot> {
        self.cache.read().get(key).cloned()
    }
}

pub fn build_workspace_mcp_prompt_block(
    snapshot: Option<&WorkspaceMcpSnapshot>,
    workspace: &Path,
) -> String {
    let Some(snapshot) = snapshot else {
        return String::new();
    };

    let mut lines = vec![
        "# 项目级 MCP 状态".to_string(),
        format!("工作区：{}", workspace.display()),
        format!("配置文件：{}", snapshot.status.config_path),
        format!(
            "整体状态：{}",
            aggregate_status_label(&snapshot.status.aggregate)
        ),
    ];

    if let Some(config_error) = &snapshot.status.config_error {
        lines.push(format!("配置错误：{config_error}"));
        return lines.join("\n");
    }

    if snapshot.status.servers.is_empty() {
        lines.push("当前项目没有配置任何 MCP server。".to_string());
        return lines.join("\n");
    }

    lines.push(
        "规则：仅调用状态为 healthy 的 MCP 工具；工具名采用 mcp__{server}__{tool} 前缀。"
            .to_string(),
    );
    for server in &snapshot.status.servers {
        lines.push(format!(
            "- server={} enabled={} transport={} state={} tools={}",
            server.name,
            server.enabled,
            server.transport,
            server_state_label(&server.state),
            server.tool_count
        ));
        if let Some(error) = &server.error {
            lines.push(format!("  error={error}"));
        }
        for tool in &server.tools {
            lines.push(format!(
                "  tool={} original={} task_support={}",
                tool.exposed_name,
                tool.name,
                task_support_label(&tool.task_support)
            ));
        }
    }

    lines.join("\n")
}

pub fn tool_definitions_from_snapshot(
    snapshot: Option<&WorkspaceMcpSnapshot>,
) -> Vec<ResolvedMcpTool> {
    let Some(snapshot) = snapshot else {
        return Vec::new();
    };

    let mut tools = snapshot.tools().cloned().collect::<Vec<_>>();
    tools.sort_by(|left, right| left.canonical_name.cmp(&right.canonical_name));
    tools
}

async fn check_project_mcp_server(
    project_path: &Path,
    server_name: &str,
    server_config: ProjectMcpServerConfig,
) -> CheckedServer {
    if !server_enabled(&server_config) {
        return CheckedServer {
            status: ProjectMcpServerStatus {
                name: server_name.to_string(),
                transport: resolve_transport_kind(&server_config)
                    .unwrap_or_else(|_| "unknown".to_string()),
                enabled: false,
                state: ProjectMcpServerState::Disabled,
                summary: "已禁用".to_string(),
                error: None,
                tool_count: 0,
                tools: vec![],
            },
            resolved_config: None,
            resolved_tools: vec![],
        };
    }

    let resolved = match resolve_project_mcp_server_config(project_path, server_name, server_config)
    {
        Ok(resolved) => resolved,
        Err(error) => {
            return CheckedServer {
                status: ProjectMcpServerStatus {
                    name: server_name.to_string(),
                    transport: "unknown".to_string(),
                    enabled: true,
                    state: ProjectMcpServerState::InvalidConfig,
                    summary: "配置无效".to_string(),
                    error: Some(error),
                    tool_count: 0,
                    tools: vec![],
                },
                resolved_config: None,
                resolved_tools: vec![],
            };
        }
    };

    let tools_result = list_server_tools(&resolved).await;
    match tools_result {
        Ok(tools) => {
            let mut used_names = HashMap::<String, usize>::new();
            let resolved_tools = tools
                .into_iter()
                .map(|tool| resolve_mcp_tool(server_name, tool, &mut used_names))
                .collect::<Vec<_>>();
            let public_tools = resolved_tools
                .iter()
                .map(|tool| ProjectMcpToolStatus {
                    name: tool.original_name.clone(),
                    exposed_name: tool.canonical_name.clone(),
                    description: tool.description.clone(),
                    task_support: tool.task_support.clone(),
                })
                .collect::<Vec<_>>();
            CheckedServer {
                status: ProjectMcpServerStatus {
                    name: server_name.to_string(),
                    transport: resolved.transport_label.clone(),
                    enabled: true,
                    state: ProjectMcpServerState::Healthy,
                    summary: format!("已连接，发现 {} 个工具", public_tools.len()),
                    error: None,
                    tool_count: public_tools.len(),
                    tools: public_tools,
                },
                resolved_config: Some(resolved),
                resolved_tools,
            }
        }
        Err((state, error)) => CheckedServer {
            status: ProjectMcpServerStatus {
                name: server_name.to_string(),
                transport: resolved.transport_label.clone(),
                enabled: true,
                state,
                summary: "校验失败".to_string(),
                error: Some(error),
                tool_count: 0,
                tools: vec![],
            },
            resolved_config: None,
            resolved_tools: vec![],
        },
    }
}

async fn list_server_tools(
    server: &ResolvedProjectMcpServerConfig,
) -> Result<Vec<Tool>, (ProjectMcpServerState, String)> {
    match &server.transport {
        ResolvedProjectMcpTransport::Stdio {
            command,
            args,
            env,
            cwd,
        } => {
            let spawned = spawn_stdio_mcp_process(command, args, env, cwd)
                .map_err(|error| (ProjectMcpServerState::SpawnFailed, error))?;
            let SpawnedStdioMcpProcess {
                transport,
                stderr_buffer,
                stderr_task,
            } = spawned;

            let result = tokio::time::timeout(server.startup_timeout, async move {
                let client = ().serve(transport).await.map_err(|error| {
                    (ProjectMcpServerState::ConnectionFailed, error.to_string())
                })?;
                let result = client
                    .list_all_tools()
                    .await
                    .map_err(|error| (ProjectMcpServerState::ConnectionFailed, error.to_string()));
                let _ = client.cancel().await;
                result
            })
            .await;
            let stderr_output = collect_captured_stderr(stderr_buffer, stderr_task).await;

            match result {
                Ok(Ok(tools)) => Ok(tools),
                Ok(Err((state, error))) => Err((
                    state,
                    enrich_stdio_error(server, error, stderr_output.as_deref()),
                )),
                Err(_) => Err((
                    ProjectMcpServerState::ConnectionFailed,
                    build_stdio_timeout_error(server, "连接或握手", stderr_output.as_deref()),
                )),
            }
        }
        ResolvedProjectMcpTransport::StreamableHttp { url, headers } => {
            timeout_server_check(
                server.startup_timeout,
                async {
                    let transport = build_streamable_http_transport(url, headers)
                        .map_err(|error| (ProjectMcpServerState::InvalidConfig, error))?;
                    let client = ().serve(transport).await.map_err(|error| {
                        (ProjectMcpServerState::ConnectionFailed, error.to_string())
                    })?;
                    let result = client.list_all_tools().await.map_err(|error| {
                        (ProjectMcpServerState::ConnectionFailed, error.to_string())
                    });
                    let _ = client.cancel().await;
                    result
                },
                build_timeout_error("MCP 连接或握手", server.startup_timeout),
            )
            .await
        }
        ResolvedProjectMcpTransport::UnixSocketHttp {
            socket_path,
            url,
            headers,
        } => {
            timeout_server_check(
                server.startup_timeout,
                async {
                    let transport = build_unix_socket_transport(socket_path, url, headers)
                        .map_err(|error| (ProjectMcpServerState::InvalidConfig, error))?;
                    let client = ().serve(transport).await.map_err(|error| {
                        (ProjectMcpServerState::ConnectionFailed, error.to_string())
                    })?;
                    let result = client.list_all_tools().await.map_err(|error| {
                        (ProjectMcpServerState::ConnectionFailed, error.to_string())
                    });
                    let _ = client.cancel().await;
                    result
                },
                build_timeout_error("MCP 连接或握手", server.startup_timeout),
            )
            .await
        }
    }
}

fn resolve_project_mcp_server_config(
    project_path: &Path,
    server_name: &str,
    config: ProjectMcpServerConfig,
) -> Result<ResolvedProjectMcpServerConfig, String> {
    let startup_timeout_seconds = config
        .startup_timeout_seconds
        .unwrap_or(DEFAULT_MCP_STARTUP_TIMEOUT.as_secs());
    if startup_timeout_seconds == 0 {
        return Err(format!(
            "MCP server '{server_name}' 的 startupTimeoutSeconds 必须大于 0"
        ));
    }
    let transport = resolve_transport_kind(&config)?;
    match transport.as_str() {
        "stdio" => {
            let command = config
                .command
                .ok_or_else(|| format!("MCP server '{server_name}' 缺少 command 字段"))?;
            let cwd = config
                .cwd
                .map(|cwd| resolve_optional_path(project_path, &cwd));
            Ok(ResolvedProjectMcpServerConfig {
                transport_label: "stdio".to_string(),
                transport: ResolvedProjectMcpTransport::Stdio {
                    command,
                    args: config.args,
                    env: config.env,
                    cwd,
                },
                startup_timeout: Duration::from_secs(startup_timeout_seconds),
            })
        }
        "streamable_http" => {
            let url = config
                .url
                .ok_or_else(|| format!("MCP server '{server_name}' 缺少 url 字段"))?;
            Ok(ResolvedProjectMcpServerConfig {
                transport_label: "streamable_http".to_string(),
                transport: ResolvedProjectMcpTransport::StreamableHttp {
                    url,
                    headers: build_header_map(&config.headers)?,
                },
                startup_timeout: Duration::from_secs(startup_timeout_seconds),
            })
        }
        "unix_socket_http" => {
            let socket_path = config
                .socket_path
                .ok_or_else(|| format!("MCP server '{server_name}' 缺少 socketPath 字段"))?;
            let url = config
                .url
                .ok_or_else(|| format!("MCP server '{server_name}' 缺少 url 字段"))?;
            Ok(ResolvedProjectMcpServerConfig {
                transport_label: "unix_socket_http".to_string(),
                transport: ResolvedProjectMcpTransport::UnixSocketHttp {
                    socket_path,
                    url,
                    headers: build_header_map(&config.headers)?,
                },
                startup_timeout: Duration::from_secs(startup_timeout_seconds),
            })
        }
        _ => Err(format!(
            "MCP server '{server_name}' 使用了不支持的 transport: {transport}"
        )),
    }
}

fn resolve_transport_kind(config: &ProjectMcpServerConfig) -> Result<String, String> {
    if let Some(transport) = config
        .transport
        .as_ref()
        .map(|value| normalize_transport_name(value))
    {
        return Ok(transport);
    }

    if config.command.is_some() {
        return Ok("stdio".to_string());
    }
    if config.socket_path.is_some() {
        return Ok("unix_socket_http".to_string());
    }
    if config.url.is_some() {
        return Ok("streamable_http".to_string());
    }

    Err("无法推断 MCP transport；请显式配置 transport 字段".to_string())
}

fn normalize_transport_name(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "stdio" => "stdio".to_string(),
        "http" | "streamable-http" | "streamable_http" | "streamablehttp" => {
            "streamable_http".to_string()
        }
        "unix_socket_http" | "unix-socket-http" | "unixsockethttp" => {
            "unix_socket_http".to_string()
        }
        other => other.to_string(),
    }
}

fn build_stdio_command(
    command: &str,
    args: &[String],
    env: &BTreeMap<String, String>,
    cwd: &Option<PathBuf>,
) -> tokio::process::Command {
    tokio::process::Command::new(command).configure(|inner| {
        inner.args(args);
        if let Some(cwd) = cwd {
            inner.current_dir(cwd);
        }
        if !env.is_empty() {
            inner.envs(env);
        }
    })
}

fn build_streamable_http_transport(
    url: &str,
    headers: &HashMap<HeaderName, HeaderValue>,
) -> Result<StreamableHttpClientTransport<reqwest::Client>, String> {
    let config = StreamableHttpClientTransportConfig::with_uri(url.to_string())
        .custom_headers(headers.clone());
    Ok(StreamableHttpClientTransport::from_config(config))
}

fn build_unix_socket_transport(
    socket_path: &str,
    url: &str,
    headers: &HashMap<HeaderName, HeaderValue>,
) -> Result<StreamableHttpClientTransport<rmcp::transport::UnixSocketHttpClient>, String> {
    let config = StreamableHttpClientTransportConfig::with_uri(url.to_string())
        .custom_headers(headers.clone());
    Ok(StreamableHttpClientTransport::from_unix_socket_with_config(
        socket_path,
        config,
    ))
}

fn build_header_map(
    headers: &BTreeMap<String, String>,
) -> Result<HashMap<HeaderName, HeaderValue>, String> {
    let mut header_map = HashMap::new();
    for (name, value) in headers {
        let header_name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|error| format!("无效的 HTTP header 名称 '{name}': {error}"))?;
        let header_value = HeaderValue::from_str(value)
            .map_err(|error| format!("无效的 HTTP header 值: {error}"))?;
        header_map.insert(header_name, header_value);
    }
    Ok(header_map)
}

fn resolve_optional_path(project_path: &Path, raw_path: &str) -> PathBuf {
    let candidate = PathBuf::from(raw_path);
    if candidate.is_absolute() {
        candidate
    } else {
        project_path.join(candidate)
    }
}

fn resolve_mcp_tool(
    server_name: &str,
    tool: Tool,
    used_names: &mut HashMap<String, usize>,
) -> ResolvedMcpTool {
    let original_name = tool.name.to_string();
    let canonical_base = format!(
        "mcp__{}__{}",
        sanitize_tool_name(server_name),
        sanitize_tool_name(&original_name)
    );
    let index = used_names.entry(canonical_base.clone()).or_insert(0);
    *index += 1;
    let canonical_name = if *index == 1 {
        canonical_base
    } else {
        format!("{canonical_base}__{index}")
    };

    ResolvedMcpTool {
        canonical_name,
        original_name,
        server_name: server_name.to_string(),
        description: tool
            .description
            .as_ref()
            .map(|desc| desc.to_string())
            .unwrap_or_else(|| format!("MCP 工具（server={server_name}）")),
        parameters: Value::Object((*tool.input_schema).clone()),
        task_support: match tool
            .execution
            .as_ref()
            .and_then(|execution| execution.task_support)
        {
            Some(TaskSupport::Optional) => ProjectMcpToolTaskSupport::Optional,
            Some(TaskSupport::Required) => ProjectMcpToolTaskSupport::Required,
            _ => ProjectMcpToolTaskSupport::Forbidden,
        },
    }
}

fn sanitize_tool_name(raw: &str) -> String {
    let mut sanitized = String::with_capacity(raw.len());
    for character in raw.chars() {
        if character.is_ascii_alphanumeric() {
            sanitized.push(character.to_ascii_lowercase());
        } else {
            sanitized.push('_');
        }
    }
    let sanitized = sanitized.trim_matches('_');
    if sanitized.is_empty() {
        "tool".to_string()
    } else {
        sanitized.to_string()
    }
}

fn read_project_mcp_config_sync(project_path: &Path) -> Result<LoadedProjectMcpConfig, String> {
    ensure_project_mcp_file(project_path.to_string_lossy().as_ref())?;
    let config_path = project_path.join(".jkcodingagent").join("mcp.json");
    let raw = std::fs::read_to_string(&config_path).map_err(|error| error.to_string())?;
    let config = serde_json::from_str::<ProjectMcpConfig>(&raw)
        .map_err(|error| format!("解析 {} 失败：{error}", config_path.display()));
    Ok(LoadedProjectMcpConfig {
        config_path,
        config,
    })
}

fn write_project_mcp_config_sync(
    project_path: &Path,
    config: &ProjectMcpConfig,
) -> Result<(), String> {
    ensure_project_mcp_file(project_path.to_string_lossy().as_ref())?;
    let config_path = project_path.join(".jkcodingagent").join("mcp.json");
    let raw = serde_json::to_string_pretty(config).map_err(|error| error.to_string())?;
    atomic_write(&config_path, &raw)
}

fn set_project_mcp_server_enabled_sync(
    project_path: &Path,
    server_name: &str,
    enabled: bool,
) -> Result<(), String> {
    let loaded = read_project_mcp_config_sync(project_path)?;
    let mut config = loaded.config?;
    let server = config
        .servers
        .get_mut(server_name)
        .ok_or_else(|| format!("未找到 MCP server '{server_name}'"))?;
    server.enabled = Some(enabled);
    write_project_mcp_config_sync(project_path, &config)
}

fn server_enabled(config: &ProjectMcpServerConfig) -> bool {
    config.enabled.unwrap_or(true)
}

fn aggregate_server_statuses(
    statuses: &[ProjectMcpServerStatus],
) -> (ProjectMcpAggregateStatus, usize, usize) {
    let enabled_server_count = statuses.iter().filter(|status| status.enabled).count();
    let healthy_server_count = statuses
        .iter()
        .filter(|status| status.enabled && matches!(status.state, ProjectMcpServerState::Healthy))
        .count();
    let aggregate = if statuses.is_empty() || enabled_server_count == 0 {
        ProjectMcpAggregateStatus::NotConfigured
    } else if healthy_server_count == enabled_server_count {
        ProjectMcpAggregateStatus::Healthy
    } else {
        ProjectMcpAggregateStatus::Degraded
    };
    (aggregate, enabled_server_count, healthy_server_count)
}

fn spawn_stdio_mcp_process(
    command: &str,
    args: &[String],
    env: &BTreeMap<String, String>,
    cwd: &Option<PathBuf>,
) -> Result<SpawnedStdioMcpProcess, String> {
    let (transport, stderr) =
        TokioChildProcess::builder(build_stdio_command(command, args, env, cwd))
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("启动 MCP server 进程失败：{error}"))?;

    let stderr_buffer = Arc::new(Mutex::new(String::new()));
    let stderr_task = stderr.map(|stderr| {
        let stderr_buffer = Arc::clone(&stderr_buffer);
        tokio::spawn(async move {
            capture_child_stderr(stderr, stderr_buffer).await;
        })
    });

    Ok(SpawnedStdioMcpProcess {
        transport,
        stderr_buffer,
        stderr_task,
    })
}

async fn capture_child_stderr(
    mut stderr: tokio::process::ChildStderr,
    stderr_buffer: Arc<Mutex<String>>,
) {
    let mut chunk = [0_u8; 1024];
    loop {
        match stderr.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                append_captured_stderr(&stderr_buffer, &String::from_utf8_lossy(&chunk[..read]))
            }
        }
    }
}

fn append_captured_stderr(stderr_buffer: &Arc<Mutex<String>>, chunk: &str) {
    let mut buffer = stderr_buffer.lock();
    if buffer.len() >= STDERR_CAPTURE_LIMIT {
        return;
    }

    let remaining = STDERR_CAPTURE_LIMIT - buffer.len();
    if chunk.len() <= remaining {
        buffer.push_str(chunk);
        return;
    }

    let mut boundary = 0;
    for (index, character) in chunk.char_indices() {
        let next = index + character.len_utf8();
        if next > remaining {
            break;
        }
        boundary = next;
    }

    if boundary > 0 {
        buffer.push_str(&chunk[..boundary]);
    }
}

async fn collect_captured_stderr(
    stderr_buffer: Arc<Mutex<String>>,
    stderr_task: Option<tokio::task::JoinHandle<()>>,
) -> Option<String> {
    if let Some(mut stderr_task) = stderr_task {
        tokio::select! {
            _ = &mut stderr_task => {}
            _ = tokio::time::sleep(STDERR_CAPTURE_SETTLE_TIMEOUT) => stderr_task.abort(),
        }
    }

    let stderr = stderr_buffer.lock().trim().to_string();
    if stderr.is_empty() {
        None
    } else {
        Some(stderr)
    }
}

fn enrich_stdio_error(
    server: &ResolvedProjectMcpServerConfig,
    mut error: String,
    stderr_output: Option<&str>,
) -> String {
    if let Some(diagnostic) = build_known_stdio_failure_diagnostic(server, &error, stderr_output) {
        error = format!("{diagnostic}\n\n原始错误：\n{error}");
    }
    if let Some(stderr_output) = stderr_output {
        error.push_str("\n\n原始 stderr：\n");
        error.push_str(stderr_output);
    }
    error
}

fn build_timeout_error(operation: &str, timeout: Duration) -> String {
    format!("{operation}超时（{} 秒）", timeout.as_secs())
}

fn build_stdio_timeout_error(
    server: &ResolvedProjectMcpServerConfig,
    operation: &str,
    stderr_output: Option<&str>,
) -> String {
    let mut message = build_timeout_error(&format!("MCP {operation}"), server.startup_timeout);
    if let ResolvedProjectMcpTransport::Stdio { command, args, .. } = &server.transport {
        if let Some(hint) = build_stdio_timeout_hint(command, args) {
            message.push('\n');
            message.push_str(&hint);
        }
    }
    if let Some(stderr_output) = stderr_output {
        message.push_str("\n\n原始 stderr：\n");
        message.push_str(stderr_output);
    }
    message
}

fn build_known_stdio_failure_diagnostic(
    server: &ResolvedProjectMcpServerConfig,
    error: &str,
    stderr_output: Option<&str>,
) -> Option<String> {
    let stderr = stderr_output.unwrap_or_default();
    let combined = format!("{error}\n{stderr}").to_ascii_lowercase();

    if combined.contains("connection closed: initialize response") {
        if let Some(hint) = build_ladybug_native_module_hint(server, &combined) {
            return Some(format!(
                "MCP server 在 initialize 阶段前退出。\n诊断：{hint}"
            ));
        }
        return Some("MCP server 在 initialize 阶段前退出，请先检查进程是否启动成功。".to_string());
    }

    build_ladybug_native_module_hint(server, &combined).map(|hint| format!("诊断：{hint}"))
}

fn build_ladybug_native_module_hint(
    server: &ResolvedProjectMcpServerConfig,
    combined: &str,
) -> Option<String> {
    if !(combined.contains("err_dlopen_failed")
        || combined.contains("lbugjs.node")
        || combined.contains("@ladybugdb/core"))
    {
        return None;
    }

    let generic =
        "检测到 native 模块加载失败，`@ladybugdb/core` 的 `lbugjs.node` 没有被正确放到主包目录。";
    if let ResolvedProjectMcpTransport::Stdio { command, args, .. } = &server.transport {
        if is_npx_command(command) && args.iter().any(|value| value.contains("gitnexus")) {
            return Some(format!(
                "{generic}\n建议：不要使用 `npx ... gitnexus@latest ...` 直接启动；优先改成已安装的稳定二进制，例如 `gitnexus` 或绝对路径。"
            ));
        }
    }

    Some(format!(
        "{generic}\n建议：改用已安装的稳定二进制，或先确认该 MCP 服务依赖的 native 模块已经正确安装。"
    ))
}

fn build_stdio_timeout_hint(command: &str, args: &[String]) -> Option<String> {
    if is_npx_command(command) && args.iter().any(|value| value.contains("@latest")) {
        return Some(
            "提示：`npx ... @latest` 首次冷启动通常更慢；如果经常超时，可在 `.jkcodingagent/mcp.json` 中设置 `startupTimeoutSeconds`，或改用已安装的本地二进制。"
                .to_string(),
        );
    }
    None
}

fn is_npx_command(command: &str) -> bool {
    let normalized = command.trim().to_ascii_lowercase();
    normalized == "npx"
        || normalized.ends_with("/npx")
        || normalized.ends_with("\\npx")
        || normalized.ends_with("npx.cmd")
        || normalized.ends_with("npx.exe")
}

async fn timeout_server_check<F, T>(
    timeout: Duration,
    future: F,
    timeout_message: String,
) -> Result<T, (ProjectMcpServerState, String)>
where
    F: std::future::Future<Output = Result<T, (ProjectMcpServerState, String)>>,
{
    match tokio::time::timeout(timeout, future).await {
        Ok(result) => result,
        Err(_) => Err((ProjectMcpServerState::ConnectionFailed, timeout_message)),
    }
}

async fn timeout_tool_call<F, T>(
    timeout: Duration,
    future: F,
    timeout_message: String,
) -> Result<T, (ProjectMcpServerState, String)>
where
    F: std::future::Future<Output = Result<T, String>>,
{
    match tokio::time::timeout(timeout, future).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err((ProjectMcpServerState::ConnectionFailed, error)),
        Err(_) => Err((ProjectMcpServerState::ConnectionFailed, timeout_message)),
    }
}

fn value_to_json_object(value: &Value) -> Result<Option<JsonObject>, String> {
    match value {
        Value::Null => Ok(None),
        Value::Object(map) if map.is_empty() => Ok(None),
        Value::Object(map) => Ok(Some(map.clone())),
        _ => Err("MCP 工具参数必须是 JSON object".to_string()),
    }
}

fn workspace_cache_key(workspace: &Path) -> String {
    workspace.to_string_lossy().into_owned()
}

fn current_timestamp_millis() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn aggregate_status_label(status: &ProjectMcpAggregateStatus) -> &'static str {
    match status {
        ProjectMcpAggregateStatus::NotConfigured => "not_configured",
        ProjectMcpAggregateStatus::Healthy => "healthy",
        ProjectMcpAggregateStatus::Degraded => "degraded",
        ProjectMcpAggregateStatus::InvalidConfig => "invalid_config",
    }
}

fn server_state_label(status: &ProjectMcpServerState) -> &'static str {
    match status {
        ProjectMcpServerState::Disabled => "disabled",
        ProjectMcpServerState::Healthy => "healthy",
        ProjectMcpServerState::InvalidConfig => "invalid_config",
        ProjectMcpServerState::SpawnFailed => "spawn_failed",
        ProjectMcpServerState::ConnectionFailed => "connection_failed",
    }
}

fn task_support_label(status: &ProjectMcpToolTaskSupport) -> &'static str {
    match status {
        ProjectMcpToolTaskSupport::Forbidden => "forbidden",
        ProjectMcpToolTaskSupport::Optional => "optional",
        ProjectMcpToolTaskSupport::Required => "required",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        aggregate_server_statuses, normalize_transport_name, resolve_project_mcp_server_config,
        sanitize_tool_name, ProjectMcpAggregateStatus, ProjectMcpConfig, ProjectMcpServerConfig,
        ProjectMcpServerState, ProjectMcpServerStatus, ResolvedProjectMcpTransport,
    };
    use std::path::Path;
    use std::time::Duration;

    #[test]
    fn sanitize_tool_name_replaces_non_ascii_segments() {
        assert_eq!(sanitize_tool_name("GitHub Search"), "github_search");
        assert_eq!(sanitize_tool_name("foo/bar:baz"), "foo_bar_baz");
    }

    #[test]
    fn resolve_transport_name_supports_aliases() {
        assert_eq!(
            normalize_transport_name("streamable-http"),
            "streamable_http"
        );
        assert_eq!(normalize_transport_name("http"), "streamable_http");
        assert_eq!(
            normalize_transport_name("unix-socket-http"),
            "unix_socket_http"
        );
    }

    #[test]
    fn resolve_relative_stdio_cwd_against_project_root() {
        let server = resolve_project_mcp_server_config(
            Path::new("/tmp/demo"),
            "memory",
            ProjectMcpServerConfig {
                transport: Some("stdio".to_string()),
                command: Some("npx".to_string()),
                cwd: Some("tools/memory".to_string()),
                ..ProjectMcpServerConfig::default()
            },
        )
        .expect("stdio config should resolve");

        match server.transport {
            ResolvedProjectMcpTransport::Stdio { cwd, .. } => {
                assert_eq!(
                    cwd.expect("cwd must exist"),
                    Path::new("/tmp/demo/tools/memory")
                );
            }
            _ => panic!("expected stdio transport"),
        }
    }

    #[test]
    fn parse_official_mcp_servers_shape() {
        let config = serde_json::from_str::<ProjectMcpConfig>(
            r#"{
              "mcpServers": {
                "gitnexus": {
                  "command": "npx",
                  "args": ["-y", "gitnexus@latest", "mcp"]
                }
              }
            }"#,
        )
        .expect("mcpServers shape should parse");

        let server = config
            .servers
            .get("gitnexus")
            .expect("gitnexus server must exist");
        assert_eq!(server.command.as_deref(), Some("npx"));
        assert_eq!(
            server.args,
            vec![
                "-y".to_string(),
                "gitnexus@latest".to_string(),
                "mcp".to_string()
            ]
        );
        assert_eq!(server.enabled, None);
    }

    #[test]
    fn parse_startup_timeout_seconds_from_official_shape() {
        let config = serde_json::from_str::<ProjectMcpConfig>(
            r#"{
              "mcpServers": {
                "gitnexus": {
                  "command": "npx",
                  "args": ["-y", "gitnexus@latest", "mcp"],
                  "startupTimeoutSeconds": 45
                }
              }
            }"#,
        )
        .expect("startupTimeoutSeconds should parse");

        let server = config
            .servers
            .get("gitnexus")
            .expect("gitnexus server must exist");
        assert_eq!(server.startup_timeout_seconds, Some(45));
    }

    #[test]
    fn resolve_startup_timeout_into_duration() {
        let server = resolve_project_mcp_server_config(
            Path::new("/tmp/demo"),
            "gitnexus",
            ProjectMcpServerConfig {
                command: Some("npx".to_string()),
                args: vec![
                    "-y".to_string(),
                    "gitnexus@latest".to_string(),
                    "mcp".to_string(),
                ],
                startup_timeout_seconds: Some(45),
                ..ProjectMcpServerConfig::default()
            },
        )
        .expect("config should resolve");

        assert_eq!(server.startup_timeout, Duration::from_secs(45));
    }

    #[test]
    fn parse_enabled_flag_from_server_shape() {
        let config = serde_json::from_str::<ProjectMcpConfig>(
            r#"{
              "mcpServers": {
                "gitnexus": {
                  "enabled": false,
                  "command": "/opt/homebrew/bin/gitnexus",
                  "args": ["mcp"]
                }
              }
            }"#,
        )
        .expect("enabled should parse");

        let server = config
            .servers
            .get("gitnexus")
            .expect("gitnexus server must exist");
        assert_eq!(server.enabled, Some(false));
    }

    #[test]
    fn aggregate_ignores_disabled_servers() {
        let statuses = vec![
            ProjectMcpServerStatus {
                name: "gitnexus".to_string(),
                transport: "stdio".to_string(),
                enabled: false,
                state: ProjectMcpServerState::Disabled,
                summary: "已禁用".to_string(),
                error: None,
                tool_count: 0,
                tools: vec![],
            },
            ProjectMcpServerStatus {
                name: "memory".to_string(),
                transport: "stdio".to_string(),
                enabled: true,
                state: ProjectMcpServerState::Healthy,
                summary: "ok".to_string(),
                error: None,
                tool_count: 1,
                tools: vec![],
            },
        ];

        let (aggregate, enabled_server_count, healthy_server_count) =
            aggregate_server_statuses(&statuses);

        assert!(matches!(aggregate, ProjectMcpAggregateStatus::Healthy));
        assert_eq!(enabled_server_count, 1);
        assert_eq!(healthy_server_count, 1);
    }

    #[test]
    fn aggregate_marks_all_disabled_as_not_configured() {
        let statuses = vec![ProjectMcpServerStatus {
            name: "gitnexus".to_string(),
            transport: "stdio".to_string(),
            enabled: false,
            state: ProjectMcpServerState::Disabled,
            summary: "已禁用".to_string(),
            error: None,
            tool_count: 0,
            tools: vec![],
        }];

        let (aggregate, enabled_server_count, healthy_server_count) =
            aggregate_server_statuses(&statuses);

        assert!(matches!(
            aggregate,
            ProjectMcpAggregateStatus::NotConfigured
        ));
        assert_eq!(enabled_server_count, 0);
        assert_eq!(healthy_server_count, 0);
    }
}
