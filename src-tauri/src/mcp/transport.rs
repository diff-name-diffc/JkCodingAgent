//! MCP 传输层：三种传输（stdio / streamable_http / unix_socket_http）的
//! 进程启动、连接构建、stderr 捕获与错误诊断。

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use reqwest::header::{HeaderName, HeaderValue};
use rmcp::transport::{
    streamable_http_client::StreamableHttpClientTransportConfig, ConfigureCommandExt,
    StreamableHttpClientTransport, TokioChildProcess,
};
use tokio::io::AsyncReadExt;

use super::{McpServerState, ResolvedMcpServerConfig, ResolvedMcpTransport};

pub(super) const STDERR_CAPTURE_LIMIT: usize = 8 * 1024;
pub(super) const STDERR_CAPTURE_SETTLE_TIMEOUT: Duration = Duration::from_millis(250);

pub(crate) struct SpawnedStdioMcpProcess {
    pub(crate) transport: TokioChildProcess,
    pub(crate) stderr_buffer: Arc<Mutex<String>>,
    pub(crate) stderr_task: Option<tokio::task::JoinHandle<()>>,
}

pub(crate) fn spawn_stdio_mcp_process(
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

pub(crate) fn build_streamable_http_transport(
    url: &str,
    headers: &HashMap<HeaderName, HeaderValue>,
) -> Result<StreamableHttpClientTransport<reqwest::Client>, String> {
    let config = StreamableHttpClientTransportConfig::with_uri(url.to_string())
        .custom_headers(headers.clone());
    Ok(StreamableHttpClientTransport::from_config(config))
}

pub(crate) fn build_unix_socket_transport(
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

pub(crate) fn build_header_map(
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

pub(crate) async fn collect_captured_stderr(
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

pub(crate) fn enrich_stdio_error(
    server: &ResolvedMcpServerConfig,
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

pub(crate) fn build_timeout_error(operation: &str, timeout: Duration) -> String {
    format!("{operation}超时（{} 秒）", timeout.as_secs())
}

pub(crate) fn build_stdio_timeout_error(
    server: &ResolvedMcpServerConfig,
    operation: &str,
    stderr_output: Option<&str>,
) -> String {
    let mut message = build_timeout_error(&format!("MCP {operation}"), server.startup_timeout);
    if let ResolvedMcpTransport::Stdio { command, args, .. } = &server.transport {
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
    server: &ResolvedMcpServerConfig,
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
    server: &ResolvedMcpServerConfig,
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
    if let ResolvedMcpTransport::Stdio { command, args, .. } = &server.transport {
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

pub(crate) async fn timeout_server_check<F, T>(
    timeout: Duration,
    future: F,
    timeout_message: String,
) -> Result<T, (McpServerState, String)>
where
    F: std::future::Future<Output = Result<T, (McpServerState, String)>>,
{
    match tokio::time::timeout(timeout, future).await {
        Ok(result) => result,
        Err(_) => Err((McpServerState::ConnectionFailed, timeout_message)),
    }
}

pub(crate) async fn timeout_tool_call<F, T>(
    timeout: Duration,
    future: F,
    timeout_message: String,
) -> Result<T, (McpServerState, String)>
where
    F: std::future::Future<Output = Result<T, String>>,
{
    match tokio::time::timeout(timeout, future).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err((McpServerState::ConnectionFailed, error)),
        Err(_) => Err((McpServerState::ConnectionFailed, timeout_message)),
    }
}
