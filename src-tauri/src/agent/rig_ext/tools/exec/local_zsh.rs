//! local_zsh 工具：在 macOS 本地 zsh 环境执行命令（受管工作目录 + 审计 + 超时/取消）。
//! 迁移自旧自实现工具层（含 `local_zsh_dir` 助手；旧文件已随迁移删除）。

use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::watch;

use super::super::common::{
    string_arg, with_compression_parameters, COMMAND_FORCE_COMPRESS_AFTER_CHARS,
};
use crate::agent::command_history::{self, CommandHistoryStatus};
use crate::agent::rig_ext::chat_workspace::ChatWorkspace;
use rig::tool::{PortableDynamicTool, ToolExecutionError, ToolOutput};

pub(crate) const MAX_OUTPUT_BYTES: usize = 128 * 1024;
pub(crate) const MAX_RESULT_CHARS: usize = 16_000;
pub(crate) const HISTORY_LIMIT: usize = 20;
pub(crate) const AUDIT_FILE_NAME: &str = "audit.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LocalZshAuditEntry {
    pub id: String,
    pub session_id: String,
    pub executed_at: String,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<String>,
    #[serde(default)]
    pub review: Option<crate::ssh_tool::SshAuditReview>,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    #[serde(default)]
    pub cancelled: bool,
    pub duration_ms: u128,
    pub stdout: String,
    pub stderr: String,
    pub output_truncated: bool,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LocalZshAuditLog {
    pub version: u32,
    pub entries: Vec<LocalZshAuditEntry>,
}

/// local_zsh 执行目录：`<workspace>/.jkcodingagent/local_env/zsh`；
/// plain-chat-browser 占位工作区落在 `workspace.parent()/local_env/zsh`。
/// 迁移自旧自实现工具层（已随迁移删除）。
pub(crate) fn local_zsh_dir(workspace: &Path) -> Result<PathBuf, String> {
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
    let normalized = super::super::common::lexical_normalize(&dir);
    let normalized_root = super::super::common::lexical_normalize(&allowed_root);
    if !normalized.starts_with(&normalized_root) {
        return Err("错误：local_zsh 目录解析到了合法根目录之外".to_string());
    }
    Ok(normalized)
}

pub(super) fn local_zsh_tool(
    workspace: PathBuf,
    chat_workspace: Option<ChatWorkspace>,
    workspace_id: String,
    exec_timeout_secs: u64,
    cancel_rx: Option<watch::Receiver<bool>>,
    review: crate::agent::rig_ext::review::RigReviewContext,
    app_handle: Option<tauri::AppHandle>,
) -> PortableDynamicTool {
    let mut parameters = json!({
        "type": "object",
        "properties": {
            "command": {
                "type": "string",
                "description": "要在 /bin/zsh -lc 中执行的命令。不要使用 cd；当前目录固定为 .jkcodingagent/local_env/zsh。"
            },
            "wait_run_id": {
                "type": "string",
                "description": "等待既有后台运行：只传本参数（不传 command），在 yield_time_ms 窗口内等待该 run_id 完成；未完成会再次返回已有输出。run_id 来自先前返回中仍在后台运行的命令。"
            },
            "yield_time_ms": {
                "type": "integer",
                "description": "等待命令完成的窗口（毫秒），默认 10000，范围 250-30000。窗口内命令结束则直接返回完整结果；超窗未结束则返回 run_id 转后台继续执行（仍受启动时硬超时约束），随后可用 wait_run_id 继续等待。长时间运行的命令（构建/测试/服务器）建议用默认窗口并接受转后台。",
                "minimum": 250,
                "maximum": 30000
            },
            "tty": {
                "type": "boolean",
                "description": "true 时用 PTY 执行（stdout/stderr 合并为单流，支持需要终端的交互式命令）；默认 false 用管道分流。",
                "default": false
            },
            "max_output_chars": {
                "type": "integer",
                "description": "返回结果中输出正文的字符预算，默认 16000，范围 1000-32000。需要核对更多原文时可调大。",
                "minimum": 1000,
                "maximum": 32000
            }
        }
    });
    let description = if chat_workspace.is_some() {
        parameters["properties"]["command"]["description"] = json!(
            "要在 /bin/zsh -lc 中执行的命令。不要使用 cd；需要工程执行目录时使用 working_directory。"
        );
        parameters["properties"]["working_directory"] = json!({
            "type": "string",
            "description": "可选绝对执行目录，必须位于当前会话目录或已绑定工程/成果目录内。不填时沿用会话临时执行目录。临时文件写会话目录，仅任务要求修改的工程文件和交付成果写共享目录。"
        });
        "在 macOS 本地 zsh 环境执行命令，执行前必须通过安全审查。默认在当前会话的临时目录执行；working_directory 可选择已绑定工程/成果目录。TMPDIR 和 audit.json 始终留在会话目录。这是目录约定与审查策略，不是操作系统沙箱。禁止 cd 和少量高危系统命令。命令超过 yield_time_ms 未结束会转后台并返回 run_id，用 wait_run_id 继续等待。"
    } else {
        "在 macOS 本地 zsh 环境执行命令。命令执行前会经过安全审查（未配置审查模型时拒绝执行）。命令固定运行于当前会话工作区的 .jkcodingagent/local_env/zsh，产物也应写入该目录；工具会维护 audit.json，记录最新 20 条命令、结果和执行会话。禁止 cd 和少量高危系统命令。命令超过 yield_time_ms 未结束会转后台并返回 run_id，用 wait_run_id 继续等待。"
    };
    let parameters = with_compression_parameters(
        parameters,
        false,
        COMMAND_FORCE_COMPRESS_AFTER_CHARS,
        "local_zsh 返回命令输出与审计历史；默认保留原文，需要从超长输出中确认特定信息时可开启压缩并写明 compress_intent。",
    );
    PortableDynamicTool::new("local_zsh", description, parameters, move |args| {
        let workspace = workspace.clone();
        let chat_workspace = chat_workspace.clone();
        let workspace_id = workspace_id.clone();
        let cancel_rx = crate::agent::rig_ext::r#loop::invocation::ToolInvocationContext::current()
            .map(|context| context.cancel_rx)
            .or_else(|| cancel_rx.clone());
        let review = review.clone();
        let app_handle = app_handle.clone();
        Box::pin(async move {
            run::run_local_zsh(
                &args,
                workspace,
                chat_workspace,
                workspace_id,
                exec_timeout_secs,
                cancel_rx,
                review,
                app_handle,
            )
            .await
            .map(ToolOutput::text)
        })
    })
}

/// 本命令的安全审查（fail-closed）：未配置审查模型直接拒绝；审查服务异常
/// 同样拒绝。移植自旧实现的 `review_local_command`。
async fn review_local_command(
    args: &Value,
    review_context: &crate::agent::rig_ext::review::RigReviewContext,
    workspace_id: &str,
    workspace: &Path,
    run_dir: &Path,
    command: &str,
) -> Result<crate::ssh_tool::SshAuditReview, String> {
    // fail-closed：未配置审查时对可执行命令默认拦截，不得跳过审查放行。
    let Some(review_config) = review_context.config.as_ref() else {
        return Err(
            "未配置安全审查，已拒绝执行命令。请先在应用设置中配置安全审查模型。".to_string(),
        );
    };

    let payload = local_review_payload(
        args,
        review_context,
        workspace_id,
        workspace,
        run_dir,
        command,
    );

    crate::agent::ssh_review::review_shell_command(review_config, &payload)
        .await
        .map_err(|error| format!("审查服务异常：{error}"))
        .map(|verdict| crate::ssh_tool::SshAuditReview {
            allowed: verdict.allowed,
            user_approved: false,
            reason: verdict.reason,
        })
}

fn local_review_payload(
    args: &Value,
    review_context: &crate::agent::rig_ext::review::RigReviewContext,
    workspace_id: &str,
    workspace: &Path,
    run_dir: &Path,
    command: &str,
) -> crate::agent::ssh_review::CommandReviewPayload {
    review_context.build_payload(
        workspace_id,
        Some(args),
        crate::agent::ssh_review::CommandReviewTarget::LocalZsh {
            workspace_path: workspace.display().to_string(),
            run_dir: run_dir.display().to_string(),
        },
        command.to_string(),
        None,
    )
}

/// 写入「被拦截」审计条目，并返回带「错误：」前缀的拦截响应（含审计明细）。
async fn blocked_command_response(
    run_dir: &Path,
    audit_dir: &Path,
    session_id: &str,
    command: &str,
    review: crate::ssh_tool::SshAuditReview,
    headline: String,
) -> String {
    let mut entry = blocked_audit_entry(session_id, command, review);
    entry.working_directory = Some(run_dir.display().to_string());
    let render_entry = entry.clone();
    let run_dir_for_audit = audit_dir.to_path_buf();
    let session_id_for_history = session_id.to_string();
    let audit_result = tokio::task::spawn_blocking(move || {
        append_audit_entry(&run_dir_for_audit, entry, &session_id_for_history)
    })
    .await;
    let render_entry_now = render_entry.clone();
    match audit_result {
        Ok(Ok(history)) => format!(
            "{headline}\n\n{}",
            render_blocked_entry(run_dir, audit_dir, &render_entry_now, &history)
        ),
        Ok(Err(error)) => format!("{headline}\n\n错误：审计历史写入失败：{error}"),
        Err(error) => format!("{headline}\n\n错误：审计历史写入失败：{error}"),
    }
}

fn render_blocked_entry(
    run_dir: &Path,
    audit_dir: &Path,
    entry: &LocalZshAuditEntry,
    session_history: &[LocalZshAuditEntry],
) -> String {
    render_command_result(
        run_dir,
        audit_dir,
        &entry.command,
        &entry.stdout,
        &entry.stderr,
        entry.exit_code,
        entry.timed_out,
        entry.cancelled,
        entry.duration_ms,
        entry.output_truncated,
        entry.review.as_ref(),
        true,
        session_history,
        MAX_RESULT_CHARS,
    )
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
        working_directory: None,
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

fn blacklist_reason(command: &str) -> Option<&'static str> {
    let normalized = command.to_ascii_lowercase();
    if command_invokes_command(&normalized, "cd") {
        return Some("禁止使用 cd；local_zsh 的执行目录由工具参数和宿主授权确定");
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
    ];

    dangerous_patterns
        .iter()
        .find_map(|(pattern, reason)| normalized.contains(pattern).then_some(*reason));
    // 管道执行远程脚本的真实形态是 "curl <url> | sh"（curl 与管道符之间总有
    // URL），子串模式从未匹配过——按管道段分析判定（与确认门禁硬排除共用）。
    if crate::agent::rig_ext::review_confirm::pipes_remote_script_into_shell(&normalized) {
        return Some("禁止管道执行远程脚本");
    }
    None
}

pub(crate) fn command_invokes_command(command: &str, target: &str) -> bool {
    command
        .split([';', '&', '|', '(', ')', '\n'])
        .filter_map(|segment| segment.split_whitespace().next())
        .any(|token| token == target)
}

mod audit;
mod paths;
mod run;
mod sessions;
use audit::{append_audit_entry, command_contains_ssh, render_command_result};

#[cfg(test)]
mod tests {
    use super::local_zsh_tool;

    #[test]
    fn local_zsh_schema_does_not_expose_runtime_timeout() {
        let tool = local_zsh_tool(
            std::env::temp_dir(),
            None,
            "test-session".to_string(),
            30,
            None,
            crate::agent::rig_ext::review::RigReviewContext::unconfigured(),
            None,
        );
        let parameters = tool.definition().parameters;

        assert!(parameters["properties"].get("command").is_some());
        assert!(parameters["properties"].get("wait_run_id").is_some());
        assert!(parameters["properties"].get("yield_time_ms").is_some());
        assert!(parameters["properties"].get("tty").is_some());
        assert!(parameters["properties"].get("max_output_chars").is_some());
        // 运行时超时仍不由调用声明（exec_timeout_secs 配置口径），schema 不暴露。
        assert!(parameters["properties"].get("timeout").is_none());
        assert!(parameters["properties"].get("working_directory").is_none());
        // command 与 wait_run_id 二选一，由工具内校验（required 不再强制 command）。
        assert!(parameters.get("required").is_none());
    }

    #[test]
    fn chat_schema_and_review_include_authorized_directories_and_actual_cwd() {
        let chat = super::ChatWorkspace {
            session_root: "/session".into(),
            project_roots: vec!["/shared/project".into()],
        };
        let mut review = crate::agent::rig_ext::review::RigReviewContext::unconfigured();
        review.review_conversation = Some("用户原始任务".into());
        let review = review.with_chat_workspace(&chat);
        let tool = local_zsh_tool(
            chat.session_root.clone(),
            Some(chat.clone()),
            "review-boundary-test".into(),
            30,
            None,
            review.clone(),
            None,
        );
        assert!(tool.definition().parameters["properties"]
            .get("working_directory")
            .is_some());
        let payload = super::local_review_payload(
            &serde_json::json!({"command": "pwd"}),
            &review,
            "review-boundary-test",
            &chat.session_root,
            &chat.project_roots[0],
            "pwd",
        );
        let conversation = payload.conversation.unwrap();
        assert!(conversation.contains("用户原始任务"));
        assert!(conversation.contains("/session"));
        assert!(conversation.contains("/shared/project"));
        assert_eq!(
            conversation,
            format!("用户原始任务{}", chat.render_prompt())
        );
        assert!(matches!(
            payload.target,
            crate::agent::ssh_review::CommandReviewTarget::LocalZsh { run_dir, .. }
                if run_dir == "/shared/project"
        ));
    }
}
