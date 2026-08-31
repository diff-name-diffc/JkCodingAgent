//! MCP 注册表：按作用域缓存「全局 ∪ 项目」合并后的服务器状态与工具快照。
//!
//! 作用域见 [`super::McpScope`]：`Global` 只合并全局注册表（所有聊天共享），
//! `Project` 再并入项目级 `mcp.json`（同名项目覆盖全局）。
//!
//! 本文件保留缓存所有权、刷新编排与快照上的工具执行；子模块按变化原因划分：
//! - `config`：作用域配置加载/合并与快照新鲜度；
//! - `resolve`：静态配置解析（传输推断、字段校验、cwd 规则、工具规范名）；
//! - `check`：连通性检查、工具清单拉取与作用域状态汇总；
//! - `tests`：纯函数单测。

mod check;
mod config;
mod resolve;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::RwLock;
use rmcp::model::{CallToolRequestParams, JsonObject};
use rmcp::ServiceExt;
use serde_json::Value;

use check::{aggregate_server_statuses, build_status, check_server};
use config::{is_fresh, load_merged_config};

use crate::mcp::transport::{
    build_stdio_timeout_error, build_streamable_http_transport, build_timeout_error,
    build_unix_socket_transport, collect_captured_stderr, enrich_stdio_error,
    spawn_stdio_mcp_process, timeout_tool_call, SpawnedStdioMcpProcess,
};
use crate::mcp::{
    McpAggregateStatus, McpScope, McpServerState, McpSnapshot, McpToolTaskSupport,
    ResolvedMcpTransport,
};

const MCP_REFRESH_MAX_AGE: Duration = Duration::from_secs(300);

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
