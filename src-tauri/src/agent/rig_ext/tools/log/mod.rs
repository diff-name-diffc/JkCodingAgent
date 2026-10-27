mod discovery;
mod schemas;

use rig::tool::{PortableDynamicTool, ToolExecutionError, ToolOutput};
use serde::Serialize;
use serde_json::Value;

use super::common::{boolish_arg, string_arg, string_list_arg, u64_arg, usize_arg};
use super::deps::RigToolDeps;
use crate::agent::log_snapshot::model::UpdateOptions;
use crate::agent::log_snapshot::query::{
    LogReadRequest, LogSearchRequest, MatchMode, OutputMode, SearchEngine,
};
use crate::agent::log_snapshot::store::SnapshotStore;
use crate::agent::log_snapshot::update::UpdateContext;
use crate::agent::log_snapshot::LogSnapshotManager;

/// 日志工具面全部工具名：指引注入与测试共用同一份名单。
pub(crate) const LOG_TOOL_NAMES: [&str; 6] = [
    "ssh_log_discover",
    "ssh_log_snapshot_update",
    "ssh_log_snapshot_status",
    "ssh_log_snapshot_cancel",
    "ssh_log_search",
    "ssh_log_read",
];

#[derive(Clone)]
struct LogToolDeps {
    workspace_id: String,
    session_root: std::path::PathBuf,
    extra_allowed_dirs: Vec<std::path::PathBuf>,
    ssh_manager: crate::ssh_tool::SshSessionManager,
    review: crate::agent::rig_ext::review::RigReviewContext,
    app_handle: Option<tauri::AppHandle>,
    cancel_rx: Option<tokio::sync::watch::Receiver<bool>>,
    manager: LogSnapshotManager,
}

impl LogToolDeps {
    fn from_deps(deps: &RigToolDeps) -> Option<Self> {
        Some(Self {
            workspace_id: deps.workspace_id.clone(),
            session_root: deps.chat_workspace.as_ref()?.session_root.clone(),
            extra_allowed_dirs: deps.extra_allowed_dirs.clone(),
            ssh_manager: deps.ssh_manager.clone(),
            review: deps.review.clone(),
            app_handle: deps.app_handle.clone(),
            cancel_rx: deps.cancel_rx.clone(),
            manager: deps.log_snapshots.clone()?,
        })
    }

    /// 本次调用的取消通道：策略层（统一超时/兜底上限）为每次调用新建 watch
    /// 通道并写入 ToolInvocationContext，装配期捕获的 `cancel_rx` 是 run 级通道
    /// 收不到该信号——必须与 local_zsh 相同在闭包内解析（见 exec/local_zsh.rs）。
    fn effective_cancel(&self) -> Option<tokio::sync::watch::Receiver<bool>> {
        crate::agent::rig_ext::r#loop::invocation::ToolInvocationContext::current()
            .map(|context| context.cancel_rx)
            .or_else(|| self.cancel_rx.clone())
    }

    fn store(&self) -> SnapshotStore {
        SnapshotStore::new(self.session_root.clone(), self.workspace_id.clone())
    }

    fn update_context(&self) -> UpdateContext {
        UpdateContext {
            workspace_id: self.workspace_id.clone(),
            session_root: self.session_root.clone(),
            extra_allowed_dirs: self.extra_allowed_dirs.clone(),
            ssh_manager: self.ssh_manager.clone(),
            review_context: self.review.clone(),
            app_handle: self.app_handle.clone(),
        }
    }
}

pub(crate) fn log_tools(deps: &RigToolDeps) -> Vec<PortableDynamicTool> {
    let Some(deps) = LogToolDeps::from_deps(deps) else {
        return Vec::new();
    };
    vec![
        discovery::discover_tool(deps.clone()),
        update_tool(deps.clone()),
        status_tool(deps.clone()),
        cancel_tool(deps.clone()),
        search_tool(deps.clone()),
        read_tool(deps),
    ]
}

fn update_tool(deps: LogToolDeps) -> PortableDynamicTool {
    PortableDynamicTool::new(
        "ssh_log_snapshot_update",
        "创建或覆盖更新日志快照并自动建立 FTS5 全文索引。支持 docker、ssh_file、local_file 多来源；后台执行，返回 snapshotId/jobId。首次必须给 sources，后续同 snapshot_id 可省略 sources 复用来源。只有全部同步和索引成功才切换 generation，失败保留上一代。先用 ssh_log_snapshot_status 等待 ready，再搜索。",
        schemas::update_parameters(),
        move |args| {
            let deps = deps.clone();
            Box::pin(async move {
                let options = parse_update_options(&args)?;
                let record = deps
                    .manager
                    .start_update(deps.update_context(), options)
                    .await
                    .map_err(other)?;
                json_result(record)
            })
        },
    )
}

fn status_tool(deps: LogToolDeps) -> PortableDynamicTool {
    PortableDynamicTool::new(
        "ssh_log_snapshot_status",
        "查看日志快照当前 generation 和更新进度；省略 snapshot_id 列出当前会话全部快照。wait_secs 可有界等待更新阶段变化。update.state=ready 且 current 存在时才表示可检索。",
        schemas::status_parameters(),
        move |args| {
            let deps = deps.clone();
            Box::pin(async move {
                let wait = u64_arg(&args, "wait_secs").unwrap_or(0).min(30);
                if let Some(snapshot_id) = string_arg(&args, "snapshot_id") {
                    let status = deps
                        .manager
                        .status(deps.store(), &snapshot_id, wait)
                        .await
                        .map_err(other)?;
                    json_result(status)
                } else {
                    let statuses = deps.manager.list(deps.store()).await.map_err(other)?;
                    json_result(serde_json::json!({"snapshots": statuses}))
                }
            })
        },
    )
}

fn cancel_tool(deps: LogToolDeps) -> PortableDynamicTool {
    PortableDynamicTool::new(
        "ssh_log_snapshot_cancel",
        "取消正在进行的日志快照更新，不删除或回滚已发布 generation；没有活动任务时返回 cancelled=false。",
        schemas::cancel_parameters(),
        move |args| {
            let deps = deps.clone();
            Box::pin(async move {
                let snapshot_id = required_string(&args, "snapshot_id")?;
                let cancelled = deps
                    .manager
                    .cancel(&deps.workspace_id, &snapshot_id)
                    .map_err(other)?;
                json_result(serde_json::json!({
                    "snapshot_id": snapshot_id,
                    "cancelled": cancelled,
                    "current_generation_preserved": true
                }))
            })
        },
    )
}

fn search_tool(deps: LogToolDeps) -> PortableDynamicTool {
    PortableDynamicTool::new(
        "ssh_log_search",
        "在已就绪日志快照中精准搜索。推荐先 output_mode=files/count 收敛，再 matches 获取 file_id/line_number/byte_offset，最后 ssh_log_read 精读。fixed 且长度至少 3 字符自动用 FTS5 索引，regex/短模式自动用 ripgrep；scan_complete 与 results_truncated 必须分开判断。更新后基于新 generation 重新搜索。",
        schemas::search_parameters(),
        move |args| {
            let deps = deps.clone();
            Box::pin(async move {
                let request = parse_search_request(&args)?;
                let result = deps
                    .manager
                    .search(deps.store(), request, deps.effective_cancel())
                    .await
                    .map_err(other)?;
                json_result(result)
            })
        },
    )
}

fn read_tool(deps: LogToolDeps) -> PortableDynamicTool {
    PortableDynamicTool::new(
        "ssh_log_read",
        "按日志搜索命中的 file_id + line_number 读取前后文，或按 byte_offset 读取原始字节区段。不要传任意文件路径。长行用 byte_offset/next_byte_offset 分页；行分页用 next_line、before=0。单次只返回有限证据。",
        schemas::read_parameters(),
        move |args| {
            let deps = deps.clone();
            Box::pin(async move {
                let request = LogReadRequest {
                    snapshot_id: required_string(&args, "snapshot_id")?,
                    file_id: u64_arg(&args, "file_id")
                        .ok_or_else(|| invalid("缺少合法 file_id"))?,
                    line_number: u64_arg(&args, "line_number"),
                    before: usize_arg(&args, "before").unwrap_or(0),
                    after: usize_arg(&args, "after").unwrap_or(99),
                    byte_offset: u64_arg(&args, "byte_offset"),
                    max_bytes: usize_arg(&args, "max_bytes").unwrap_or(65_536),
                    max_line_chars: usize_arg(&args, "max_line_chars").unwrap_or(2_000),
                };
                let result = deps
                    .manager
                    .read(deps.store(), request)
                    .await
                    .map_err(other)?;
                json_result(result)
            })
        },
    )
}

fn parse_update_options(args: &Value) -> Result<UpdateOptions, ToolExecutionError> {
    let sources = match args.get("sources") {
        Some(value) => serde_json::from_value(value.clone())
            .map_err(|error| invalid(format!("sources 格式无效：{error}")))?,
        None => Vec::new(),
    };
    Ok(UpdateOptions {
        snapshot_id: string_arg(args, "snapshot_id"),
        label: string_arg(args, "label"),
        sources,
        since: string_arg(args, "since"),
        until: string_arg(args, "until"),
        tail: u64_arg(args, "tail"),
        max_total_bytes: u64_arg(args, "max_total_bytes")
            .unwrap_or(crate::agent::log_snapshot::model::DEFAULT_MAX_TOTAL_BYTES),
        timeout_secs: u64_arg(args, "timeout_secs")
            .unwrap_or(crate::agent::log_snapshot::model::DEFAULT_UPDATE_TIMEOUT_SECS),
    })
}

fn parse_search_request(args: &Value) -> Result<LogSearchRequest, ToolExecutionError> {
    let patterns = string_list_arg(args, "pattern", "patterns").map_err(invalid)?;
    let match_mode = match string_arg(args, "match_mode").as_deref() {
        Some("regex") => MatchMode::Regex,
        _ => MatchMode::Fixed,
    };
    let output_mode = match string_arg(args, "output_mode").as_deref() {
        Some("files") => OutputMode::Files,
        Some("count") => OutputMode::Count,
        _ => OutputMode::Matches,
    };
    let engine = match string_arg(args, "engine").as_deref() {
        Some("scan") => SearchEngine::Scan,
        _ => SearchEngine::Auto,
    };
    let file_ids = args
        .get("file_ids")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .map(|value| {
                    value
                        .as_u64()
                        .ok_or_else(|| invalid("file_ids 必须为整数数组"))
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?
        .unwrap_or_default();
    Ok(LogSearchRequest {
        snapshot_id: required_string(args, "snapshot_id")?,
        patterns,
        match_mode,
        case_sensitive: boolish_arg(args, "case_sensitive").unwrap_or(false),
        output_mode,
        engine,
        file_ids,
        context_before: usize_arg(args, "context_before").unwrap_or(0),
        context_after: usize_arg(args, "context_after").unwrap_or(0),
        max_matches: usize_arg(args, "max_matches").unwrap_or(50),
        max_files: usize_arg(args, "max_files").unwrap_or(50),
        max_line_chars: usize_arg(args, "max_line_chars").unwrap_or(2_000),
        cursor: string_arg(args, "cursor"),
    })
}

fn required_string(args: &Value, key: &str) -> Result<String, ToolExecutionError> {
    string_arg(args, key).ok_or_else(|| invalid(format!("缺少必填参数 {key}")))
}

fn json_result(value: impl Serialize) -> Result<ToolOutput, ToolExecutionError> {
    serde_json::to_string_pretty(&value)
        .map(ToolOutput::text)
        .map_err(|error| other(format!("序列化日志工具结果失败：{error}")))
}

fn invalid(message: impl Into<String>) -> ToolExecutionError {
    let message = message.into();
    ToolExecutionError::invalid_args(if message.starts_with("错误：") {
        message
    } else {
        format!("错误：{message}")
    })
}

fn other(message: impl Into<String>) -> ToolExecutionError {
    let message = message.into();
    ToolExecutionError::other(if message.starts_with("错误：") {
        message
    } else {
        format!("错误：{message}")
    })
}

#[cfg(test)]
mod tests;
