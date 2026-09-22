//! sync_directory 工具：rsync over SSH 目录同步（路径沙箱 + 取消 + 审计）。
//! 移植自旧 `tools/builtin/sync_directory.rs`；rsync 编排保留在 `ssh_tool::sync`。

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};
use tokio::sync::watch;

use super::super::common::resolve_path;
use crate::agent::command_history::{self, CommandHistoryStatus};
use crate::agent::db::DispatcherDb;
use crate::ssh_tool::sync::{SyncDirectory, SyncResult};
use crate::ssh_tool::{SshAuditRecord, SshSessionManager};
use rig::tool::{PortableDynamicTool, ToolExecutionError, ToolOutput};

#[allow(clippy::too_many_arguments)]
pub(super) fn sync_directory_tool(
    manager: SshSessionManager,
    workspace: PathBuf,
    workspace_id: String,
    restrict_to_workspace: bool,
    extra_allowed_dirs: Vec<PathBuf>,
    app_handle: Option<AppHandle>,
    db: DispatcherDb,
    cancel_rx: Option<watch::Receiver<bool>>,
) -> PortableDynamicTool {
    PortableDynamicTool::new(
        "sync_directory",
        "用 rsync over SSH 将本地目录内容同步到已配置服务器的指定绝对目录。ssh_profile 是 ssh_list_servers 返回的 id。复用密码/私钥及主机指纹校验；执行前安全审查。默认不删除远端多余文件，dry_run=true 仅预演（仍需连接服务器）。只上传目录内容，不额外嵌套源目录名；保留时间、权限及安全符号链接，不跟随符号链接读取目录外文件；排除 .git/ 和 .jkcodingagent/。要求本机 rsync 3.1+、OpenSSH，远端 rsync 3.0+，目标父目录须存在。不提权、传输不自动重试；超时使用服务器配置（最大300秒），中止/失败可能已同步部分文件。",
        json!({
            "type":"object", "additionalProperties":false,
            "properties":{
                "ssh_profile":{"type":"string","minLength":1,"description":"已启用 SSH 服务器的 id，来自 ssh_list_servers"},
                "source":{"type":"string","minLength":1,"description":"本地已有目录，受工作区/额外允许目录权限约束"},
                "destination":{"type":"string","minLength":1,"description":"远端绝对 POSIX 目录，不能为 / 或包含 .、.."},
                "delete":{"type":"boolean","default":false,"description":"删除远端目标目录内源端不存在的文件；排除目录不删除"},
                "dry_run":{"type":"boolean","default":false,"description":"仅计算和报告变化，不写入或删除远端文件"}
            },
            "required":["ssh_profile","source","destination"]
        }),
        move |args| {
            let manager = manager.clone();
            let workspace = workspace.clone();
            let workspace_id = workspace_id.clone();
            let extra_allowed_dirs = extra_allowed_dirs.clone();
            let app_handle = app_handle.clone();
            let db = db.clone();
            let cancel_rx = cancel_rx.clone();
            Box::pin(async move {
                let cancelled = cancel_rx.clone();
                match execute_inner(
                    &args,
                    manager,
                    workspace,
                    workspace_id,
                    restrict_to_workspace,
                    extra_allowed_dirs,
                    app_handle,
                    db,
                    cancel_rx,
                )
                .await
                {
                    Ok(text) => Ok(ToolOutput::text(text)),
                    Err(SyncFailure::Cancelled(text)) => {
                        Err(ToolExecutionError::cancelled(text))
                    }
                    // run 级取消优先于一切错误形态（语义同旧 ToolResult::cancelled 覆盖）。
                    Err(SyncFailure::Recoverable(text))
                        if cancelled.as_ref().is_some_and(|rx| *rx.borrow()) =>
                    {
                        Err(ToolExecutionError::cancelled(ensure_error_prefix(text)))
                    }
                    Err(SyncFailure::Recoverable(text)) => {
                        Ok(ToolOutput::text(ensure_error_prefix(text)))
                    }
                }
            })
        },
    )
}

/// 强制错误消息满足「错误：」前缀约定（对齐旧 `ToolResult::recoverable_error`）。
fn ensure_error_prefix(message: String) -> String {
    let trimmed = message.trim_start();
    if trimmed.starts_with("错误：") {
        message
    } else {
        format!("错误：{message}")
    }
}

/// 同步失败的两种去向：取消走 cancelled 错误信封（run 级语义），
/// 其余为「错误：」开头的可恢复文本（模型可据此自愈）。
enum SyncFailure {
    Cancelled(String),
    Recoverable(String),
}

#[allow(clippy::too_many_arguments)]
async fn execute_inner(
    args: &Value,
    manager: SshSessionManager,
    workspace: PathBuf,
    workspace_id: String,
    restrict_to_workspace: bool,
    extra_allowed_dirs: Vec<PathBuf>,
    app_handle: Option<AppHandle>,
    db: DispatcherDb,
    cancel_rx: Option<watch::Receiver<bool>>,
) -> Result<String, SyncFailure> {
    let mut request: SyncDirectory = serde_json::from_value(args.clone())
        .map_err(|e| SyncFailure::Recoverable(format!("错误：同步参数无效：{e}")))?;
    request
        .validate()
        .map_err(|e| SyncFailure::Recoverable(format!("错误：{e}")))?;
    let source = {
        let workspace = workspace.clone();
        let raw = request.source.clone();
        tokio::task::spawn_blocking(move || {
            resolve_sync_source(&workspace, restrict_to_workspace, &extra_allowed_dirs, &raw)
        })
        .await
        .map_err(|e| SyncFailure::Recoverable(format!("错误：解析源目录任务失败：{e}")))?
        .map_err(SyncFailure::Recoverable)?
    };
    request.source = source
        .to_str()
        .ok_or_else(|| SyncFailure::Recoverable("源目录必须为 UTF-8 路径".to_string()))?
        .into();
    let server = manager
        .server_config_async(request.ssh_profile.clone())
        .await
        .map_err(|e| SyncFailure::Recoverable(format!("错误：{e}")))?;
    let command = request.command_description();

    // TODO(T3)：审查门禁移至 runtime ToolExecutionPolicy——旧实现在此对同步
    // 命令做 fail-closed 安全审查（未配置审查即拦截、服务器可显式豁免、拦截写
    // 审计与命令台账）；迁移后工具层不再审查，审计记录的 review 字段暂为 None。

    // 审计元数据需要会话标题：旧实现由 ToolContext 注入，此处从会话库按
    // workspace_id 现查（查不到回退空串，不阻断同步）。
    let session_title = db
        .get_session_title_async(&workspace_id)
        .await
        .unwrap_or_default();

    let profile = server.id.clone();
    let progress_workspace_id = workspace_id.clone();
    let progress = Arc::new(move |p: crate::ssh_tool::sync::SyncProgress| {
        if let Some(app) = &app_handle {
            // TODO(T3)：toolCallId 由 runtime 策略层按次注入，当前恒为 null。
            if let Err(error) = app.emit(
                "ssh-sync-progress",
                json!({
                    "workspaceId": progress_workspace_id, "toolCallId": null,
                    "sshProfile": profile, "progress": p
                }),
            ) {
                eprintln!("[sync_directory] 发送进度事件失败：{error}");
            }
        }
    });
    let result = manager
        .sync_directory(server, request.clone(), source, cancel_rx, progress)
        .await;
    let record = audit_record(
        &workspace,
        &workspace_id,
        &session_title,
        &request,
        command.clone(),
        &result,
    );
    let audit_error = manager.append_sync_audit(record).await.err();
    let mut result = match result {
        Ok(result) => result,
        Err(error) => {
            command_history::record(
                &workspace_id,
                "sync_directory",
                &request.ssh_profile,
                &command,
                CommandHistoryStatus::Executed,
                &error,
            );
            let error = match audit_error {
                Some(audit) => format!("{error}；写入 SSH 审计失败：{audit}"),
                None => error,
            };
            return Err(SyncFailure::Recoverable(format!("错误：{error}")));
        }
    };
    result.audit_error = audit_error;
    let text = serde_json::to_string_pretty(&result)
        .map_err(|e| SyncFailure::Recoverable(format!("错误：序列化同步结果失败：{e}")))?;
    command_history::record(
        &workspace_id,
        "sync_directory",
        &request.ssh_profile,
        &command,
        CommandHistoryStatus::Executed,
        &text,
    );
    if result.cancelled {
        Err(SyncFailure::Cancelled(format!(
            "目录同步已取消，远端可能已有部分变更。\n{text}"
        )))
    } else if result.timed_out || result.exit_code != Some(0) {
        // 可恢复错误：模型可检查 rsync/OpenSSH 版本与返回信息后重试。
        Err(SyncFailure::Recoverable(format!(
            "错误：目录同步失败，远端可能已有部分变更。请检查 rsync/OpenSSH 版本及返回信息。\n{text}"
        )))
    } else {
        Ok(text)
    }
}

/// 源目录解析与校验（纯函数，便于单测）：工作区沙箱 + 符号链接复核 +
/// 拒绝 .git / 应用配置根目录。移植自旧 execute_inner 的 spawn_blocking 段。
fn resolve_sync_source(
    workspace: &std::path::Path,
    restrict_to_workspace: bool,
    extra_allowed_dirs: &[PathBuf],
    raw_source: &str,
) -> Result<PathBuf, String> {
    let path = resolve_path(workspace, restrict_to_workspace, extra_allowed_dirs, raw_source)?
        .canonicalize()
        .map_err(|e| format!("解析源目录失败：{e}"))?;
    // 即便关闭工作区限制，解析符号链接后也要再次检查敏感路径。
    resolve_path(
        workspace,
        restrict_to_workspace,
        extra_allowed_dirs,
        path.to_str().ok_or("源目录必须为 UTF-8 路径")?,
    )?;
    if !path.is_dir() {
        return Err("source 必须是已有目录".to_string());
    }
    // 会话沙箱及 local_zsh 产物本就位于 .jkcodingagent 下，
    // 不能仅凭祖先目录名拒绝；敏感子路径由 resolve_path 校验。
    if path.components().any(|c| c.as_os_str() == ".git")
        || path.file_name().is_some_and(|name| name == ".jkcodingagent")
        || (path.file_name().is_some_and(|name| name == "local_env")
            && path.parent().and_then(|p| p.file_name()).is_some_and(|name| name == ".jkcodingagent"))
    {
        return Err(format!(
            "不能同步应用配置根目录或 Git 元数据目录：{}。当前工作区：{}。请指定工作区或 local_zsh 下具体的产物子目录。",
            path.display(), workspace.display()
        ));
    }
    Ok(path)
}

fn audit_record(
    workspace: &std::path::Path,
    workspace_id: &str,
    session_title: &str,
    request: &SyncDirectory,
    command: String,
    result: &Result<SyncResult, String>,
) -> SshAuditRecord {
    let output = result.as_ref().ok();
    SshAuditRecord {
        created_at: chrono::Utc::now().to_rfc3339(),
        workspace_path: workspace.to_string_lossy().into_owned(),
        workspace_id: workspace_id.to_string(),
        session_title: session_title.to_string(),
        server_id: request.ssh_profile.clone(),
        session_id: workspace_id.to_string(),
        command,
        exit_code: output.and_then(|r| r.exit_code),
        stdout: output
            .map(|r| r.stdout.chars().take(8000).collect())
            .unwrap_or_default(),
        stderr: output
            .map(|r| r.stderr.chars().take(8000).collect())
            .unwrap_or_default(),
        duration_ms: output.map(|r| r.duration_ms as u128),
        truncated: output.is_some_and(|r| {
            r.truncated || r.stdout.chars().count() > 8000 || r.stderr.chars().count() > 8000
        }),
        interactive_blocked: false,
        error: result.as_ref().err().cloned().or_else(|| {
            output.and_then(|r| {
                if r.cancelled {
                    Some("同步已取消".into())
                } else if r.timed_out {
                    Some("同步超时".into())
                } else if r.exit_code != Some(0) {
                    Some(format!("rsync 非零退出：{:?}", r.exit_code))
                } else {
                    None
                }
            })
        }),
        review: None,
    }
}

#[cfg(test)]
#[path = "sync_directory_tests.rs"]
mod tests;
