//! 简单浏览器动作工具：点击、输入、按键、等待、关闭。
//!
//! 这些工具只做单一 sidecar 命令转发与结果/错误投影；
//! 导航（open_url）、快照读取（read_text）与视觉分析见入口模块。

use async_trait::async_trait;
use serde_json::{json, Value};
use tauri::Manager;

use super::recovery::handle_browser_error;
use super::snapshot::invalidate_cached_snapshot;
use super::{browser_value_result, run_browser_command, run_browser_command_value, timeout_arg};
use crate::agent::tools::builtin::common::{string_arg, with_compression_parameters};
use crate::agent::tools::context::ToolContext;
use crate::agent::tools::registry::AgentTool;
use crate::agent::tools::ToolResult;
use crate::browser::BrowserManager;

pub(super) struct ClickTool;
pub(super) struct TypeTool;
pub(super) struct PressTool;
pub(super) struct WaitForTool;
pub(super) struct CloseTool;

#[async_trait]
impl AgentTool for ClickTool {
    fn name(&self) -> &'static str {
        "browser_click"
    }

    fn description(&self) -> &'static str {
        "点击 Accessibility Tree 快照中的元素 ref。先调用 browser_read_text 获取页面快照，再使用快照中标注的 ref。"
    }

    fn parameters(&self) -> Value {
        with_compression_parameters(
            json!({
                "type": "object",
                "properties": {
                    "ref": { "type": "string", "description": "browser_read_text 返回的元素 ref，例如 r12" },
                    "timeout": { "type": "integer", "description": "超时时间，单位毫秒，默认 60000", "minimum": 1 }
                },
                "required": ["ref"]
            }),
            false,
            "点击结果很短，默认关闭压缩。",
        )
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> ToolResult {
        let Some(ref_id) = string_arg(args, "ref") else {
            return ToolResult::recoverable_error(
                "错误：缺少必填参数 ref；请先调用 browser_read_text 获取元素 ref",
            );
        };
        match run_browser_command_value(
            context,
            "click",
            json!({
                "ref": ref_id,
                "timeout": timeout_arg(args)
            }),
        )
        .await
        {
            Ok(value) => browser_value_result(value),
            Err(error) => ToolResult::from_text(handle_browser_error(context, error).await),
        }
    }
}

#[async_trait]
impl AgentTool for TypeTool {
    fn name(&self) -> &'static str {
        "browser_type"
    }

    fn description(&self) -> &'static str {
        "点击指定输入元素并输入文本。"
    }

    fn parameters(&self) -> Value {
        with_compression_parameters(
            json!({
                "type": "object",
                "properties": {
                    "ref": { "type": "string", "description": "browser_read_text 返回的输入元素 ref，例如 r12" },
                    "text": { "type": "string", "description": "要输入的文本" },
                    "timeout": { "type": "integer", "description": "超时时间，单位毫秒，默认 60000", "minimum": 1 }
                },
                "required": ["ref", "text"]
            }),
            false,
            "输入结果很短，默认关闭压缩。",
        )
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> ToolResult {
        let Some(ref_id) = string_arg(args, "ref") else {
            return ToolResult::recoverable_error(
                "错误：缺少必填参数 ref；请先调用 browser_read_text 获取输入元素 ref",
            );
        };
        let Some(text) = string_arg(args, "text") else {
            return ToolResult::recoverable_error("错误：缺少必填参数 text");
        };
        match run_browser_command_value(
            context,
            "type",
            json!({ "ref": ref_id, "text": text, "timeout": timeout_arg(args) }),
        )
        .await
        {
            Ok(value) => browser_value_result(value),
            Err(error) => ToolResult::from_text(handle_browser_error(context, error).await),
        }
    }
}

#[async_trait]
impl AgentTool for PressTool {
    fn name(&self) -> &'static str {
        "browser_press"
    }

    fn description(&self) -> &'static str {
        "在 CloakBrowser 当前页面发送键盘按键，例如 Enter、Escape、Meta+L。"
    }

    fn parameters(&self) -> Value {
        with_compression_parameters(
            json!({
                "type": "object",
                "properties": {
                    "key": { "type": "string", "description": "Playwright 按键名称，例如 Enter" }
                },
                "required": ["key"]
            }),
            false,
            "按键结果很短，默认关闭压缩。",
        )
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> ToolResult {
        let Some(key) = string_arg(args, "key") else {
            return ToolResult::recoverable_error("错误：缺少必填参数 key");
        };
        run_browser_command(context, "press", json!({ "key": key })).await
    }
}

#[async_trait]
impl AgentTool for WaitForTool {
    fn name(&self) -> &'static str {
        "browser_wait_for"
    }

    fn description(&self) -> &'static str {
        "等待当前页面进入指定 load_state。元素定位统一通过 browser_read_text 的 ref 快照完成。"
    }

    fn parameters(&self) -> Value {
        with_compression_parameters(
            json!({
                "type": "object",
                "properties": {
                    "load_state": {
                        "type": "string",
                        "description": "Playwright load state",
                        "enum": ["load", "domcontentloaded", "networkidle"]
                    },
                    "timeout": { "type": "integer", "description": "超时时间，单位毫秒，默认 60000", "minimum": 1 }
                }
            }),
            false,
            "等待结果很短，默认关闭压缩。",
        )
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> ToolResult {
        run_browser_command(
            context,
            "wait_for",
            json!({
                "loadState": string_arg(args, "load_state").unwrap_or_else(|| "domcontentloaded".to_string()),
                "timeout": timeout_arg(args)
            }),
        )
        .await
    }
}

#[async_trait]
impl AgentTool for CloseTool {
    fn name(&self) -> &'static str {
        "browser_close"
    }

    fn description(&self) -> &'static str {
        "关闭当前 Dispatcher 会话的 CloakBrowser。"
    }

    fn parameters(&self) -> Value {
        with_compression_parameters(
            json!({ "type": "object", "properties": {} }),
            false,
            "关闭结果很短，默认关闭压缩。",
        )
    }

    async fn execute(&self, _args: &Value, context: &ToolContext) -> ToolResult {
        let Some(app) = context.app_handle.clone() else {
            return ToolResult::recoverable_error(
                "错误：浏览器工具缺少 Tauri AppHandle，无法访问 CloakBrowser 管理器",
            );
        };
        let manager = app.state::<BrowserManager>();
        match manager.stop(&context.workspace_id).await {
            Ok(()) => {
                // 浏览器已停：缓存快照与 sidecar 的 ref 映射一并失效，同步丢弃。
                invalidate_cached_snapshot(&context.workspace_id);
                ToolResult::success_data(
                    json!({ "closed": true }),
                    "CloakBrowser 已关闭",
                    "CloakBrowser 已关闭",
                )
            }
            Err(error) => ToolResult::recoverable_error(format!("错误：{error}")),
        }
    }
}
