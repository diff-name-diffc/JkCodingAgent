use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::sync::watch;
use tokio::time::{sleep, Duration};

use super::common::{string_arg, with_compression_parameters};
use crate::agent::tools::context::ToolContext;
use crate::agent::tools::registry::AgentTool;
use crate::agent::tools::ToolResult;

const MAX_OUTPUT_BYTES: usize = 128 * 1024;
const MAX_RESULT_CHARS: usize = 16_000;
const HISTORY_LIMIT: usize = 20;
const AUDIT_FILE_NAME: &str = "audit.json";

pub(super) fn local_zsh_tool() -> Box<dyn AgentTool> {
    Box::new(LocalZshTool)
}

struct LocalZshTool;

struct CapturedCommandOutput {
    output: Output,
    total_bytes_read: usize,
    timed_out: bool,
    cancelled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalZshAuditEntry {
    id: String,
    session_id: String,
    executed_at: String,
    command: String,
    #[serde(default)]
    review: Option<crate::ssh_tool::SshAuditReview>,
    exit_code: Option<i32>,
    timed_out: bool,
    #[serde(default)]
    cancelled: bool,
    duration_ms: u128,
    stdout: String,
    stderr: String,
    output_truncated: bool,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct LocalZshAuditLog {
    version: u32,
    entries: Vec<LocalZshAuditEntry>,
}

#[async_trait]
impl AgentTool for LocalZshTool {
    fn name(&self) -> &'static str {
        "local_zsh"
    }

    fn description(&self) -> &'static str {
        "在 macOS 本地 zsh 环境执行命令。命令执行前会经过安全审查（未配置审查模型时拒绝执行）。命令固定运行于当前会话工作区的 .jkcodingagent/local_env/zsh，产物也应写入该目录；工具会维护 audit.json，记录最新 20 条命令、结果和执行会话。禁止 cd 和少量高危系统命令。"
    }

    fn parameters(&self) -> Value {
        with_compression_parameters(
            json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "要在 /bin/zsh -lc 中执行的命令。不要使用 cd；当前目录固定为 .jkcodingagent/local_env/zsh。"
                    }
                },
                "required": ["command"]
            }),
            false,
            "local_zsh 会返回命令输出和审计历史摘要；默认保留原文，输出很长时系统仍会压缩。",
        )
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> ToolResult {
        let mut command_cancelled = false;
        let result = async {
            let Some(command) = string_arg(args, "command") else {
                return "错误：缺少必填参数 command".to_string();
            };
            let command = command.trim().to_string();
            if command.is_empty() {
                return "错误：command 不能为空".to_string();
            }
            if let Some(reason) = blacklist_reason(&command) {
                return format!("错误：local_zsh 已拦截命令：{reason}\n命令：{command}");
            }

            let timeout_secs = context.exec_timeout_secs.max(1);
            let workspace = context.workspace.clone();
            let session_id = context.workspace_id.clone();

            let run_result = tokio::task::spawn_blocking(move || {
                let run_dir = local_zsh_dir(&workspace)?;
                fs::create_dir_all(&run_dir)
                    .map_err(|error| format!("错误：创建 local_zsh 目录失败：{error}"))?;
                Ok::<PathBuf, String>(run_dir)
            })
            .await
            .map_err(|error| format!("错误：准备 local_zsh 目录失败：{error}"));

            let run_dir = match run_result {
                Ok(Ok(dir)) => dir,
                Ok(Err(error)) | Err(error) => return error,
            };

            // 安全审查门禁（fail-closed）：未配置审查 / 审查异常 / 判定不通过一律拒绝执行，
            // 并把「被拦截」写入 audit.json 审计。
            let review = match review_local_command(args, context, &run_dir, &command).await {
                Ok(review) => review,
                Err(error) => {
                    let blocked = crate::ssh_tool::SshAuditReview {
                        allowed: false,
                        reason: error.clone(),
                    };
                    return blocked_command_response(
                        &run_dir,
                        &session_id,
                        &command,
                        blocked,
                        format!("错误：{error}"),
                    )
                    .await;
                }
            };
            if !review.allowed {
                let headline = format!("错误：命令已被安全审查拦截：{}", review.reason);
                return blocked_command_response(&run_dir, &session_id, &command, review, headline)
                    .await;
            }

            let started = std::time::Instant::now();
            let mut cmd = Command::new("/bin/zsh");
            cmd.arg("-lc")
                .arg(&command)
                .current_dir(&run_dir)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            #[cfg(unix)]
            cmd.process_group(0); // 独立进程组：超时可按组终止全部派生进程
            let mut child = match cmd.spawn() {
                Ok(child) => child,
                Err(error) => return format!("错误：执行 zsh 命令失败：{error}"),
            };

            let captured =
                match capture_command_output(&mut child, timeout_secs, context.cancel_rx.clone())
                    .await
                {
                    Ok(output) => output,
                    Err(error) => return format!("错误：执行 zsh 命令失败：{error}"),
                };
            command_cancelled = captured.cancelled;
            let duration_ms = started.elapsed().as_millis();
            let stdout = String::from_utf8_lossy(&captured.output.stdout)
                .trim_end()
                .to_string();
            let stderr = String::from_utf8_lossy(&captured.output.stderr)
                .trim_end()
                .to_string();
            let retained_bytes = captured.output.stdout.len() + captured.output.stderr.len();
            let output_truncated = captured.total_bytes_read > retained_bytes;

            let entry = LocalZshAuditEntry {
                id: uuid::Uuid::new_v4().to_string(),
                session_id: session_id.clone(),
                executed_at: Utc::now().to_rfc3339(),
                command: command.clone(),
                review: Some(review.clone()),
                exit_code: captured.output.status.code(),
                timed_out: captured.timed_out,
                cancelled: captured.cancelled,
                duration_ms,
                stdout: stdout.clone(),
                stderr: stderr.clone(),
                output_truncated,
                error: None,
            };

            let run_dir_for_audit = run_dir.clone();
            let session_id_for_history = session_id.clone();
            let audit_result = tokio::task::spawn_blocking(move || {
                append_audit_entry(&run_dir_for_audit, entry, &session_id_for_history)
            })
            .await
            .map_err(|error| format!("错误：写入 local_zsh 审计历史失败：{error}"));

            let session_history = match audit_result {
                Ok(Ok(history)) => history,
                Ok(Err(error)) | Err(error) => {
                    return format!(
                        "错误：命令已执行，但审计历史写入失败：{error}\n\n{}",
                        render_command_result(
                            &run_dir,
                            &command,
                            &stdout,
                            &stderr,
                            captured.output.status.code(),
                            captured.timed_out,
                            captured.cancelled,
                            duration_ms,
                            output_truncated,
                            Some(&review),
                            false,
                            &[],
                        )
                    )
                }
            };

            render_command_result(
                &run_dir,
                &command,
                &stdout,
                &stderr,
                captured.output.status.code(),
                captured.timed_out,
                captured.cancelled,
                duration_ms,
                output_truncated,
                Some(&review),
                command_contains_ssh(&command),
                &session_history,
            )
        }
        .await;

        if command_cancelled {
            ToolResult::cancelled(result)
        } else {
            ToolResult::from_text(result)
        }
    }
}

fn local_zsh_dir(workspace: &Path) -> Result<PathBuf, String> {
    // plain-chat-browser 是聊天占位工作区，其执行目录按设计落在
    // workspace.parent()/local_env/zsh；此时以 workspace.parent() 为合法根目录做
    // 包含性校验。其余情况执行目录必须位于工作区根目录内。
    let is_plain_chat = workspace
        .file_name()
        .is_some_and(|name| name == "plain-chat-browser");
    let allowed_root = if is_plain_chat {
        workspace
            .parent()
            .ok_or_else(|| "错误：无法解析聊天工作区根目录".to_string())?
            .to_path_buf()
    } else {
        workspace.to_path_buf()
    };
    let dir = if is_plain_chat {
        allowed_root.join("local_env").join("zsh")
    } else {
        workspace
            .join(".jkcodingagent")
            .join("local_env")
            .join("zsh")
    };
    let normalized = super::common::lexical_normalize(&dir);
    let normalized_root = super::common::lexical_normalize(&allowed_root);
    if !normalized.starts_with(&normalized_root) {
        return Err("错误：local_zsh 目录解析到了合法根目录之外".to_string());
    }
    Ok(normalized)
}

fn blacklist_reason(command: &str) -> Option<&'static str> {
    let normalized = command.to_ascii_lowercase();
    if command_invokes_command(&normalized, "cd") {
        return Some("禁止使用 cd；local_zsh 的执行目录固定为 .jkcodingagent/local_env/zsh");
    }
    if normalized.contains(":(){:|:&};:") {
        return Some("禁止 fork bomb");
    }

    let dangerous_commands = [
        ("sudo", "禁止 sudo 提权"),
        ("su", "禁止切换用户"),
        ("shutdown", "禁止关机"),
        ("reboot", "禁止重启"),
        ("halt", "禁止停止系统"),
        ("poweroff", "禁止关闭电源"),
        ("mkfs", "禁止格式化文件系统"),
    ];
    if let Some(reason) = dangerous_commands
        .iter()
        .find_map(|(name, reason)| command_invokes_command(&normalized, name).then_some(*reason))
    {
        return Some(reason);
    }

    let dangerous_patterns = [
        ("diskutil erase", "禁止擦除磁盘/卷"),
        ("dd if=/dev/zero", "禁止直接覆写块设备"),
        ("dd if=/dev/random", "禁止直接覆写块设备"),
        ("dd if=/dev/urandom", "禁止直接覆写块设备"),
        ("rm -rf /", "禁止删除根目录"),
        ("rm -rf ~", "禁止删除用户目录"),
        ("chmod 777 /", "禁止放开系统路径权限"),
        ("chmod -r 777 /", "禁止递归放开系统路径权限"),
        ("chown root", "禁止改为 root 所有者"),
        ("curl | sh", "禁止管道执行远程脚本"),
        ("curl | bash", "禁止管道执行远程脚本"),
        ("wget | sh", "禁止管道执行远程脚本"),
        ("wget | bash", "禁止管道执行远程脚本"),
    ];

    dangerous_patterns
        .iter()
        .find_map(|(pattern, reason)| normalized.contains(pattern).then_some(*reason))
}

async fn review_local_command(
    args: &Value,
    context: &ToolContext,
    run_dir: &Path,
    command: &str,
) -> Result<crate::ssh_tool::SshAuditReview, String> {
    // fail-closed：未配置审查时对可执行命令默认拦截，不得跳过审查放行。
    let Some(review_config) = context.ssh_review.as_ref() else {
        return Err(
            "未配置安全审查，已拒绝执行命令。请先在应用设置中配置安全审查模型。".to_string(),
        );
    };

    let intent = string_arg(args, "compress_intent")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| context.session_title.clone());
    let payload = crate::agent::ssh_review::CommandReviewPayload {
        intent,
        task: context.user_task.clone().unwrap_or_default(),
        target: crate::agent::ssh_review::CommandReviewTarget::LocalZsh {
            workspace_path: context.workspace.display().to_string(),
            run_dir: run_dir.display().to_string(),
        },
        command: command.to_string(),
        stdin: None,
    };

    crate::agent::ssh_review::review_shell_command(review_config, &payload)
        .await
        .map_err(|error| format!("审查服务异常：{error}"))
        .map(|verdict| crate::ssh_tool::SshAuditReview {
            allowed: verdict.allowed,
            reason: verdict.reason,
        })
}

/// 写入「被拦截」审计条目，并返回带「错误：」前缀的拦截响应（含审计明细）。
async fn blocked_command_response(
    run_dir: &Path,
    session_id: &str,
    command: &str,
    review: crate::ssh_tool::SshAuditReview,
    headline: String,
) -> String {
    let entry = blocked_audit_entry(session_id, command, review);
    let render_entry = entry.clone();
    let run_dir_for_audit = run_dir.to_path_buf();
    let session_id_for_history = session_id.to_string();
    let audit_result = tokio::task::spawn_blocking(move || {
        append_audit_entry(&run_dir_for_audit, entry, &session_id_for_history)
    })
    .await;
    match audit_result {
        Ok(Ok(history)) => format!(
            "{headline}\n\n{}",
            render_local_audit_entry(run_dir, &render_entry, true, &history)
        ),
        Ok(Err(error)) => format!("{headline}\n\n错误：审计历史写入失败：{error}"),
        Err(error) => format!("{headline}\n\n错误：审计历史写入失败：{error}"),
    }
}

fn blocked_audit_entry(
    session_id: &str,
    command: &str,
    review: crate::ssh_tool::SshAuditReview,
) -> LocalZshAuditEntry {
    LocalZshAuditEntry {
        id: uuid::Uuid::new_v4().to_string(),
        session_id: session_id.to_string(),
        executed_at: Utc::now().to_rfc3339(),
        command: command.to_string(),
        review: Some(review),
        exit_code: None,
        timed_out: false,
        cancelled: false,
        duration_ms: 0,
        stdout: String::new(),
        stderr: String::new(),
        output_truncated: false,
        error: Some("命令被审查 AI 拦截，未执行。".to_string()),
    }
}

fn command_invokes_command(command: &str, target: &str) -> bool {
    command
        .split(|ch: char| matches!(ch, ';' | '&' | '|' | '(' | ')' | '\n'))
        .filter_map(|segment| segment.split_whitespace().next())
        .any(|token| token == target)
}

async fn capture_command_output(
    child: &mut tokio::process::Child,
    timeout_secs: u64,
    cancel_rx: Option<watch::Receiver<bool>>,
) -> std::io::Result<CapturedCommandOutput> {
    let stdout_reader = child.stdout.take();
    let stderr_reader = child.stderr.take();
    let stdout_task = tokio::spawn(async move { read_limited(stdout_reader).await });
    let stderr_task = tokio::spawn(async move { read_limited(stderr_reader).await });

    let wait_outcome = {
        let wait = child.wait();
        tokio::pin!(wait);
        let deadline = sleep(Duration::from_secs(timeout_secs));
        tokio::pin!(deadline);
        let cancellation = wait_for_cancellation(cancel_rx);
        tokio::pin!(cancellation);

        tokio::select! {
            biased;
            status = &mut wait => CommandWaitOutcome::Exited(status),
            _ = &mut cancellation => CommandWaitOutcome::Cancelled,
            _ = &mut deadline => CommandWaitOutcome::TimedOut,
        }
    };

    let (status, timed_out, cancelled) = match wait_outcome {
        CommandWaitOutcome::Exited(status) => (status?, false, false),
        CommandWaitOutcome::TimedOut => {
            // 超时：杀整个进程组（含派生的孙进程），确保管道写端全部关闭、
            // reader 能读到 EOF，不会永久阻塞。
            kill_process_group(child);
            (child.wait().await?, true, false)
        }
        CommandWaitOutcome::Cancelled => {
            kill_process_group(child);
            (child.wait().await?, false, true)
        }
    };

    let (stdout, stdout_read) = stdout_task.await.map_err(std::io::Error::other)?;
    let (stderr, stderr_read) = stderr_task.await.map_err(std::io::Error::other)?;

    Ok(CapturedCommandOutput {
        output: Output {
            status,
            stdout,
            stderr,
        },
        total_bytes_read: stdout_read + stderr_read,
        timed_out,
        cancelled,
    })
}

enum CommandWaitOutcome {
    Exited(std::io::Result<std::process::ExitStatus>),
    TimedOut,
    Cancelled,
}

async fn wait_for_cancellation(mut cancel_rx: Option<watch::Receiver<bool>>) {
    let Some(cancel_rx) = cancel_rx.as_mut() else {
        std::future::pending::<()>().await;
        return;
    };
    if *cancel_rx.borrow() {
        return;
    }
    loop {
        match cancel_rx.changed().await {
            Ok(()) if *cancel_rx.borrow() => return,
            Ok(()) => {}
            Err(_) => return,
        }
    }
}

async fn read_limited<R>(reader: Option<R>) -> (Vec<u8>, usize)
where
    R: AsyncRead + Unpin,
{
    let Some(mut reader) = reader else {
        return (Vec::new(), 0);
    };

    let mut retained = Vec::new();
    let mut total_read = 0;
    let mut chunk = [0u8; 8192];
    loop {
        match reader.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => {
                total_read += n;
                let remaining = MAX_OUTPUT_BYTES.saturating_sub(retained.len());
                if remaining > 0 {
                    retained.extend_from_slice(&chunk[..n.min(remaining)]);
                }
            }
            Err(_) => break,
        }
    }

    (retained, total_read)
}

/// 终止子进程所在的整个进程组。spawn 时设置了 `process_group(0)`，
/// 子进程 pid 即进程组 id；组杀失败时兜底单杀直接子进程。
#[cfg(unix)]
fn kill_process_group(child: &mut tokio::process::Child) {
    if let Some(pid) = child.id() {
        unsafe {
            libc::killpg(pid as libc::pid_t, libc::SIGKILL);
        }
    }
    let _ = child.start_kill();
}

#[cfg(not(unix))]
fn kill_process_group(child: &mut tokio::process::Child) {
    let _ = child.start_kill();
}

/// 按 run_dir 粒度的审计写锁：并发调用串行化 audit.json 的读-改-写，避免互相覆盖。
fn audit_lock_for(run_dir: &Path) -> Arc<Mutex<()>> {
    static AUDIT_LOCKS: std::sync::OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> =
        std::sync::OnceLock::new();
    AUDIT_LOCKS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .entry(run_dir.to_path_buf())
        .or_default()
        .clone()
}

fn append_audit_entry(
    run_dir: &Path,
    entry: LocalZshAuditEntry,
    session_id: &str,
) -> Result<Vec<LocalZshAuditEntry>, String> {
    // 锁的用途就是串行化本文件的读-改-写，持锁期间仅做该审计文件的 I/O。
    let lock = audit_lock_for(run_dir);
    let _guard = lock.lock();
    let audit_path = run_dir.join(AUDIT_FILE_NAME);
    let mut log = match fs::read_to_string(&audit_path) {
        Ok(content) if !content.trim().is_empty() => {
            serde_json::from_str::<LocalZshAuditLog>(&content)
                .map_err(|error| format!("解析 audit.json 失败：{error}"))?
        }
        Ok(_) => LocalZshAuditLog::default(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => LocalZshAuditLog::default(),
        Err(error) => return Err(format!("读取 audit.json 失败：{error}")),
    };

    log.version = 1;
    log.entries.push(entry);
    if log.entries.len() > HISTORY_LIMIT {
        let drop_count = log.entries.len() - HISTORY_LIMIT;
        log.entries.drain(0..drop_count);
    }

    let content = serde_json::to_string_pretty(&log)
        .map_err(|error| format!("序列化 audit.json 失败：{error}"))?;
    // 原子写：先写临时文件再 rename，避免写一半崩溃留下损坏的 JSON。
    let tmp_path = run_dir.join(format!(".{AUDIT_FILE_NAME}.tmp"));
    fs::write(&tmp_path, content).map_err(|error| format!("写入 audit.json 失败：{error}"))?;
    fs::rename(&tmp_path, &audit_path).map_err(|error| format!("写入 audit.json 失败：{error}"))?;

    Ok(log
        .entries
        .into_iter()
        .filter(|item| item.session_id == session_id)
        .collect())
}

#[allow(clippy::too_many_arguments)]
fn render_command_result(
    run_dir: &Path,
    command: &str,
    stdout: &str,
    stderr: &str,
    exit_code: Option<i32>,
    timed_out: bool,
    cancelled: bool,
    duration_ms: u128,
    output_truncated: bool,
    review: Option<&crate::ssh_tool::SshAuditReview>,
    show_session_history: bool,
    session_history: &[LocalZshAuditEntry],
) -> String {
    let mut result = String::new();
    result.push_str("## local_zsh 执行结果\n\n");
    result.push_str(&format!("- 工作目录: `{}`\n", run_dir.display()));
    result.push_str(&format!(
        "- 退出码: `{}`\n",
        exit_code_label(exit_code, timed_out, cancelled)
    ));
    result.push_str(&format!("- 耗时: `{duration_ms}ms`\n"));
    if output_truncated {
        result.push_str("- 输出: `已截断`\n");
    }
    if cancelled {
        result.push_str("- 终止状态: `已取消，进程组已收敛`\n");
    }
    push_review_summary(&mut result, review);
    result.push_str("\n### 命令\n\n");
    result.push_str("```zsh\n");
    result.push_str(command);
    result.push_str("\n```\n");

    if !stdout.trim().is_empty() {
        result.push_str("\n### stdout\n\n");
        result.push_str("```text\n");
        result.push_str(&truncate_chars(stdout, MAX_RESULT_CHARS));
        result.push_str("\n```\n");
    }
    if !stderr.trim().is_empty() {
        result.push_str("\n### stderr\n\n");
        result.push_str("```text\n");
        result.push_str(&truncate_chars(stderr, MAX_RESULT_CHARS));
        result.push_str("\n```\n");
    }
    if review.as_ref().is_some_and(|review| !review.allowed) {
        result.push_str("\n[命令被审查 AI 拦截，未执行]\n");
    } else if stdout.trim().is_empty() && stderr.trim().is_empty() {
        result.push_str("\n[命令已完成，无输出]\n");
    }

    if show_session_history {
        result.push_str("\n### 当前会话命令历史\n\n");
        result.push_str("| 时间 | 状态 | 命令 |\n|---|---:|---|\n");
        for item in session_history.iter().rev().take(HISTORY_LIMIT) {
            result.push_str(&format!(
                "| {} | {} | `{}` |\n",
                item.executed_at,
                history_status_label(item),
                escape_table_cell(&truncate_chars(&item.command, 160))
            ));
        }
        result.push_str("\n审计文件: `audit.json`\n");
    }

    result
}

fn render_local_audit_entry(
    run_dir: &Path,
    entry: &LocalZshAuditEntry,
    show_session_history: bool,
    session_history: &[LocalZshAuditEntry],
) -> String {
    render_command_result(
        run_dir,
        &entry.command,
        &entry.stdout,
        &entry.stderr,
        entry.exit_code,
        entry.timed_out,
        entry.cancelled,
        entry.duration_ms,
        entry.output_truncated,
        entry.review.as_ref(),
        show_session_history,
        session_history,
    )
}

fn push_review_summary(result: &mut String, review: Option<&crate::ssh_tool::SshAuditReview>) {
    let Some(review) = review else {
        return;
    };
    result.push_str(&format!(
        "- 审查结论: `{}`\n",
        if review.allowed { "通过" } else { "拦截" }
    ));
    let reason = if review.reason.trim().is_empty() {
        if review.allowed {
            "审查通过，允许执行。"
        } else {
            "审查拒绝，命令未执行。"
        }
    } else {
        review.reason.trim()
    };
    result.push_str(&format!("- 审查原因: {reason}\n"));
}

fn exit_code_label(exit_code: Option<i32>, timed_out: bool, cancelled: bool) -> String {
    if cancelled {
        return "cancelled".to_string();
    }
    if timed_out {
        return "timeout".to_string();
    }
    exit_code
        .map(|code| code.to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

fn history_status_label(item: &LocalZshAuditEntry) -> String {
    if item.review.as_ref().is_some_and(|review| !review.allowed) {
        return "review-blocked".to_string();
    }
    if item.error.is_some() {
        return "error".to_string();
    }
    exit_code_label(item.exit_code, item.timed_out, item.cancelled)
}

fn command_contains_ssh(command: &str) -> bool {
    command_invokes_command(&command.to_ascii_lowercase(), "ssh")
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    let mut output = String::new();
    for (index, ch) in value.chars().enumerate() {
        if index >= max_chars {
            output.push_str("\n...[已截断]");
            return output;
        }
        output.push(ch);
    }
    output
}

fn escape_table_cell(value: &str) -> String {
    value.replace('|', "\\|").replace('\n', " ")
}

#[cfg(test)]
mod tests {
    use std::process::Stdio;

    use super::{capture_command_output, AgentTool, LocalZshTool};
    use tokio::process::Command;

    #[test]
    fn local_zsh_schema_does_not_expose_runtime_timeout() {
        let parameters = LocalZshTool.parameters();

        assert!(parameters["properties"].get("command").is_some());
        assert!(parameters["properties"].get("timeout").is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_kills_the_zsh_process_group() {
        let mut command = Command::new("/bin/zsh");
        command
            .arg("-lc")
            .arg("sleep 30 & wait")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        let mut child = command.spawn().expect("spawn zsh");
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        cancel_tx.send(true).expect("request cancellation");

        let captured = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            capture_command_output(&mut child, 30, Some(cancel_rx)),
        )
        .await
        .expect("process group should settle")
        .expect("capture output");

        assert!(captured.cancelled);
        assert!(!captured.timed_out);
    }
}
