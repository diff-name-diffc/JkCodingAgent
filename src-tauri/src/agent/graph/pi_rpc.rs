//! PI SDK sidecar 的严格 JSONL 传输。协议 stdout 与诊断 stderr 完全隔离。

use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};

const SIDECAR_NAME: &str = "pi-agent-sidecar";
const PROTOCOL_VERSION: i64 = 2;

#[derive(Debug, Clone)]
pub(crate) struct PiExtensionTool {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SidecarEnvelope {
    pub r#type: String,
    pub request_id: String,
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub node_id: Option<String>,
    pub sequence: i64,
    #[serde(default)]
    pub data: Value,
}

pub(crate) async fn discover_extension_tools(
    workspace: &Path,
) -> Result<(Vec<PiExtensionTool>, Vec<String>)> {
    let mut child = spawn_sidecar()?;
    let response: Result<Value> = async {
        let mut stdin = child.stdin.take().context("PI sidecar stdin 未捕获")?;
        let stdout = child.stdout.take().context("PI sidecar stdout 未捕获")?;
        let request_id = uuid::Uuid::new_v4().to_string();
        let agent_dir = global_agent_dir()?;
        let project_resource_dir = workspace.join(".jkcodingagent/pi-agent");
        let request = json!({
            "type": "discover",
            "requestId": request_id,
            "runId": "catalog",
            "nodeId": "catalog",
            "sequence": 1,
            "workspace": workspace,
            "agentDir": agent_dir,
            "projectResourceDir": project_resource_dir,
        });
        stdin.write_all(format!("{}\n", request).as_bytes()).await?;
        stdin.flush().await?;
        let mut lines = BufReader::new(stdout).lines();
        tokio::time::timeout(std::time::Duration::from_secs(20), async {
            while let Some(line) = lines.next_line().await.context("读取 PI catalog 响应")? {
                let envelope: SidecarEnvelope = serde_json::from_str(&line)
                    .with_context(|| format!("PI sidecar 输出非法 JSONL：{line}"))?;
                if envelope.r#type == "catalog" && envelope.request_id == request_id {
                    return Ok(envelope.data);
                }
                if envelope.r#type == "failed" && envelope.request_id == request_id {
                    return Err(anyhow!(
                        "{}",
                        envelope
                            .data
                            .get("error")
                            .and_then(Value::as_str)
                            .unwrap_or("PI catalog 失败")
                    ));
                }
            }
            Err(anyhow!("PI sidecar 在 catalog 响应前退出"))
        })
        .await
        .context("PI catalog 发现超时")?
    }
    .await;
    // spawn 后所有结果都经过统一清理，避免协议错误或超时遗留孙进程。
    terminate_sidecar_process_group(&mut child).await;
    let response = response?;
    let version = response
        .get("protocolVersion")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    if version != PROTOCOL_VERSION {
        return Err(anyhow!(
            "PI sidecar 协议版本不匹配：期望 {PROTOCOL_VERSION}，实际 {version}"
        ));
    }
    let tools = response
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|tool| {
            Some(PiExtensionTool {
                name: tool.get("name")?.as_str()?.to_string(),
                description: tool
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            })
        })
        .collect();
    let diagnostics = response
        .get("diagnostics")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    Ok((tools, diagnostics))
}

pub(crate) fn spawn_sidecar() -> Result<Child> {
    let path = resolve_sidecar_path()?;
    let mut command = Command::new(&path);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command
        .spawn()
        .with_context(|| format!("启动 PI sidecar 失败：{}", path.display()))?;
    if let Some(mut stderr) = child.stderr.take() {
        tokio::spawn(async move {
            let mut buffer = [0_u8; 4096];
            loop {
                match stderr.read(&mut buffer).await {
                    Ok(0) => break,
                    Ok(read) => eprintln!(
                        "[pi-sidecar] {}",
                        String::from_utf8_lossy(&buffer[..read]).trim_end()
                    ),
                    Err(error) => {
                        eprintln!("[pi-sidecar] 读取 stderr 失败：{error}");
                        break;
                    }
                }
            }
        });
    }
    Ok(child)
}

pub(crate) async fn terminate_sidecar_process_group(child: &mut Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        if let Ok(process_group_id) = libc::pid_t::try_from(pid) {
            // SAFETY: sidecar 以自身 PID 作为独立进程组 ID；PID 已检查可表示为
            // libc::pid_t，取负后用于向该进程组发送 SIGKILL。
            let _ = unsafe { libc::kill(-process_group_id, libc::SIGKILL) };
        } else {
            let _ = child.start_kill();
        }
    }
    #[cfg(windows)]
    if let Some(pid) = child.id() {
        let pid = pid.to_string();
        let _ = Command::new("taskkill")
            .args(["/PID", pid.as_str(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
    }
    #[cfg(all(not(unix), not(windows)))]
    let _ = child.start_kill();
    let _ = child.wait().await;
}

pub(crate) fn global_agent_dir() -> Result<PathBuf> {
    dirs::home_dir()
        .map(|home| home.join(".jkcodingagent/pi-agent"))
        .ok_or_else(|| anyhow!("无法解析用户主目录"))
}

fn resolve_sidecar_path() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("获取当前可执行文件路径失败")?;
    let exe_dir = exe.parent().context("当前可执行文件无父目录")?;
    let base = if exe_dir.ends_with("deps") {
        exe_dir.parent().unwrap_or(exe_dir)
    } else {
        exe_dir
    };
    let bundled = sidecar_candidate(base.join(SIDECAR_NAME));
    if bundled.exists() {
        return Ok(bundled);
    }
    #[cfg(debug_assertions)]
    {
        let triple = platform_triple();
        let development = sidecar_candidate(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("binaries")
                .join(format!("{SIDECAR_NAME}-{triple}")),
        );
        if development.exists() {
            return Ok(development);
        }
        Err(anyhow!(
            "未找到开发版 PI sidecar：{}。请先运行 pnpm pi-sidecar:build",
            development.display()
        ))
    }
    #[cfg(not(debug_assertions))]
    {
        Err(anyhow!(
            "未找到随应用安装的 PI sidecar：{}。应用安装可能不完整，请重新安装",
            bundled.display()
        ))
    }
}

fn sidecar_candidate(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let mut path = path;
        path.set_extension("exe");
        path
    }
    #[cfg(not(windows))]
    {
        path
    }
}

#[cfg(debug_assertions)]
fn platform_triple() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("windows", "x86_64") => "x86_64-pc-windows-msvc",
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        _ => "unsupported-target",
    }
}
