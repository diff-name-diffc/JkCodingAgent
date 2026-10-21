//! 单次本机命令：参数、审查、经监督器执行（yield/后台/收割）与审计。
//!
//! 执行模型参考 codex shell：`yield_time_ms` 软等待窗口内结束走同步渲染；
//! 未结束返回 run_id 转后台（监督器继续收集输出、受 `exec_timeout_secs`
//! 硬超时约束），后续调用用 `wait_run_id` 继续等待或收割。
use super::super::super::common::{boolish_arg, u64_arg, usize_arg};
use super::*;

#[cfg(test)]
mod tests;

/// yield_time_ms 声明区间（与 schema 文案一致，两处不许各自漂移）。
const YIELD_MIN_MS: u64 = 250;
const YIELD_MAX_MS: u64 = 30_000;
const YIELD_DEFAULT_MS: u64 = 10_000;
/// max_output_chars 声明区间。
const MAX_OUTPUT_CHARS_MIN: usize = 1_000;
const MAX_OUTPUT_CHARS_MAX: usize = 32_000;

/// 执行与审计完成后返回结构化结果，退出码不得当作成功正文。
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_local_zsh(
    args: &Value,
    workspace: PathBuf,
    chat_workspace: Option<ChatWorkspace>,
    workspace_id: String,
    exec_timeout_secs: u64,
    cancel_rx: Option<watch::Receiver<bool>>,
    review_context: crate::agent::rig_ext::review::RigReviewContext,
    app_handle: Option<tauri::AppHandle>,
) -> Result<String, ToolExecutionError> {
    let command = string_arg(args, "command")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let wait_run_id = string_arg(args, "wait_run_id").filter(|value| !value.trim().is_empty());
    if command.is_none() && wait_run_id.is_none() {
        return Err(ToolExecutionError::invalid_args(
            "错误：必须提供 command（要执行的命令）或 wait_run_id（等待既有后台运行）",
        ));
    }
    let yield_ms = u64_arg(args, "yield_time_ms")
        .unwrap_or(YIELD_DEFAULT_MS)
        .clamp(YIELD_MIN_MS, YIELD_MAX_MS);
    let yield_window = std::time::Duration::from_millis(yield_ms);
    let max_output_chars = usize_arg(args, "max_output_chars")
        .unwrap_or(MAX_RESULT_CHARS)
        .clamp(MAX_OUTPUT_CHARS_MIN, MAX_OUTPUT_CHARS_MAX);
    let tty = boolish_arg(args, "tty").unwrap_or(false);

    // ── 收割路径：等待既有后台运行，不重复审查（启动时已审）。──
    if let Some(run_id) = wait_run_id {
        if command.is_some() {
            return Err(ToolExecutionError::invalid_args(
                "错误：wait_run_id 与 command 不可同时提供；等待后台运行时只传 wait_run_id",
            ));
        }
        let Some(run) = sessions::lookup(&run_id, &workspace_id) else {
            return Err(ToolExecutionError::invalid_args(format!(
                "错误：未找到运行 {run_id}（仅同一会话内可等待；已收割或不存在的 run_id 按不存在处理）"
            )));
        };
        return match sessions::wait_for(&run, yield_window).await {
            sessions::RunSnapshot::Running {
                elapsed_ms,
                stdout,
                stderr,
                output_truncated,
                ..
            } => Ok(render_still_running(
                &run_id,
                &run.command,
                elapsed_ms,
                &stdout,
                &stderr,
                output_truncated,
                max_output_chars,
                exec_timeout_secs,
            )),
            sessions::RunSnapshot::Finished(finished) => {
                sessions::reap(&run_id, &workspace_id);
                let text = render_command_result(
                    &unknown_run_dir_hint(&run_id),
                    &unknown_run_dir_hint(&run_id),
                    &run.command,
                    &finished.stdout,
                    &finished.stderr,
                    finished.exit_code,
                    finished.timed_out,
                    finished.cancelled,
                    finished.duration_ms,
                    finished.output_truncated,
                    None,
                    command_contains_ssh(&run.command),
                    &finished.history,
                    max_output_chars,
                );
                classify_finished(&finished, text)
            }
        };
    }
    let command = command.unwrap_or_default();

    let working_directory = match args.get("working_directory") {
        None => None,
        Some(Value::String(path)) if !path.trim().is_empty() => Some(path.clone()),
        Some(_) => {
            return Err(ToolExecutionError::invalid_args(
                "错误：working_directory 必须是非空路径字符串",
            ))
        }
    };
    if let Some(reason) = blacklist_reason(&command) {
        command_history::record(
            &workspace_id,
            "local_zsh",
            "本地 zsh",
            &command,
            CommandHistoryStatus::Blocked,
            &format!("命中内置黑名单：{reason}"),
        );
        return Err(ToolExecutionError::refused(format!(
            "错误：local_zsh 已拦截命令：{reason}\n命令：{command}"
        )));
    }

    let timeout_secs = exec_timeout_secs.max(1);
    let session_id = workspace_id;
    // 审查载荷需要工作区绝对路径；workspace 随后被移入 spawn_blocking 闭包。
    let workspace_for_review = workspace.clone();
    let chat_for_paths = chat_workspace.clone();

    let paths_result = tokio::task::spawn_blocking(move || {
        paths::prepare(
            &workspace,
            chat_for_paths.as_ref(),
            working_directory.as_deref(),
        )
    })
    .await
    .map_err(|error| format!("错误：准备 local_zsh 目录失败：{error}"));

    let paths::ExecutionPaths {
        run_dir,
        audit_dir,
        temp_dir,
    } = match paths_result {
        Ok(Ok(paths)) => paths,
        Ok(Err(error)) | Err(error) => return Err(ToolExecutionError::other(error)),
    };

    // 安全审查门禁（fail-closed）：未配置审查 / 审查异常 / 判定不通过一律拒绝执行，
    // 并把「被拦截」写入 audit.json 审计。
    let mut review = match review_local_command(
        args,
        &review_context,
        &session_id,
        &workspace_for_review,
        &run_dir,
        &command,
    )
    .await
    {
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
                user_approved: false,
                reason: error.clone(),
            };
            return Err(ToolExecutionError::refused(
                blocked_command_response(
                    &run_dir,
                    &audit_dir,
                    &session_id,
                    &command,
                    blocked,
                    format!("错误：{error}"),
                )
                .await,
            ));
        }
    };
    if !review.allowed {
        // 需用户确认类拦截（原因带「需用户确认」标记）：统一走通用弹窗门禁。
        // 裁决结局必须区分——用户明确拒绝≠超时无人裁决：拒绝后引导重发等于
        // 让模型重试刚被用户否决的命令；等待期间取消按取消语义收口（取消
        // 不得改判为审查拦截）。
        let confirm_outcome = if crate::agent::rig_ext::review_confirm::needs_user_confirmation(
            &review.reason,
            &command,
            false,
        ) {
            Some(
                crate::agent::rig_ext::review_confirm::request_confirmation(
                    app_handle.as_ref(),
                    cancel_rx.clone(),
                    crate::agent::rig_ext::review_confirm::ConfirmRequest {
                        workspace_id: session_id.clone(),
                        tool: "local_zsh".to_string(),
                        target: "本地 zsh".to_string(),
                        command: command.clone(),
                        reason: review.reason.clone(),
                        elevated: false,
                    },
                )
                .await,
            )
        } else {
            None
        };
        match confirm_outcome {
            Some(crate::agent::rig_ext::review_confirm::ConfirmOutcome::Approved) => {
                // 用户确认放行：审计结论改标为经确认放行，并继续正常执行路径。
                review = crate::ssh_tool::SshAuditReview {
                    allowed: true,
                    user_approved: true,
                    reason: format!("安全审查未自动放行，经用户确认放行：{}", review.reason),
                };
            }
            Some(crate::agent::rig_ext::review_confirm::ConfirmOutcome::Cancelled) => {
                return Err(ToolExecutionError::cancelled(
                    "错误：等待用户确认期间本轮已取消，命令未执行。",
                ));
            }
            Some(crate::agent::rig_ext::review_confirm::ConfirmOutcome::Denied) => {
                command_history::record(
                    &session_id,
                    "local_zsh",
                    "本地 zsh",
                    &command,
                    CommandHistoryStatus::Blocked,
                    &format!("用户已明确拒绝执行：{}", review.reason),
                );
                // 用户明确拒绝同样落 audit.json（与超时/未弹窗口径一致）：拒绝
                // 是最需要留痕的裁决，取证链不得缺记。
                let denied_review = crate::ssh_tool::SshAuditReview {
                    allowed: false,
                    user_approved: false,
                    reason: format!("用户已明确拒绝执行：{}", review.reason),
                };
                return Err(ToolExecutionError::refused(
                    blocked_command_response(
                        &run_dir,
                        &audit_dir,
                        &session_id,
                        &command,
                        denied_review,
                        "错误：用户已明确拒绝执行该命令，不得重试或改写绕过。".to_string(),
                    )
                    .await,
                ));
            }
            Some(crate::agent::rig_ext::review_confirm::ConfirmOutcome::Timeout) | None => {
                command_history::record(
                    &session_id,
                    "local_zsh",
                    "本地 zsh",
                    &command,
                    CommandHistoryStatus::Blocked,
                    &review.reason,
                );
                let headline = crate::agent::ssh_review::with_confirm_guidance(
                    format!("错误：命令已被安全审查拦截：{}", review.reason),
                    &review.reason,
                );
                return Err(ToolExecutionError::refused(
                    blocked_command_response(
                        &run_dir,
                        &audit_dir,
                        &session_id,
                        &command,
                        review,
                        headline,
                    )
                    .await,
                ));
            }
        }
    }

    if cancel_rx
        .as_ref()
        .is_some_and(|rx| *rx.borrow() || rx.has_changed().is_err())
    {
        return Err(ToolExecutionError::cancelled(
            "错误：本机命令已取消，未启动进程",
        ));
    }

    // ── 启动监督器：窗口内结束走同步渲染；否则转后台。──
    let context = sessions::SupervisorContext {
        workspace_id: session_id.clone(),
        command: command.clone(),
        run_dir: run_dir.clone(),
        audit_dir: audit_dir.clone(),
        temp_dir,
        review: review.clone(),
        exec_timeout_secs,
        cancel_rx: cancel_rx.clone(),
        history_with_cwd: chat_workspace.is_some(),
    };
    let run = sessions::start_command(context, tty).map_err(ToolExecutionError::other)?;

    match sessions::wait_for(&run, yield_window).await {
        sessions::RunSnapshot::Finished(finished) => {
            sessions::reap(&run.run_id, &session_id);
            let text = render_command_result(
                &run_dir,
                &audit_dir,
                &command,
                &finished.stdout,
                &finished.stderr,
                finished.exit_code,
                finished.timed_out,
                finished.cancelled,
                finished.duration_ms,
                finished.output_truncated,
                Some(&review),
                command_contains_ssh(&command),
                &finished.history,
                max_output_chars,
            );
            classify_finished(&finished, text)
        }
        sessions::RunSnapshot::Running {
            run_id,
            elapsed_ms,
            stdout,
            stderr,
            output_truncated,
            ..
        } => Ok(render_still_running(
            &run_id,
            &command,
            elapsed_ms,
            &stdout,
            &stderr,
            output_truncated,
            max_output_chars,
            timeout_secs,
        )),
    }
}

/// 收割路径没有 run_dir 上下文（目录信息在审计条目里），渲染头用 run_id
/// 占位说明。保持 render_command_result 的签名统一。
fn unknown_run_dir_hint(run_id: &str) -> PathBuf {
    PathBuf::from(format!("(后台运行 {run_id}，目录见审计)"))
}

#[allow(clippy::too_many_arguments)]
fn render_still_running(
    run_id: &str,
    command: &str,
    elapsed_ms: u128,
    stdout: &str,
    stderr: &str,
    output_truncated: bool,
    max_output_chars: usize,
    timeout_secs: u64,
) -> String {
    let mut result = String::new();
    result.push_str("## local_zsh 执行结果\n\n");
    result.push_str("- 状态: `仍在后台运行`\n");
    result.push_str(&format!("- run_id: `{run_id}`\n"));
    result.push_str(&format!("- 已运行: `{elapsed_ms}ms`\n"));
    result.push_str(&format!(
        "- 硬超时: 启动后 `{timeout_secs}s` 强制终止并落审计\n"
    ));
    if output_truncated {
        result.push_str("- 已有输出: `已截断`\n");
    }
    result.push_str("\n### 命令\n\n");
    result.push_str("```zsh\n");
    result.push_str(command);
    result.push_str("\n```\n");
    if !stdout.trim().is_empty() {
        result.push_str("\n### 已有输出（stdout）\n\n");
        result.push_str("```text\n");
        result.push_str(&super::audit::truncate_chars(stdout, max_output_chars));
        result.push_str("\n```\n");
    }
    if !stderr.trim().is_empty() {
        result.push_str("\n### 已有输出（stderr）\n\n");
        result.push_str("```text\n");
        result.push_str(&super::audit::truncate_chars(stderr, max_output_chars));
        result.push_str("\n```\n");
    }
    if stdout.trim().is_empty() && stderr.trim().is_empty() {
        result.push_str("\n[命令仍在运行，暂无输出]\n");
    }
    result.push_str(&format!(
        "\n继续等待：再次调用 local_zsh，只传 `wait_run_id=\"{run_id}\"`（可再设 yield_time_ms，命令无需重复提供）。\n"
    ));
    result
}

fn classify_finished(
    finished: &sessions::FinishedRun,
    text: String,
) -> Result<String, ToolExecutionError> {
    if let Some(audit_error) = &finished.audit_error {
        // 审计写失败只留痕，不得吞掉命令结果本身；取消优先。
        let text = format!("错误：命令已执行，但审计历史写入失败：{audit_error}\n\n{text}");
        if finished.cancelled {
            return Err(ToolExecutionError::cancelled(text));
        }
        return Err(ToolExecutionError::other(text)
            .with_code("audit_failed")
            .with_retryable(false));
    }
    if finished.cancelled {
        Err(ToolExecutionError::cancelled(text))
    } else if finished.timed_out {
        Err(ToolExecutionError::timeout(text).with_retryable(false))
    } else if finished.exit_code != Some(0) {
        Err(ToolExecutionError::other(text).with_code("command_failed"))
    } else {
        Ok(text)
    }
}
