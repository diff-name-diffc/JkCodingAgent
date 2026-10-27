use rig::tool::{PortableDynamicTool, ToolExecutionError};
use serde_json::{json, Value};

use super::super::common::{boolish_arg, string_arg, string_array_arg, u64_arg};
use super::{json_result, other, LogToolDeps};
use crate::agent::command_history::{self, CommandHistoryStatus};
use crate::agent::log_snapshot::model::{ssh_session_id, LogSource};
use crate::agent::rig_ext::tools::common::resolve_path;
use crate::agent::ssh_review::command_gate::{authorize_ssh_command, SshCommandGateRequest};
use crate::ssh_tool::SshExecResult;

pub(super) fn discover_tool(deps: LogToolDeps) -> PortableDynamicTool {
    PortableDynamicTool::new(
        "ssh_log_discover",
        "发现日志来源但不读取正文：列出 SSH 服务器容器，并检查远端/本地日志文件是否存在及大小。返回的 source 可直接用于 ssh_log_snapshot_update。先发现再同步，避免无选择地下载大目录。",
        super::schemas::discover_parameters(),
        move |args| {
            let deps = deps.clone();
            Box::pin(async move {
                let server_id = string_arg(&args, "server_id");
                let remote_files = string_array_arg(&args, "remote_files").unwrap_or_default();
                let local_files = string_array_arg(&args, "local_files").unwrap_or_default();
                if server_id.is_none() && !remote_files.is_empty() {
                    return Err(super::invalid("remote_files 需要同时提供 server_id"));
                }
                let sudo = boolish_arg(&args, "sudo").unwrap_or(false);
                let timeout = u64_arg(&args, "timeout_secs");
                let mut files = discover_local(&deps, local_files).await?;
                if let Some(server) = &server_id {
                    for path in remote_files {
                        let source = LogSource::SshFile {
                            server_id: server.clone(),
                            path,
                            sudo,
                        };
                        source.validate().map_err(super::invalid)?;
                        let stat_command = remote_stat_command(&source)?;
                        match execute_remote(&deps, &args, server, &stat_command, sudo, timeout).await {
                            Ok(result) if result.exit_code == 0 => {
                                let size = result.stdout.trim().strip_prefix("size=")
                                    .and_then(|value| value.trim().parse::<u64>().ok());
                                if let Some(size) = size {
                                    files.push(json!({"source":source,"size_bytes":size}));
                                } else {
                                    files.push(json!({"source":source,"error":"无法解析远端文件大小"}));
                                }
                            }
                            Ok(result) => files.push(json!({
                                "source":source,
                                "error":format!("远端文件不可读取（exit={}）：{}",result.exit_code,result.stderr)
                            })),
                            Err(error) => return Err(error),
                        }
                    }
                }
                let mut containers = Vec::new();
                let mut containers_truncated = false;
                if let Some(server) = server_id.filter(|_| {
                    boolish_arg(&args, "include_docker").unwrap_or(true)
                }) {
                    let command = "docker ps -a --no-trunc --format '{{json .}}'";
                    let result = execute_remote(&deps, &args, &server, command, sudo, timeout).await?;
                    if result.exit_code != 0 || result.external_state_unknown {
                        return Err(other(format!(
                            "Docker 容器发现失败（exit={}）：{}",
                            result.exit_code, result.stderr
                        )));
                    }
                    let lines = result.stdout.lines().collect::<Vec<_>>();
                    containers_truncated = lines.len() > 200 || result.truncated;
                    for line in lines.into_iter().take(200) {
                        let Ok(value) = serde_json::from_str::<Value>(line) else {
                            continue;
                        };
                        let id = value.get("ID").and_then(Value::as_str).unwrap_or_default();
                        let name = value.get("Names").and_then(Value::as_str).unwrap_or(id);
                        if id.is_empty() {
                            continue;
                        }
                        containers.push(json!({
                            "id":id,
                            "name":name,
                            "image":value.get("Image").and_then(Value::as_str),
                            "state":value.get("State").and_then(Value::as_str),
                            "status":value.get("Status").and_then(Value::as_str),
                            "source":LogSource::Docker {
                                server_id:server.clone(),
                                container:id.to_string(),
                                sudo,
                            }
                        }));
                    }
                }
                json_result(json!({
                    "containers":containers,
                    "containers_truncated":containers_truncated,
                    "files":files,
                    "next":"确认来源后用 ssh_log_snapshot_update 同步并自动建索引"
                }))
            })
        },
    )
}

async fn discover_local(
    deps: &LogToolDeps,
    paths: Vec<String>,
) -> Result<Vec<Value>, ToolExecutionError> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let root = deps.session_root.clone();
    let extra = deps.extra_allowed_dirs.clone();
    tokio::task::spawn_blocking(move || {
        let mut results = Vec::with_capacity(paths.len());
        for raw in paths {
            let resolved = resolve_path(&root, true, &extra, &raw).and_then(|path| {
                let path = path.canonicalize().map_err(|error| error.to_string())?;
                let path = resolve_path(&root, true, &extra, &path.to_string_lossy())?;
                let metadata = std::fs::metadata(&path).map_err(|error| error.to_string())?;
                if !metadata.is_file() {
                    return Err("不是普通文件".into());
                }
                // 与 update 同一份文本性预检：发现阶段就把压缩/二进制文件标为
                // 不可用，避免「发现可用、同步必败」的自相矛盾。
                crate::agent::log_snapshot::update::validate_text_file(&path)
                    .map_err(|failure| failure.message)?;
                Ok((path, metadata.len()))
            });
            match resolved {
                Ok((path, size)) => results.push(json!({
                    "source":LogSource::LocalFile { path:path.to_string_lossy().into_owned() },
                    "size_bytes":size
                })),
                Err(error) => results.push(json!({"path":raw,"error":error})),
            }
        }
        Ok::<_, String>(results)
    })
    .await
    .map_err(|error| other(format!("本地日志发现任务失败：{error}")))?
    .map_err(other)
}

fn remote_stat_command(source: &LogSource) -> Result<String, ToolExecutionError> {
    let LogSource::SshFile { path, .. } = source else {
        return Err(super::invalid("远端文件来源类型不匹配"));
    };
    let quoted = crate::ssh_tool::shell_single_quote(path);
    Ok(format!(
        "if [ -f {quoted} ]; then printf 'size='; wc -c < {quoted}; else printf 'not a regular file\\n' >&2; exit 3; fi"
    ))
}

async fn execute_remote(
    deps: &LogToolDeps,
    args: &Value,
    server_id: &str,
    command: &str,
    sudo: bool,
    timeout: Option<u64>,
) -> Result<SshExecResult, ToolExecutionError> {
    let session_id = ssh_session_id(&deps.workspace_id, "discovery");
    let review = authorize_ssh_command(SshCommandGateRequest {
        manager: &deps.ssh_manager,
        workspace: &deps.session_root,
        workspace_id: &deps.workspace_id,
        session_title: &deps.review.session_title,
        server_id,
        session_id: &session_id,
        tool_name: "ssh_log_discover",
        command,
        stdin: None,
        elevate: sudo,
        args,
        review_context: &deps.review,
        app_handle: deps.app_handle.clone(),
        cancel_rx: deps.effective_cancel(),
    })
    .await?;
    let result = deps
        .ssh_manager
        .execute(
            deps.session_root.clone(),
            deps.workspace_id.clone(),
            deps.review.session_title.clone(),
            server_id.to_string(),
            session_id,
            command.to_string(),
            None,
            timeout,
            deps.effective_cancel(),
            review,
            sudo,
        )
        .await
        .map_err(|failure| other(failure.message))?;
    command_history::record(
        &deps.workspace_id,
        "ssh_log_discover",
        server_id,
        command,
        CommandHistoryStatus::Executed,
        &format!("exit={} truncated={}", result.exit_code, result.truncated),
    );
    if result.cancelled || result.external_state_unknown {
        return Err(other(format!(
            "远端日志发现未完成（exit={}）：{}",
            result.exit_code, result.stderr
        )));
    }
    Ok(result)
}
