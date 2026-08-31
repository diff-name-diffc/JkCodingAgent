use std::path::Path;

use chrono::Utc;

use super::{SshAuditRecord, SshAuditReview, SshExecResult};

/// 单条审计记录里 stdout / stderr 各自保留的最大字符数（头尾各半）。
/// 完整输出仍随工具结果返回；这里只限制审计文件的落盘体积。
pub(super) const AUDIT_OUTPUT_CHARS: usize = 8_000;

impl SshAuditRecord {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn from_execution(
        workspace_path: &Path,
        workspace_id: String,
        session_title: String,
        server_id: String,
        session_id: String,
        command: String,
        result: &Result<SshExecResult, String>,
        review: Option<&SshAuditReview>,
    ) -> Self {
        let review = review.cloned();
        match result {
            Ok(output) => Self {
                created_at: Utc::now().to_rfc3339(),
                workspace_path: normalize_project_key(workspace_path),
                workspace_id,
                session_title,
                server_id,
                session_id,
                command,
                exit_code: Some(output.exit_code),
                stdout: truncate_for_audit(&output.stdout),
                stderr: truncate_for_audit(&output.stderr),
                duration_ms: Some(output.duration_ms),
                truncated: output.truncated,
                interactive_blocked: output.interactive_blocked,
                error: None,
                review,
            },
            Err(error) => Self {
                created_at: Utc::now().to_rfc3339(),
                workspace_path: normalize_project_key(workspace_path),
                workspace_id,
                session_title,
                server_id,
                session_id,
                command,
                exit_code: None,
                stdout: String::new(),
                stderr: String::new(),
                duration_ms: None,
                truncated: false,
                interactive_blocked: false,
                error: Some(sanitize_error_text(error)),
                review,
            },
        }
    }
}

/// 审计落盘只保留头尾各半的输出，防止单条大输出把审计文件撑大；
/// 完整输出仍随工具结果返回给调用方。
pub(super) fn truncate_for_audit(text: &str) -> String {
    let total = text.chars().count();
    if total <= AUDIT_OUTPUT_CHARS {
        return text.to_string();
    }
    let half = AUDIT_OUTPUT_CHARS / 2;
    let head: String = text.chars().take(half).collect();
    let tail: String = text.chars().skip(total - half).collect();
    format!(
        "{head}\n…[审计输出已截断，省略 {} 字符]…\n{tail}",
        total - half * 2
    )
}

pub fn render_ssh_audit_record_markdown(record: &SshAuditRecord) -> String {
    let mut output = String::new();
    output.push_str("## SSH 命令审查记录\n\n");
    output.push_str(&format!("- 时间: `{}`\n", record.created_at));
    output.push_str(&format!("- 服务器: `{}`\n", record.server_id));
    output.push_str(&format!("- 会话: `{}`\n", record.session_id));
    if let Some(review) = record.review.as_ref() {
        output.push_str(&format!(
            "- 审查结论: `{}`\n",
            if review.allowed { "通过" } else { "拦截" }
        ));
        output.push_str(&format!(
            "- 审查原因: {}\n",
            if review.reason.trim().is_empty() {
                if review.allowed {
                    "审查通过，允许执行。"
                } else {
                    "审查拒绝，命令未执行。"
                }
            } else {
                review.reason.trim()
            }
        ));
    } else {
        output.push_str("- 审查结论: `未审查`\n");
    }
    output.push_str(&format!(
        "- 执行状态: `{}`\n",
        audit_execution_status(record)
    ));
    output.push_str("\n### 命令\n\n```sh\n");
    output.push_str(&record.command);
    output.push_str("\n```\n");
    if !record.stdout.trim().is_empty() {
        output.push_str("\n### stdout\n\n```text\n");
        output.push_str(&record.stdout);
        output.push_str("\n```\n");
    }
    if !record.stderr.trim().is_empty() {
        output.push_str("\n### stderr\n\n```text\n");
        output.push_str(&record.stderr);
        output.push_str("\n```\n");
    }
    if let Some(error) = record
        .error
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        output.push_str("\n### 错误\n\n");
        output.push_str(error);
        output.push('\n');
    }
    output
}

fn audit_execution_status(record: &SshAuditRecord) -> String {
    if record.review.as_ref().is_some_and(|review| !review.allowed) {
        return "审查拦截，未执行".to_string();
    }
    if record.interactive_blocked {
        return "交互阻塞，已中止".to_string();
    }
    if record.error.is_some() {
        return "执行失败".to_string();
    }
    match record.exit_code {
        Some(code) => format!(
            "exit={code}, duration={}ms",
            record.duration_ms.unwrap_or(0)
        ),
        None => "未执行".to_string(),
    }
}

pub(super) fn normalize_project_key(project_path: &Path) -> String {
    project_path
        .canonicalize()
        .unwrap_or_else(|_| project_path.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

pub(super) fn sanitize_ssh_error(prefix: &str, error: russh::Error) -> String {
    format!("{prefix}：{}", sanitize_error_text(&error.to_string()))
}

pub(super) fn sanitize_error_text(error: &str) -> String {
    error
        .replace("password", "[redacted]")
        .replace("Password", "[redacted]")
        .replace("PASSWORD", "[redacted]")
        .replace("passphrase", "[redacted]")
        .replace("Passphrase", "[redacted]")
        .replace("PASSPHRASE", "[redacted]")
}
