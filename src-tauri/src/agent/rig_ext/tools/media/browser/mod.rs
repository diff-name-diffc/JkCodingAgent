//! 通用浏览器工具只负责打开页面；探索委派给 browser-use 子智能体。

mod file_url;

use std::time::Duration;

use rig::tool::{PortableDynamicTool, ToolExecutionError, ToolOutput};
use serde_json::json;
use tauri::Manager;

use super::super::common::{
    string_arg, with_compression_parameters, DEFAULT_FORCE_COMPRESS_AFTER_CHARS,
};
use super::super::deps::RigToolDeps;
use crate::agent::rig_ext::r#loop::invocation::ToolInvocationContext;
use crate::browser::{normalize_browser_url, BrowserManager};

pub(super) fn browser_tools(deps: &RigToolDeps) -> Vec<PortableDynamicTool> {
    let deps = deps.clone();
    vec![PortableDynamicTool::new(
        "browser_open_url",
        "在当前会话的内置浏览器界面打开并展示指定网址。此工具不读取网页、不执行页面交互；网页探索、搜索和自动化任务请通过 call_sub_agent 委派 browser-agent。探索运行中不可手动导航。首次启动包含运行环境准备，最多等待 180 秒；取消或超时会停止浏览器进程。",
        with_compression_parameters(json!({
            "type": "object",
            "properties": { "url": { "type": "string", "description": "要展示的网址；本地 file:// URL 必须位于当前工作目录内。" } },
            "required": ["url"]
        }), false, DEFAULT_FORCE_COMPRESS_AFTER_CHARS, "打开网址仅返回页面状态，通常不需要压缩。"),
        move |args| {
            let deps = deps.clone();
            let cancel_rx = ToolInvocationContext::current()
                .map(|context| context.cancel_rx)
                .or_else(|| deps.cancel_rx.clone());
            Box::pin(async move {
                let raw = string_arg(&args, "url")
                    .ok_or_else(|| ToolExecutionError::invalid_args("错误：url 不能为空"))?;
                let url = normalize_browser_url(raw)
                    .map_err(|error| ToolExecutionError::invalid_args(format!("错误：{error}")))?;
                if url.to_ascii_lowercase().starts_with("file:") {
                    let candidate = url.clone();
                    let workspace = deps.workspace.clone();
                    tokio::task::spawn_blocking(move || {
                        file_url::validate_file_url_within_workspace(&candidate, &workspace)
                    }).await.map_err(|error| ToolExecutionError::other(format!("错误：URL 校验任务失败：{error}")))?
                        .map_err(|error| ToolExecutionError::refused(format!("错误：{error}")))?;
                }
                let app = deps.app_handle.ok_or_else(|| ToolExecutionError::other("错误：浏览器需要桌面应用运行环境"))?;
                let manager = app.state::<BrowserManager>();
                let result = manager.command_with_cancellation(
                    app.clone(), deps.workspace_id, deps.workspace.to_string_lossy().into_owned(),
                    "open_url", json!({"url":url}), cancel_rx, Duration::from_secs(180),
                ).await.map_err(|error| {
                    if error.starts_with("[cancelled]") {
                        ToolExecutionError::cancelled(format!("错误：{error}"))
                    } else if error.starts_with("[timeout]") {
                        ToolExecutionError::timeout(format!("错误：{error}")).with_retryable(false)
                    } else {
                        ToolExecutionError::other(format!("错误：{error}"))
                    }
                })?;
                Ok(ToolOutput::text(result.to_string()))
            })
        },
    )]
}
