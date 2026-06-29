use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};

use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::time::{timeout, Duration};

use super::common::{string_arg, u64_arg, with_compression_parameters};
use crate::agent::tools::context::ToolContext;
use crate::agent::tools::registry::AgentTool;

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
        "在 macOS 本地 zsh 环境执行命令。命令固定运行于当前会话工作区的 .jkcodingagent/local_env/zsh，产物也应写入该目录；工具会维护 audit.json，记录最新 20 条命令、结果和执行会话。禁止 cd 和少量高危系统命令。"
    }

    fn parameters(&self) -> Value {
        with_compression_parameters(
            json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "要在 /bin/zsh -lc 中执行的命令。不要使用 cd；当前目录固定为 .jkcodingagent/local_env/zsh。"
                    },
                    "timeout": {
                        "type": "integer",
                        "description": "超时时间，单位秒，默认使用会话工具超时。",
                        "minimum": 1
                    }
                },
                "required": ["command"]
            }),
            false,
            "local_zsh 会返回命令输出和审计历史摘要；默认保留原文，输出很长时系统仍会压缩。",
        )
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
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

        let timeout_secs = u64_arg(args, "timeout")
            .unwrap_or(context.exec_timeout_secs)
            .max(1);
        let workspace = context.workspace.clone();
        let session_id = context.workspace_id.clone();

        let run_result = tokio::task::spawn_blocking(move || {
            let run_dir = local_zsh_dir(&workspace)?;
            fs::create_dir_all(&run_dir)
                .map_err(|error| format!("创建 local_zsh 目录失败：{error}"))?;
            Ok::<PathBuf, String>(run_dir)
        })
        .await
        .map_err(|error| format!("准备 local_zsh 目录失败：{error}"));

        let run_dir = match run_result {
            Ok(Ok(dir)) => dir,
            Ok(Err(error)) | Err(error) => return error,
        };

        let review_outcome = match review_local_command(args, context, &run_dir, &command).await {
            Ok(outcome) => outcome,
            Err(error) => {
                let review = crate::ssh_tool::SshAuditReview {
                    allowed: false,
                    reason: format!("审查服务异常：{error}"),
                };
                let entry = blocked_audit_entry(&session_id, &command, review);
                let run_dir_for_audit = run_dir.clone();
                let session_id_for_history = session_id.clone();
                let audit_result = tokio::task::spawn_blocking(move || {
                    append_audit_entry(&run_dir_for_audit, entry.clone(), &session_id_for_history)
                        .map(|history| (entry, history))
                })
                .await
                .map_err(|error| format!("写入 local_zsh 审计历史失败：{error}"));
                return match audit_result {
                    Ok(Ok((entry, history))) => {
                        render_local_audit_entry(&run_dir, &entry, true, &history)
                    }
                    Ok(Err(error)) | Err(error) => {
                        format!("错误：命令已被安全审查拦截，但审计历史写入失败：{error}")
                    }
                };
            }
        };

        if let Some(review) = review_outcome.as_ref().filter(|review| !review.allowed) {
            let entry = blocked_audit_entry(&session_id, &command, review.clone());
            let run_dir_for_audit = run_dir.clone();
            let session_id_for_history = session_id.clone();
            let audit_result = tokio::task::spawn_blocking(move || {
                append_audit_entry(&run_dir_for_audit, entry.clone(), &session_id_for_history)
                    .map(|history| (entry, history))
            })
            .await
            .map_err(|error| format!("写入 local_zsh 审计历史失败：{error}"));
            return match audit_result {
                Ok(Ok((entry, history))) => {
                    render_local_audit_entry(&run_dir, &entry, true, &history)
                }
                Ok(Err(error)) | Err(error) => {
                    format!("错误：命令已被安全审查拦截，但审计历史写入失败：{error}")
                }
            };
        }

        let started = std::time::Instant::now();
        let mut child = match Command::new("/bin/zsh")
            .arg("-lc")
            .arg(&command)
            .current_dir(&run_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
        {
            Ok(child) => child,
            Err(error) => return format!("执行 zsh 命令失败：{error}"),
        };

        let captured = match capture_command_output(&mut child, timeout_secs).await {
            Ok(output) => output,
            Err(error) => return format!("执行 zsh 命令失败：{error}"),
        };
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
            review: review_outcome.clone(),
            exit_code: captured.output.status.code(),
            timed_out: captured.timed_out,
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
        .map_err(|error| format!("写入 local_zsh 审计历史失败：{error}"));

        let session_history = match audit_result {
            Ok(Ok(history)) => history,
            Ok(Err(error)) | Err(error) => {
                return format!(
                    "命令已执行，但审计历史写入失败：{error}\n\n{}",
                    render_command_result(
                        &run_dir,
                        &command,
                        &stdout,
                        &stderr,
                        captured.output.status.code(),
                        captured.timed_out,
                        duration_ms,
                        output_truncated,
                        review_outcome.as_ref(),
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
            duration_ms,
            output_truncated,
            review_outcome.as_ref(),
            command_contains_ssh(&command),
            &session_history,
        )
    }
}

fn local_zsh_dir(workspace: &Path) -> Result<PathBuf, String> {
    let dir = if workspace
        .file_name()
        .is_some_and(|name| name == "plain-chat-browser")
    {
        workspace
            .parent()
            .ok_or_else(|| "无法解析聊天 local_env 根目录".to_string())?
            .join("local_env")
            .join("zsh")
    } else {
        workspace
            .join(".jkcodingagent")
            .join("local_env")
            .join("zsh")
    };
    let normalized = super::common::lexical_normalize(&dir);
    let allowed_root = normalized
        .parent()
        .ok_or_else(|| "无法解析 local_env 根目录".to_string())?;
    if !normalized.starts_with(allowed_root) {
        return Err("local_zsh 目录解析到了 local_env 之外".to_string());
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
) -> Result<Option<crate::ssh_tool::SshAuditReview>, String> {
    let Some(review_config) = context.ssh_review.as_ref() else {
        return Ok(None);
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
    };

    crate::agent::ssh_review::review_shell_command(review_config, &payload)
        .await
        .map(|verdict| {
            Some(crate::ssh_tool::SshAuditReview {
                allowed: verdict.allowed,
                reason: verdict.reason,
            })
        })
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
) -> std::io::Result<CapturedCommandOutput> {
    let stdout_reader = child.stdout.take();
    let stderr_reader = child.stderr.take();
    let stdout_task = tokio::spawn(async move { read_limited(stdout_reader).await });
    let stderr_task = tokio::spawn(async move { read_limited(stderr_reader).await });

    let (status, timed_out) = match timeout(Duration::from_secs(timeout_secs), child.wait()).await {
        Ok(status) => (status?, false),
        Err(_) => {
            child.kill().await?;
            (child.wait().await?, true)
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
    })
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

fn append_audit_entry(
    run_dir: &Path,
    entry: LocalZshAuditEntry,
    session_id: &str,
) -> Result<Vec<LocalZshAuditEntry>, String> {
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
    fs::write(&audit_path, content).map_err(|error| format!("写入 audit.json 失败：{error}"))?;

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
        exit_code_label(exit_code, timed_out)
    ));
    result.push_str(&format!("- 耗时: `{duration_ms}ms`\n"));
    if output_truncated {
        result.push_str("- 输出: `已截断`\n");
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

fn exit_code_label(exit_code: Option<i32>, timed_out: bool) -> String {
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
    exit_code_label(item.exit_code, item.timed_out)
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
