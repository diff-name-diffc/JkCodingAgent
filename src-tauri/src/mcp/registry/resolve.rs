//! 原始服务器/工具配置 → 运行时形态的解析。
//!
//! 只处理静态配置的形状校验与归一化（不发起连接）：
//! 传输类型推断与别名归一、字段完整性、cwd 作用域规则、工具规范名与去重。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rmcp::model::{TaskSupport, Tool};
use serde_json::Value;

use crate::mcp::transport::build_header_map;
use crate::mcp::{
    McpServerConfig, McpToolTaskSupport, ResolvedMcpServerConfig, ResolvedMcpTool,
    ResolvedMcpTransport,
};

const DEFAULT_MCP_STARTUP_TIMEOUT: Duration = Duration::from_secs(30);

/// 解析单个服务器配置。`cwd_base` 为相对 cwd 的解析基准：
/// 项目作用域传项目根；全局作用域传 `None`，相对 cwd 判为无效配置。
pub(super) fn resolve_server_config(
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
pub(super) fn resolve_cwd(
    cwd_base: Option<&Path>,
    server_name: &str,
    raw: &str,
) -> Result<PathBuf, String> {
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

pub(super) fn resolve_transport_kind(config: &McpServerConfig) -> Result<String, String> {
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

pub(super) fn normalize_transport_name(value: &str) -> String {
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

pub(super) fn resolve_mcp_tool(
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

pub(super) fn sanitize_tool_name(raw: &str) -> String {
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
