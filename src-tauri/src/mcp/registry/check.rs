//! 服务器连通性检查与工具清单拉取（刷新管线的执行段）。
//!
//! 对每个已解析的服务器配置发起真实连接：列举工具、归类状态，
//! 并汇总为作用域级 `McpStatus`。配置解析见 `resolve`，快照缓存见 `registry`。

use std::collections::HashMap;
use std::path::Path;

use rmcp::model::Tool;
use rmcp::ServiceExt;

use super::resolve::{resolve_mcp_tool, resolve_server_config, resolve_transport_kind};
use crate::mcp::project_file::server_enabled;
use crate::mcp::transport::{
    build_stdio_timeout_error, build_streamable_http_transport, build_timeout_error,
    build_unix_socket_transport, collect_captured_stderr, enrich_stdio_error,
    spawn_stdio_mcp_process, timeout_server_check, SpawnedStdioMcpProcess,
};
use crate::mcp::{
    McpAggregateStatus, McpScope, McpServerConfig, McpServerState, McpServerStatus, McpStatus,
    McpToolStatus, ResolvedMcpServerConfig, ResolvedMcpTool, ResolvedMcpTransport,
};

#[derive(Debug)]
pub(super) struct CheckedServer {
    pub status: McpServerStatus,
    pub resolved_config: Option<ResolvedMcpServerConfig>,
    pub resolved_tools: Vec<ResolvedMcpTool>,
}

pub(super) async fn check_server(
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
                let client = ()
                    .serve(transport)
                    .await
                    .map_err(|error| (McpServerState::ConnectionFailed, error.to_string()))?;
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
                    let client = ()
                        .serve(transport)
                        .await
                        .map_err(|error| (McpServerState::ConnectionFailed, error.to_string()))?;
                    let result = client
                        .list_all_tools()
                        .await
                        .map_err(|error| (McpServerState::ConnectionFailed, error.to_string()));
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
                    let client = ()
                        .serve(transport)
                        .await
                        .map_err(|error| (McpServerState::ConnectionFailed, error.to_string()))?;
                    let result = client
                        .list_all_tools()
                        .await
                        .map_err(|error| (McpServerState::ConnectionFailed, error.to_string()));
                    let _ = client.cancel().await;
                    result
                },
                build_timeout_error("MCP 连接或握手", server.startup_timeout),
            )
            .await
        }
    }
}

pub(super) fn build_status(
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

pub(super) fn aggregate_server_statuses(
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
