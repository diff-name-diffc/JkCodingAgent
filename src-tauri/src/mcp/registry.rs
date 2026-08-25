//! MCP 注册表：按作用域缓存「全局 ∪ 项目」合并后的服务器状态与工具快照。
//!
//! 作用域见 [`super::McpScope`]：`Global` 只合并全局注册表（所有聊天共享），
//! `Project` 再并入项目级 `mcp.json`（同名项目覆盖全局）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::RwLock;
use rmcp::model::{CallToolRequestParams, JsonObject, TaskSupport, Tool};
use rmcp::ServiceExt;
use serde_json::Value;

use super::project_file::{read_project_mcp_config_sync, server_enabled};
use super::transport::{
    build_header_map, build_stdio_timeout_error, build_streamable_http_transport,
    build_timeout_error, build_unix_socket_transport, collect_captured_stderr, enrich_stdio_error,
    spawn_stdio_mcp_process, timeout_server_check, timeout_tool_call, SpawnedStdioMcpProcess,
};
use super::{
    McpAggregateStatus, McpConfig, McpScope, McpServerConfig, McpServerState, McpServerStatus,
    McpSnapshot, McpStatus, McpToolStatus, McpToolTaskSupport, ResolvedMcpServerConfig,
    ResolvedMcpTool, ResolvedMcpTransport,
};

const MCP_REFRESH_MAX_AGE: Duration = Duration::from_secs(300);
const DEFAULT_MCP_STARTUP_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct McpRegistry {
    cache: Arc<RwLock<HashMap<McpScope, McpSnapshot>>>,
    db: crate::agent::db::DispatcherDb,
}

impl McpRegistry {
    /// DB 是全局注册表的唯一权威源，构造期强制持有（大声失败：
    /// 没有 DB 就不存在 MCP 注册表，而非运行期静默跳过全局合并）。
    pub fn new(db: crate::agent::db::DispatcherDb) -> Self {
        Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
            db,
        }
    }

    /// 清空全部作用域缓存（全局注册表变更影响全局与所有项目的合并结果）。
    pub fn invalidate_all(&self) {
        self.cache.write().clear();
    }

    pub(crate) fn db(&self) -> &crate::agent::db::DispatcherDb {
        &self.db
    }

    pub async fn ensure_recent(&self, scope: &McpScope) -> Result<McpSnapshot, String> {
        if let Some(snapshot) = self.cached_for_scope(scope) {
            if is_fresh(
                snapshot.status.checked_at,
                current_timestamp_millis(),
                MCP_REFRESH_MAX_AGE,
            ) {
                return Ok(snapshot);
            }
        }

        self.refresh(scope).await
    }

    pub fn cached_for_scope(&self, scope: &McpScope) -> Option<McpSnapshot> {
        self.cache.read().get(scope).cloned()
    }

    pub async fn refresh(&self, scope: &McpScope) -> Result<McpSnapshot, String> {
        let db = self.db.clone();
        let scope_for_load = scope.clone();
        let loaded = tokio::task::spawn_blocking(move || load_merged_config(&db, &scope_for_load))
            .await
            .map_err(|error| error.to_string())??;

        let checked_at = current_timestamp_millis();
        let snapshot = match loaded.config {
            Ok(config) => {
                let mut statuses = Vec::new();
                let mut resolved_tools = HashMap::new();
                let mut resolved_servers = HashMap::new();

                // 相对 cwd 的解析基准：项目作用域挂在项目根下；全局作用域
                // 没有项目语境，相对路径在 resolve 阶段直接判为无效配置。
                let cwd_base = match scope {
                    McpScope::Global => None,
                    McpScope::Project(path) => Some(path.as_path()),
                };

                for (server_name, server_config) in config.servers {
                    let checked = check_server(cwd_base, &server_name, server_config).await;
                    if let Some(resolved_config) = checked.resolved_config {
                        resolved_servers.insert(server_name.clone(), resolved_config);
                    }
                    for tool in checked.resolved_tools {
                        resolved_tools.insert(tool.canonical_name.clone(), tool);
                    }
                    statuses.push(checked.status);
                }

                statuses.sort_by(|left, right| left.name.cmp(&right.name));
                // 启用/健康计数由 build_status 从最终服务器列表统计，
                // 这里只取聚合态。
                let (aggregate, _, _) = aggregate_server_statuses(&statuses);

                McpSnapshot::new(
                    build_status(
                        scope,
                        loaded.project_path.as_deref(),
                        loaded.config_path.as_deref(),
                        aggregate,
                        checked_at,
                        statuses,
                        None,
                    ),
                    resolved_tools,
                    resolved_servers,
                )
            }
            Err(config_error) => McpSnapshot::new(
                build_status(
                    scope,
                    loaded.project_path.as_deref(),
                    loaded.config_path.as_deref(),
                    McpAggregateStatus::InvalidConfig,
                    checked_at,
                    Vec::new(),
                    Some(config_error),
                ),
                HashMap::new(),
                HashMap::new(),
            ),
        };

        self.cache.write().insert(scope.clone(), snapshot.clone());
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
}

/// 加载作用域对应的合并配置。
///
/// - `Global`：只读全局注册表，完全不触碰文件系统；
/// - `Project`：全局 ∪ 项目级 `mcp.json`（缺失视为空），同名项目覆盖全局。
fn load_merged_config(
    db: &crate::agent::db::DispatcherDb,
    scope: &McpScope,
) -> Result<LoadedScopeConfig, String> {
    let global = db
        .get_global_mcp_config()
        .map_err(|error| error.to_string())?;
    match scope {
        McpScope::Global => Ok(LoadedScopeConfig {
            project_path: None,
            config_path: None,
            config: Ok(global),
        }),
        McpScope::Project(path) => {
            let loaded = read_project_mcp_config_sync(path)?;
            let config = loaded.config.map(|project| merge_configs(global, project));
            Ok(LoadedScopeConfig {
                project_path: Some(path.to_path_buf()),
                config_path: Some(loaded.config_path),
                config,
            })
        }
    }
}

/// 合并全局与项目配置：项目同名服务器整体覆盖全局条目（不做字段级合并）。
pub fn merge_configs(global: McpConfig, project: McpConfig) -> McpConfig {
    let mut servers = global.servers;
    servers.extend(project.servers);
    McpConfig { servers }
}

/// 快照新鲜度判定（checked_at 与 now 均为毫秒时间戳）。
pub fn is_fresh(checked_at_ms: i64, now_ms: i64, max_age: Duration) -> bool {
    now_ms.saturating_sub(checked_at_ms) <= max_age.as_millis() as i64
}

struct LoadedScopeConfig {
    project_path: Option<PathBuf>,
    config_path: Option<PathBuf>,
    config: Result<McpConfig, String>,
}

fn build_status(
    scope: &McpScope,
    project_path: Option<&Path>,
    config_path: Option<&Path>,
    aggregate: McpAggregateStatus,
    checked_at: i64,
    servers: Vec<McpServerStatus>,
    config_error: Option<String>,
) -> McpStatus {
    let enabled_server_count = servers.iter().filter(|server| server.enabled).count();
    let healthy_server_count = servers
        .iter()
        .filter(|server| server.enabled && matches!(server.state, McpServerState::Healthy))
        .count();
    McpStatus {
        scope: scope.kind(),
        project_path: project_path.map(|path| path.to_string_lossy().into_owned()),
        config_path: config_path.map(|path| path.to_string_lossy().into_owned()),
        aggregate,
        checked_at,
        server_count: servers.len(),
        enabled_server_count,
        healthy_server_count,
        servers,
        config_error,
    }
}

#[derive(Debug)]
struct CheckedServer {
    status: McpServerStatus,
    resolved_config: Option<ResolvedMcpServerConfig>,
    resolved_tools: Vec<ResolvedMcpTool>,
}

async fn check_server(
    cwd_base: Option<&Path>,
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

    let resolved = match resolve_server_config(cwd_base, server_name, server_config) {
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

/// 解析单个服务器配置。`cwd_base` 为相对 cwd 的解析基准：
/// 项目作用域传项目根；全局作用域传 `None`，相对 cwd 判为无效配置。
fn resolve_server_config(
    cwd_base: Option<&Path>,
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
            let cwd = match config.cwd.as_deref() {
                Some(raw) => Some(resolve_cwd(cwd_base, server_name, raw)?),
                None => None,
            };
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

/// 解析服务器 `cwd`：绝对路径原样使用；相对路径仅项目作用域可解析
/// （挂在项目根下），全局作用域没有项目语境，大声失败要求绝对路径。
fn resolve_cwd(cwd_base: Option<&Path>, server_name: &str, raw: &str) -> Result<PathBuf, String> {
    let candidate = PathBuf::from(raw);
    if candidate.is_absolute() {
        return Ok(candidate);
    }
    match cwd_base {
        Some(base) => Ok(base.join(candidate)),
        None => Err(format!(
            "全局 MCP server '{server_name}' 的 cwd 必须使用绝对路径（相对路径只在项目级 mcp.json 中可用）"
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

fn current_timestamp_millis() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(command: &str) -> McpServerConfig {
        McpServerConfig {
            command: Some(command.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn merge_configs_prefers_project_entries_on_name_collision() {
        let mut global = McpConfig::default();
        global.servers.insert("shared".to_string(), server("global-bin"));
        global.servers.insert("global-only".to_string(), server("g"));
        let mut project = McpConfig::default();
        project.servers.insert("shared".to_string(), server("project-bin"));
        project
            .servers
            .insert("project-only".to_string(), server("p"));

        let merged = merge_configs(global, project);
        assert_eq!(merged.servers.len(), 3);
        assert_eq!(
            merged.servers["shared"].command.as_deref(),
            Some("project-bin")
        );
        assert!(merged.servers.contains_key("global-only"));
        assert!(merged.servers.contains_key("project-only"));
    }

    #[test]
    fn merge_configs_handles_empty_sides() {
        let empty = McpConfig::default();
        let mut one = McpConfig::default();
        one.servers.insert("a".to_string(), server("a"));

        assert!(merge_configs(empty.clone(), McpConfig::default())
            .servers
            .is_empty());
        assert_eq!(merge_configs(empty.clone(), one.clone()).servers.len(), 1);
        assert_eq!(merge_configs(one, empty).servers.len(), 1);
    }

    #[test]
    fn is_fresh_boundary_is_inclusive_at_max_age() {
        let max_age = Duration::from_secs(300);
        let now = 1_000_000_000_000;
        assert!(is_fresh(now - 300_000, now, max_age));
        assert!(!is_fresh(now - 300_001, now, max_age));
        assert!(is_fresh(now, now, max_age));
    }

    #[test]
    fn resolve_cwd_requires_absolute_paths_in_global_scope() {
        let base = PathBuf::from("/tmp/project");
        assert_eq!(
            resolve_cwd(Some(&base), "s", "sub/dir").unwrap(),
            base.join("sub/dir")
        );
        assert_eq!(
            resolve_cwd(Some(&base), "s", "/abs/path").unwrap(),
            PathBuf::from("/abs/path")
        );
        assert_eq!(
            resolve_cwd(None, "s", "/abs/path").unwrap(),
            PathBuf::from("/abs/path")
        );
        let error = resolve_cwd(None, "fetch", "relative").unwrap_err();
        assert!(error.contains("fetch"));
        assert!(error.contains("绝对路径"));
    }

    #[test]
    fn resolve_server_config_rejects_invalid_shapes() {
        let base = Path::new("/tmp/project");
        let missing_command = McpServerConfig::default();
        assert!(resolve_server_config(Some(base), "a", missing_command).is_err());

        let zero_timeout = McpServerConfig {
            command: Some("node".to_string()),
            startup_timeout_seconds: Some(0),
            ..Default::default()
        };
        let error = resolve_server_config(Some(base), "a", zero_timeout).unwrap_err();
        assert!(error.contains("startupTimeoutSeconds"));

        let unknown_transport = McpServerConfig {
            transport: Some("carrier-pigeon".to_string()),
            command: Some("node".to_string()),
            ..Default::default()
        };
        assert!(resolve_server_config(Some(base), "a", unknown_transport).is_err());
    }

    #[test]
    fn transport_kind_inference_and_aliases() {
        assert_eq!(
            resolve_transport_kind(&server("node")).unwrap(),
            "stdio"
        );
        assert_eq!(
            resolve_transport_kind(&McpServerConfig {
                url: Some("http://x".to_string()),
                ..Default::default()
            })
            .unwrap(),
            "streamable_http"
        );
        assert_eq!(
            normalize_transport_name("Streamable-HTTP"),
            "streamable_http"
        );
        assert_eq!(normalize_transport_name("http"), "streamable_http");
        assert_eq!(normalize_transport_name("stdio"), "stdio");
    }

    #[test]
    fn aggregate_statuses_count_enabled_and_healthy() {
        let healthy = McpServerStatus {
            name: "a".to_string(),
            transport: "stdio".to_string(),
            enabled: true,
            state: McpServerState::Healthy,
            summary: String::new(),
            error: None,
            tool_count: 0,
            tools: vec![],
        };
        let disabled = McpServerStatus {
            enabled: false,
            name: "b".to_string(),
            ..healthy.clone()
        };
        let failed = McpServerStatus {
            name: "c".to_string(),
            state: McpServerState::ConnectionFailed,
            ..healthy.clone()
        };

        let (aggregate, _, _) = aggregate_server_statuses(&[]);
        assert!(matches!(aggregate, McpAggregateStatus::NotConfigured));
        let (aggregate, enabled, healthy_count) =
            aggregate_server_statuses(&[healthy.clone(), disabled]);
        assert!(matches!(aggregate, McpAggregateStatus::Healthy));
        assert_eq!((enabled, healthy_count), (1, 1));
        let (aggregate, enabled, healthy_count) = aggregate_server_statuses(&[healthy, failed]);
        assert!(matches!(aggregate, McpAggregateStatus::Degraded));
        assert_eq!((enabled, healthy_count), (2, 1));
    }

    #[test]
    fn tool_names_are_sanitized_and_deduplicated() {
        assert_eq!(sanitize_tool_name("My-Server.1"), "my_server_1");
        assert_eq!(sanitize_tool_name("___"), "tool");

        let tool = |name: &str| {
            serde_json::from_value::<Tool>(serde_json::json!({
                "name": name,
                "inputSchema": { "type": "object" }
            }))
            .unwrap()
        };
        let mut used = HashMap::new();
        let first = resolve_mcp_tool("srv", tool("read"), &mut used);
        let second = resolve_mcp_tool("srv", tool("read"), &mut used);
        assert_eq!(first.canonical_name, "mcp__srv__read");
        assert_eq!(second.canonical_name, "mcp__srv__read__2");
    }
}
