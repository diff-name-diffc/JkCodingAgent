//! 单次本机命令：参数、审查、执行与审计。
use super::*;

#[cfg(test)]
mod tests;

/// 执行与审计完成后返回结构化结果，退出码不得当作成功正文。
pub(super) async fn run_local_zsh(
    args: &Value,
    workspace: PathBuf,
    workspace_id: String,
    exec_timeout_secs: u64,
    cancel_rx: Option<watch::Receiver<bool>>,
    review_context: crate::agent::rig_ext::review::RigReviewContext,
    app_handle: Option<tauri::AppHandle>,
) -> Result<String, ToolExecutionError> {
    let Some(command) = string_arg(args, "command") else {
        return Err(ToolExecutionError::invalid_args(
            "错误：缺少必填参数 command",
        ));
    };
    let command = command.trim().to_string();
    if command.is_empty() {
        return Err(ToolExecutionError::invalid_args("错误：command 不能为空"));
    }
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

    let run_result = tokio::task::spawn_blocking(move || {
        let run_dir = local_zsh_dir(&workspace)?;
        std::fs::create_dir_all(&run_dir)
            .map_err(|error| format!("错误：创建 local_zsh 目录失败：{error}"))?;
        Ok::<PathBuf, String>(run_dir)
    })
    .await
    .map_err(|error| format!("错误：准备 local_zsh 目录失败：{error}"));

    let run_dir = match run_result {
        Ok(Ok(dir)) => dir,
        Ok(Err(error)) | Err(error) => return Err(ToolExecutionError::other(error)),
    };

    // 安全审查门禁（fail-closed）：未配置审查 / 审查异常 / 判定不通过一律拒绝执行，
    // 并把「被拦截」写入 audit.json 审计（对齐旧实现的 `review_local_command` 与
    // `blocked_command_response` 的完整语义）。
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
                // 台账只在执行完成后登记一次（前置 Executed 会造成双写、且把
                // 「尚未执行」误标为已执行）。
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
                    blocked_command_response(&run_dir, &session_id, &command, review, headline)
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
        Err(error) => {
            return Err(ToolExecutionError::other(format!(
                "错误：执行 zsh 命令失败：{error}"
            )))
        }
    };

    let captured = match capture_command_output(&mut child, timeout_secs, cancel_rx).await {
        Ok(output) => output,
        Err(error) => {
            return Err(ToolExecutionError::other(format!(
                "错误：执行 zsh 命令失败：{error}"
            )))
        }
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
            let text = format!(
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
                    None,
                    false,
                    &[],
                )
            );
            // 取消优先：审计写失败只留痕，不得把用户取消改判为 audit_failed。
            if captured.cancelled {
                return Err(ToolExecutionError::cancelled(text));
            }
            return Err(ToolExecutionError::other(text)
                .with_code("audit_failed")
                .with_retryable(false));
        }
    };

    classify_command_result(
        &captured,
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
        ),
    )
}

fn classify_command_result(
    captured: &CapturedCommandOutput,
    text: String,
) -> Result<String, ToolExecutionError> {
    if captured.cancelled {
        Err(ToolExecutionError::cancelled(text))
    } else if captured.timed_out {
        Err(ToolExecutionError::timeout(text).with_retryable(false))
    } else if !captured.output.status.success() {
        Err(ToolExecutionError::other(text).with_code("command_failed"))
    } else {
        Ok(text)
    }
}
