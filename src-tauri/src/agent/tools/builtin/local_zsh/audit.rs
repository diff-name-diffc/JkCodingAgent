use super::*;

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

pub(super) fn append_audit_entry(
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
pub(super) fn render_command_result(
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

pub(super) fn render_local_audit_entry(
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

pub(super) fn command_contains_ssh(command: &str) -> bool {
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
