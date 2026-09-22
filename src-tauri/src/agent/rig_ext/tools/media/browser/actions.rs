//! 简单浏览器动作工具：点击、输入、按键、等待、关闭。
//!
//! 这些工具只做单一 sidecar 命令转发与结果/错误投影；
//! 导航（open_url）、快照读取（read_text）与视觉分析见入口模块。

use rig::tool::{PortableDynamicTool, ToolExecutionError, ToolOutput};
use serde_json::{json, Value};
use tauri::Manager;

use super::recovery::handle_browser_error;
use super::snapshot::invalidate_cached_snapshot;
use super::{
    browser_value_output, run_browser_command, run_browser_command_value, timeout_arg, BrowserDeps,
};
use crate::agent::rig_ext::tools::common::{
    boolish_arg, string_arg, with_compression_parameters, DEFAULT_FORCE_COMPRESS_AFTER_CHARS,
};
use crate::browser::BrowserManager;

pub(super) fn click_tool(deps: BrowserDeps) -> PortableDynamicTool {
    PortableDynamicTool::new(
        "browser_click",
        "点击 Accessibility Tree 快照中的元素 ref。优先按语义（角色+名称）定位并做可见性/稳定性校验，主文档元素在语义失败时回退坐标点击；iframe 内元素同样支持。先调用 browser_read_text 获取页面快照，再使用快照中标注的 ref。",
        with_compression_parameters(
            json!({
                "type": "object",
                "properties": {
                    "ref": { "type": "string", "description": "browser_read_text 返回的元素 ref，例如 r12" },
                    "humanize": { "type": "boolean", "description": "是否启用反爬拟人化（模拟人类鼠标轨迹与节奏）。默认关闭；仅当页面出现人机验证、行为检测拦截（如点击无效、提示异常流量、验证码）时才开启——开启后本会话内浏览器动作会显著变慢", "default": false },
                    "timeout": { "type": "integer", "description": "超时时间，单位毫秒，默认 60000", "minimum": 1 }
                },
                "required": ["ref"]
            }),
            false,
            DEFAULT_FORCE_COMPRESS_AFTER_CHARS,
            "点击结果很短，默认关闭压缩。",
        ),
        move |args: Value| {
            let deps = deps.clone();
            Box::pin(async move {
                let Some(ref_id) = string_arg(&args, "ref") else {
                    return Err(ToolExecutionError::invalid_args(
                        "错误：缺少必填参数 ref；请先调用 browser_read_text 获取元素 ref",
                    ));
                };
                match run_browser_command_value(
                    &deps,
                    "click",
                    json!({
                        "ref": ref_id,
                        "humanize": boolish_arg(&args, "humanize").unwrap_or(false),
                        "timeout": timeout_arg(&args)
                    }),
                )
                .await
                {
                    Ok(value) => browser_value_output(value),
                    Err(error) => Err(ToolExecutionError::other(
                        handle_browser_error(&deps, error).await,
                    )),
                }
            })
        },
    )
}

pub(super) fn type_tool(deps: BrowserDeps) -> PortableDynamicTool {
    PortableDynamicTool::new(
        "browser_type",
        "向指定输入元素输入文本（真实按键序列，追加不清空）。按语义（角色+名称）定位输入框，iframe 内输入框同样支持。",
        with_compression_parameters(
            json!({
                "type": "object",
                "properties": {
                    "ref": { "type": "string", "description": "browser_read_text 返回的输入元素 ref，例如 r12" },
                    "text": { "type": "string", "description": "要输入的文本" },
                    "humanize": { "type": "boolean", "description": "是否启用反爬拟人化（人类打字节奏、随机停顿与误触纠正）。默认关闭；仅当页面出现人机验证、行为检测拦截时才开启——开启后本会话内浏览器动作会显著变慢", "default": false },
                    "timeout": { "type": "integer", "description": "超时时间，单位毫秒，默认 60000", "minimum": 1 }
                },
                "required": ["ref", "text"]
            }),
            false,
            DEFAULT_FORCE_COMPRESS_AFTER_CHARS,
            "输入结果很短，默认关闭压缩。",
        ),
        move |args: Value| {
            let deps = deps.clone();
            Box::pin(async move {
                let Some(ref_id) = string_arg(&args, "ref") else {
                    return Err(ToolExecutionError::invalid_args(
                        "错误：缺少必填参数 ref；请先调用 browser_read_text 获取输入元素 ref",
                    ));
                };
                let Some(text) = string_arg(&args, "text") else {
                    return Err(ToolExecutionError::invalid_args("错误：缺少必填参数 text"));
                };
                match run_browser_command_value(
                    &deps,
                    "type",
                    json!({
                        "ref": ref_id,
                        "text": text,
                        "humanize": boolish_arg(&args, "humanize").unwrap_or(false),
                        "timeout": timeout_arg(&args)
                    }),
                )
                .await
                {
                    Ok(value) => browser_value_output(value),
                    Err(error) => Err(ToolExecutionError::other(
                        handle_browser_error(&deps, error).await,
                    )),
                }
            })
        },
    )
}

pub(super) fn press_tool(deps: BrowserDeps) -> PortableDynamicTool {
    PortableDynamicTool::new(
        "browser_press",
        "在浏览器当前页面发送键盘按键，例如 Enter、Escape、Meta+L。",
        with_compression_parameters(
            json!({
                "type": "object",
                "properties": {
                    "key": { "type": "string", "description": "Playwright 按键名称，例如 Enter" },
                    "humanize": { "type": "boolean", "description": "是否启用反爬拟人化。默认关闭；仅当页面出现人机验证、行为检测拦截时才开启——开启后本会话内浏览器动作会显著变慢", "default": false }
                },
                "required": ["key"]
            }),
            false,
            DEFAULT_FORCE_COMPRESS_AFTER_CHARS,
            "按键结果很短，默认关闭压缩。",
        ),
        move |args: Value| {
            let deps = deps.clone();
            Box::pin(async move {
                let Some(key) = string_arg(&args, "key") else {
                    return Err(ToolExecutionError::invalid_args("错误：缺少必填参数 key"));
                };
                run_browser_command(
                    &deps,
                    "press",
                    json!({
                        "key": key,
                        "humanize": boolish_arg(&args, "humanize").unwrap_or(false)
                    }),
                )
                .await
            })
        },
    )
}

pub(super) fn wait_for_tool(deps: BrowserDeps) -> PortableDynamicTool {
    PortableDynamicTool::new(
        "browser_wait_for",
        "等待当前页面进入指定 load_state。元素定位统一通过 browser_read_text 的 ref 快照完成。",
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
            DEFAULT_FORCE_COMPRESS_AFTER_CHARS,
            "等待结果很短，默认关闭压缩。",
        ),
        move |args: Value| {
            let deps = deps.clone();
            Box::pin(async move {
                run_browser_command(
                    &deps,
                    "wait_for",
                    json!({
                        "loadState": string_arg(&args, "load_state").unwrap_or_else(|| "domcontentloaded".to_string()),
                        "timeout": timeout_arg(&args)
                    }),
                )
                .await
            })
        },
    )
}

pub(super) fn close_tool(deps: BrowserDeps) -> PortableDynamicTool {
    PortableDynamicTool::new(
        "browser_close",
        "关闭当前 Dispatcher 会话的浏览器。",
        with_compression_parameters(
            json!({ "type": "object", "properties": {} }),
            false,
            DEFAULT_FORCE_COMPRESS_AFTER_CHARS,
            "关闭结果很短，默认关闭压缩。",
        ),
        move |_args: Value| {
            let deps = deps.clone();
            Box::pin(async move {
                let Some(app) = deps.app_handle.clone() else {
                    return Err(ToolExecutionError::other(
                        "错误：浏览器工具缺少 Tauri AppHandle，无法访问浏览器管理器",
                    ));
                };
                let manager = app.state::<BrowserManager>();
                match manager.stop(&deps.workspace_id).await {
                    Ok(()) => {
                        // 浏览器已停：缓存快照与 sidecar 的 ref 映射一并失效，同步丢弃。
                        invalidate_cached_snapshot(&deps.workspace_id);
                        Ok(ToolOutput::text("浏览器已关闭"))
                    }
                    Err(error) => Err(ToolExecutionError::other(format!("错误：{error}"))),
                }
            })
        },
    )
}
