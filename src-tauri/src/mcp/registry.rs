//! MCP 注册表：按工作区缓存「全局 ∪ 项目」合并后的服务器状态与工具快照。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::RwLock;
use rmcp::model::{CallToolRequestParams, JsonObject, TaskSupport, Tool};
use rmcp::ServiceExt;
use serde_json::Value;

use super::project_file::{read_project_mcp_config_sync, server_enabled, LoadedMcpConfig};
use super::transport::{
    build_header_map, build_stdio_timeout_error, build_streamable_http_transport,
    build_timeout_error, build_unix_socket_transport, collect_captured_stderr, enrich_stdio_error,
    spawn_stdio_mcp_process, timeout_server_check, timeout_tool_call, SpawnedStdioMcpProcess,
};
use super::{
    McpAggregateStatus, McpConfig, McpServerConfig, McpServerState, McpServerStatus, McpSnapshot,
    McpStatus, McpToolStatus, McpToolTaskSupport, ResolvedMcpServerConfig, ResolvedMcpTool,
    ResolvedMcpTransport,
};

const MCP_REFRESH_MAX_AGE: Duration = Duration::from_secs(300);
const DEFAULT_MCP_STARTUP_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Default)]
pub struct McpRegistry {
    cache: Arc<RwLock<HashMap<String, McpSnapshot>>>,
    /// 全局 MCP 注册表的读取入口。注册表在 DispatcherState 之前构造
    /// （app 启动顺序），DB 打开后通过 attach_db 注入；Clone 共享同一槽位。
    db: Arc<RwLock<Option<crate::agent::db::DispatcherDb>>>,
}

impl McpRegistry {
    /// 注入全局 DB（DispatcherState::new 打开数据库后调用一次）。
    pub fn attach_db(&self, db: crate::agent::db::DispatcherDb) {
        *self.db.write() = Some(db);
    }

    pub(crate) fn attached_db(&self) -> Option<crate::agent::db::DispatcherDb> {
        self.db.read().clone()
    }

    /// 清空全部工作区缓存（全局注册表变更影响所有工作区）。
    pub fn invalidate_all(&self) {
        self.cache.write().clear();
    }

    pub async fn ensure_recent(&self, project_path: &Path) -> Result<McpSnapshot, String> {
        let key = workspace_cache_key(project_path);
        if let Some(snapshot) = self.cached(&key) {
            let age_ms = current_timestamp_millis().saturating_sub(snapshot.status.checked_at);
            if age_ms <= MCP_REFRESH_MAX_AGE.as_millis() as i64 {
                return Ok(snapshot);
            }
        }

        self.refresh(project_path.to_string_lossy().as_ref()).await
    }

    pub fn cached_for_workspace(&self, workspace: &Path) -> Option<McpSnapshot> {
        self.cached(&workspace_cache_key(workspace))
    }

    pub async fn refresh(&self, project_path: &str) -> Result<McpSnapshot, String> {
        let project_path = PathBuf::from(project_path);
        let cache_key = workspace_cache_key(&project_path);
        let db = self.attached_db();
        let loaded = tokio::task::spawn_blocking({
            let project_path = project_path.clone();
            move || -> Result<LoadedMcpConfig, String> {
                let mut loaded = read_project_mcp_config_sync(&project_path)?;
                // 全局注册表并入工作区配置：同名时项目级覆盖全局。
                if let Some(db) = db {
                    let global = db
                        .get_global_mcp_config()
                        .map_err(|error| error.to_string())?;
                    let workspace = loaded.config?;
                    let mut servers = global.servers;
                    servers.extend(workspace.servers);
                    loaded.config = Ok(McpConfig { servers });
                }
                Ok(loaded)
            }
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
                    let checked = check_server(&project_path, &server_name, server_config).await;
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

                McpSnapshot::new(
                    McpStatus {
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
                    resolved_tools,
                    resolved_servers,
                )
            }
            Err(config_error) => McpSnapshot::new(
                McpStatus {
                    project_path: project_path.to_string_lossy().into_owned(),
                    config_path: loaded.config_path.to_string_lossy().into_owned(),
                    aggregate: McpAggregateStatus::InvalidConfig,
                    checked_at,
                    server_count: 0,
                    enabled_server_count: 0,
                    healthy_server_count: 0,
                    servers: vec![],
                    config_error: Some(config_error),
                },
                HashMap::new(),
                HashMap::new(),
            ),
        };

        self.cache.write().insert(cache_key, snapshot.clone());
        Ok(snapshot)
    }

    /// 在调用方已经校验过的不可变目录快照上执行，避免再次刷新缓存后把
    /// 同名但 Schema/server 已变化的工具偷换进当前 invocation。
    pub(crate) async fn execute_tool_from_snapshot(
        &self,
        snapshot: &McpSnapshot,
        tool_name: &str,
        arguments: &Value,
    ) -> Result<String, String> {
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
        if matches!(tool.task_support, McpToolTaskSupport::Required) {
            call = call.with_task(JsonObject::new());
        }

        let result = match &server_config.transport {
            ResolvedMcpTransport::Stdio {
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
                        McpServerState::ConnectionFailed,
                        enrich_stdio_error(&server_config, error, stderr_output.as_deref()),
                    )),
                    Err(_) => Err((
                        McpServerState::ConnectionFailed,
                        build_stdio_timeout_error(
                            &server_config,
                            "工具调用",
                            stderr_output.as_deref(),
                        ),
                    )),
                }
            }
            ResolvedMcpTransport::StreamableHttp { url, headers } => {
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
            ResolvedMcpTransport::UnixSocketHttp {
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

    fn cached(&self, key: &str) -> Option<McpSnapshot> {
        self.cache.read().get(key).cloned()
    }
}

#[derive(Debug)]
struct CheckedServer {
    status: McpServerStatus,
    resolved_config: Option<ResolvedMcpServerConfig>,
    resolved_tools: Vec<ResolvedMcpTool>,
}

async fn check_server(
    project_path: &Path,
    server_name: &str,
    server_config: McpServerConfig,
) -> CheckedServer {
    if !server_enabled(&server_config) {
        return CheckedServer {
            status: McpServerStatus {
                name: server_name.to_string(),
                transport: resolve_transport_kind(&server_config)
                    .unwrap_or_else(|_| "unknown".to_string()),
                enabled: false,
                state: McpServerState::Disabled,
                summary: "已禁用".to_string(),
                error: None,
                tool_count: 0,
                tools: vec![],
            },
            resolved_config: None,
            resolved_tools: vec![],
        };
    }

    let resolved = match resolve_server_config(project_path, server_name, server_config) {
        Ok(resolved) => resolved,
        Err(error) => {
            return CheckedServer {
                status: McpServerStatus {
                    name: server_name.to_string(),
                    transport: "unknown".to_string(),
                    enabled: true,
                    state: McpServerState::InvalidConfig,
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
                .map(|tool| McpToolStatus {
                    name: tool.original_name.clone(),
                    exposed_name: tool.canonical_name.clone(),
                    description: tool.description.clone(),
                    task_support: tool.task_support.clone(),
                })
                .collect::<Vec<_>>();
            CheckedServer {
                status: McpServerStatus {
                    name: server_name.to_string(),
                    transport: resolved.transport_label.clone(),
                    enabled: true,
                    state: McpServerState::Healthy,
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
            status: McpServerStatus {
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
    server: &ResolvedMcpServerConfig,
) -> Result<Vec<Tool>, (McpServerState, String)> {
    match &server.transport {
        ResolvedMcpTransport::Stdio {
            command,
            args,
            env,
            cwd,
        } => {
            let spawned = spawn_stdio_mcp_process(command, args, env, cwd)
                .map_err(|error| (McpServerState::SpawnFailed, error))?;
            let SpawnedStdioMcpProcess {
                transport,
                stderr_buffer,
                stderr_task,
            } = spawned;

            let result = tokio::time::timeout(server.startup_timeout, async move {
                let client = ().serve(transport).await.map_err(|error| {
                    (McpServerState::ConnectionFailed, error.to_string())
                })?;
                let result = client
                    .list_all_tools()
                    .await
                    .map_err(|error| (McpServerState::ConnectionFailed, error.to_string()));
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
                    McpServerState::ConnectionFailed,
                    build_stdio_timeout_error(server, "连接或握手", stderr_output.as_deref()),
                )),
            }
        }
        ResolvedMcpTransport::StreamableHttp { url, headers } => {
            timeout_server_check(
                server.startup_timeout,
                async {
                    let transport = build_streamable_http_transport(url, headers)
                        .map_err(|error| (McpServerState::InvalidConfig, error))?;
                    let client = ().serve(transport).await.map_err(|error| {
                        (McpServerState::ConnectionFailed, error.to_string())
                    })?;
                    let result = client.list_all_tools().await.map_err(|error| {
                        (McpServerState::ConnectionFailed, error.to_string())
                    });
                    let _ = client.cancel().await;
                    result
                },
                build_timeout_error("MCP 连接或握手", server.startup_timeout),
            )
            .await
        }
        ResolvedMcpTransport::UnixSocketHttp {
            socket_path,
            url,
            headers,
        } => {
            timeout_server_check(
                server.startup_timeout,
                async {
                    let transport = build_unix_socket_transport(socket_path, url, headers)
                        .map_err(|error| (McpServerState::InvalidConfig, error))?;
                    let client = ().serve(transport).await.map_err(|error| {
                        (McpServerState::ConnectionFailed, error.to_string())
                    })?;
                    let result = client.list_all_tools().await.map_err(|error| {
                        (McpServerState::ConnectionFailed, error.to_string())
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

fn resolve_server_config(
    project_path: &Path,
    server_name: &str,
    config: McpServerConfig,
) -> Result<ResolvedMcpServerConfig, String> {
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
            Ok(ResolvedMcpServerConfig {
                transport_label: "stdio".to_string(),
                transport: ResolvedMcpTransport::Stdio {
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
            Ok(ResolvedMcpServerConfig {
                transport_label: "streamable_http".to_string(),
                transport: ResolvedMcpTransport::StreamableHttp {
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
            Ok(ResolvedMcpServerConfig {
                transport_label: "unix_socket_http".to_string(),
                transport: ResolvedMcpTransport::UnixSocketHttp {
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

fn resolve_transport_kind(config: &McpServerConfig) -> Result<String, String> {
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
            Some(TaskSupport::Optional) => McpToolTaskSupport::Optional,
            Some(TaskSupport::Required) => McpToolTaskSupport::Required,
            _ => McpToolTaskSupport::Forbidden,
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

fn aggregate_server_statuses(
    statuses: &[McpServerStatus],
) -> (McpAggregateStatus, usize, usize) {
    let enabled_server_count = statuses.iter().filter(|status| status.enabled).count();
    let healthy_server_count = statuses
        .iter()
        .filter(|status| status.enabled && matches!(status.state, McpServerState::Healthy))
        .count();
    let aggregate = if statuses.is_empty() || enabled_server_count == 0 {
        McpAggregateStatus::NotConfigured
    } else if healthy_server_count == enabled_server_count {
        McpAggregateStatus::Healthy
    } else {
        McpAggregateStatus::Degraded
    };
    (aggregate, enabled_server_count, healthy_server_count)
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
