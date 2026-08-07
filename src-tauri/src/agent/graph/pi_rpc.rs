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
    let mut guard = SidecarChildGuard::spawn()?;
    let response: Result<Value> = async {
        let mut stdin = guard
            .child_mut()
            .stdin
            .take()
            .context("PI sidecar stdin 未捕获")?;
        let stdout = guard
            .child_mut()
            .stdout
            .take()
            .context("PI sidecar stdout 未捕获")?;
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
    guard.terminate().await;
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

/// 杀 sidecar 整个进程组（同步）：coding 节点的 bash 等工具是 sidecar 的子进程，
/// 继承同一进程组，只杀直接子进程会把它们留成孤儿。
/// 注意 PGID 复用竞态：整组退出后 PGID 数值可能被系统回收复用（新进程组以
/// 复用后的 PID 为组长），此时盲目 `kill(-pgid, SIGKILL)` 会误杀无关进程组。
/// 因此「直接子进程已确认退出」的调用方（守卫 Drop）必须先校验目标仍存在
/// 再组杀（Unix：`process_group_alive` 探测；Windows：持句柄线程，见 Drop）；
/// 子进程仍存活时其 PID 不会被复用，组杀无复用风险。
/// Windows 分支的 taskkill 是外部进程，这里以 spawn 发起后不等待——调用方
/// 持有存活的子进程句柄，PID 不会被复用，无需等待 taskkill 完成；async
/// 路径走 `kill_process_group_async`。
fn kill_process_group(child: &mut Child) {
    #[cfg(any(unix, windows))]
    if let Some(pid) = child.id() {
        // 子进程仍存活：其必然还在组内，组杀无 PGID 复用风险。
        kill_process_group_pid(pid);
        return;
    }
    // 已回收时 id() 为 None，start_kill 对已退出进程是安全 no-op；
    // 非 unix/windows 目标无进程组 API，同样回退到杀直接子进程。
    let _ = child.start_kill();
}

/// Windows：taskkill 按 PID 递归杀进程树（/T）并强制终止（/F）。
/// stdio 全部置 null：Drop 等路径上无人消费其输出，继承管道反而可能
/// 让 taskkill 因管道写满而阻塞。
#[cfg(windows)]
fn taskkill_command(pid: u32) -> std::process::Command {
    let mut command = std::process::Command::new("taskkill");
    command
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    command
}

/// 按 PID 杀进程组（同步）。sidecar 以自身 PID 作为 PGID，故 pid 即组 ID。
/// pid 由调用方在回收子进程之前捕获——子进程被 wait/try_wait 回收后
/// `Child::id()` 返回 None，但捕获的 PID 仍可用于组杀与存活探测。
/// 调用方必须保证目标仍存在（子进程存活，或已做过存活校验），
/// 否则存在误杀复用 PID 的风险（见 `kill_process_group` 注释）。
#[cfg(any(unix, windows))]
fn kill_process_group_pid(pid: u32) {
    #[cfg(unix)]
    if let Ok(process_group_id) = libc::pid_t::try_from(pid) {
        // SAFETY: sidecar 以自身 PID 作为独立进程组 ID；PID 已检查可表示为
        // libc::pid_t，取负后用于向该进程组发送 SIGKILL。
        let _ = unsafe { libc::kill(-process_group_id, libc::SIGKILL) };
    }
    #[cfg(windows)]
    let _ = taskkill_command(pid).spawn();
}

/// PID 所属进程组是否仍存在：信号 0 探测不发实际信号，仅检查可达性。
/// 返回 true 表示组内仍有进程（或无权限判断，保守视为存在）；ESRCH 表示
/// 整组已退出——此时 PGID 可能已被复用，组杀必须跳过。
#[cfg(unix)]
fn process_group_alive(pid: u32) -> bool {
    let Ok(process_group_id) = libc::pid_t::try_from(pid) else {
        return true;
    };
    // SAFETY: 信号 0 探测仅检查进程组存在性，不发送信号。
    let result = unsafe { libc::kill(-process_group_id, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

/// async 版组杀：Unix 分支只是发信号（非阻塞系统调用），直接复用同步实现；
/// Windows 分支改用 tokio::process 等待 taskkill，避免阻塞 tokio worker 线程。
#[cfg(not(windows))]
async fn kill_process_group_async(child: &mut Child) {
    kill_process_group(child);
}

#[cfg(windows)]
async fn kill_process_group_async(child: &mut Child) {
    if let Some(pid) = child.id() {
        let _ = tokio::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await;
    }
}

/// PI sidecar 进程守卫：Drop 时杀整个进程组。正常路径应调用 `terminate`
/// （杀进程组并 wait 回收）；panic 展开、tokio 任务 abort 等绕过显式清理的
/// 路径由 Drop 兜底，避免 sidecar 及其工具子进程泄漏为孤儿。
pub(crate) struct SidecarChildGuard {
    /// Option 仅为让 Drop 能把句柄移入 Windows 的持句柄线程（见 Drop 的
    /// windows 分支）；仅在被 terminate/Drop 取走期间为 None。
    child: Option<Child>,
    /// terminate 中 wait 已成功回收子进程时置位，Drop 据此跳过重复组杀。
    /// 显式状态优于 mem::forget 跳过 Drop 的隐式手法：新增字段时不易破坏不变式。
    reaped: bool,
}

impl SidecarChildGuard {
    pub(crate) fn spawn() -> Result<Self> {
        Ok(Self {
            child: Some(spawn_sidecar()?),
            reaped: false,
        })
    }

    pub(crate) fn child_mut(&mut self) -> &mut Child {
        self.child
            .as_mut()
            .expect("sidecar 子进程已被 terminate/Drop 取走")
    }

    /// 正常路径的显式收尾：杀进程组并回收子进程（防僵尸）。
    pub(crate) async fn terminate(mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        kill_process_group_async(&mut child).await;
        // wait 失败时进程可能尚未退出：不置位 reaped，放回子进程，
        // 交由 Drop 的兜底组杀收尾。
        match child.wait().await {
            Ok(_) => self.reaped = true, // 已确认回收，跳过 Drop 的重复组杀
            Err(_) => self.child = Some(child),
        }
    }
}

impl Drop for SidecarChildGuard {
    fn drop(&mut self) {
        if self.reaped {
            return;
        }
        let Some(mut child) = self.child.take() else {
            return;
        };
        // pid 必须在 try_wait 之前捕获：tokio 的 Child 在 wait/try_wait 成功
        // 回收后 id() 返回 None，此后将无法定位子进程所属的进程组——而「直接
        // 子进程已退出、孙进程仍占用 PGID 存活」恰恰是最需要兜底组杀的场景。
        let pid = child.id();
        if matches!(child.try_wait(), Ok(Some(_))) {
            // 直接子进程已确认退出时，PID/PGID 存在被复用的竞态，组杀前必须
            // 确认目标仍存在——两个平台的保护是对称的：
            #[cfg(unix)]
            // 信号 0 探测；ESRCH 即跳过（组内已无进程可杀，孤儿孙进程若仍在
            // 运行则占用着该 PGID，探测为存在，组杀照常进行）。
            if !pid.is_some_and(process_group_alive) {
                return;
            }
            #[cfg(windows)]
            if let Some(pid) = pid {
                // taskkill 是独立外部进程：若 fire-and-forget，本函数返回后
                // Child 句柄随 Drop 关闭，已退出子进程的 PID 可能被复用，
                // taskkill 的 OpenProcess 一旦晚于复用就会按 PID 递归误杀无关
                // 进程树。把句柄移入分离线程：句柄存活期间进程对象不销毁、
                // PID 不会被复用；线程内同步等待 taskkill 完成再释放句柄，
                // 闭合竞态窗口。Drop 自身不能阻塞（可能在 tokio worker 线程
                // 上），故由线程代劳等待。
                match std::thread::Builder::new()
                    .spawn(move || kill_process_group_pid_hold_handle(pid, child))
                {
                    Ok(_) => return,
                    // 建线程失败（闭包连同 child 被丢弃；kill_on_drop 对已退出
                    // 进程是 no-op）：退化为 fire-and-forget，同旧实现。
                    Err(_) => kill_process_group_pid(pid),
                }
                return;
            }
            // pid 为 None 却已确认回收：理论上不可达（try_wait 之前 id() 必为
            // Some）；即便到达也无 PID 可用于组杀。
            return;
        }
        #[cfg(any(unix, windows))]
        if let Some(pid) = pid {
            // 子进程仍存活：存活期间 PID 不会被复用，taskkill 先 OpenProcess
            // 再杀、必然命中原进程，组杀无复用风险。
            kill_process_group_pid(pid);
        } else {
            let _ = child.start_kill();
        }
        #[cfg(not(any(unix, windows)))]
        let _ = child.start_kill();
        // tokio 的 Child（kill_on_drop=true）注册在运行时 orphan queue 异步回收，
        // 这里只做一次非阻塞 try_wait——守卫可能在 tokio worker 线程上被 drop
        // （terminate 的兜底路径、async 块 panic 展开），睡眠轮询会阻塞调度。
        let _ = child.try_wait();
    }
}

/// Windows Drop 兜底（直接子进程已退出分支）：持有子进程句柄并同步等待
/// taskkill 完成。句柄存活保证进程对象不销毁、PID 不被复用，taskkill 不会
/// 命中被复用的 PID；taskkill 结束后才释放句柄。与 Unix 分支的
/// `process_group_alive` 探测等价。
#[cfg(windows)]
fn kill_process_group_pid_hold_handle(pid: u32, child: Child) {
    let _ = taskkill_command(pid).status();
    drop(child); // 等 taskkill 结束再释放句柄（显式化释放时机）
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
