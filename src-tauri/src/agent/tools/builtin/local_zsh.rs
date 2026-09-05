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
use crate::agent::command_history::{self, CommandHistoryStatus};
use crate::agent::ssh_review::with_confirm_guidance;
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
                command_history::record(
                    &context.workspace_id,
                    "local_zsh",
                    "本地 zsh",
                    &command,
                    CommandHistoryStatus::Blocked,
                    &format!("命中内置黑名单：{reason}"),
                );
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
                    // 与黑名单/review-denied 路径一致：审查异常导致的阻断也登记台账。
                    command_history::record(
                        &session_id,
                        "local_zsh",
                        "本地 zsh",
                        &command,
                        CommandHistoryStatus::Blocked,
                        &error,
                    );
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
                command_history::record(
                    &session_id,
                    "local_zsh",
                    "本地 zsh",
                    &command,
                    CommandHistoryStatus::Blocked,
                    &review.reason,
                );
                let headline = with_confirm_guidance(
                    format!("错误：命令已被安全审查拦截：{}", review.reason),
                    &review.reason,
                );
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

            // 命令台账：供后续命令的安全审查判断来龙去脉（如清理本任务派生的进程）。
            let history_note = if captured.timed_out {
                format!("超时终止（{timeout_secs}s）")
            } else if captured.cancelled {
                "已取消".to_string()
            } else {
                let exit = captured
                    .output
                    .status
                    .code()
                    .map(|code| format!("exit={code}"))
                    .unwrap_or_else(|| format!("exit={}", captured.output.status));
                let excerpt = if !stdout.is_empty() {
                    stdout.as_str()
                } else {
                    stderr.as_str()
                };
                format!("{exit}；输出：{excerpt}")
            };
            command_history::record(
                &session_id,
                "local_zsh",
                "本地 zsh",
                &command,
                CommandHistoryStatus::Executed,
                &history_note,
            );

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

    let payload = crate::agent::tools::review_context::build_review_payload(
        context,
        Some(args),
        crate::agent::ssh_review::CommandReviewTarget::LocalZsh {
            workspace_path: context.workspace.display().to_string(),
            run_dir: run_dir.display().to_string(),
        },
        command.to_string(),
        None,
    );

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
        .split([';', '&', '|', '(', ')', '\n'])
        .filter_map(|segment| segment.split_whitespace().next())
        .any(|token| token == target)
}

mod execution;
use execution::capture_command_output;
mod audit;
use audit::{
    append_audit_entry, command_contains_ssh, render_command_result, render_local_audit_entry,
};
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
