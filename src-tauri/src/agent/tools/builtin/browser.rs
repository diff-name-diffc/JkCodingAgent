//! 浏览器智能体工具（CloakBrowser）。
//!
//! 入口保留工具注册、导航（open_url）、快照读取（read_text）、视觉分析与
//! sidecar 命令管道；子模块按变化原因划分：
//! - `actions`：点击/输入/按键/等待/关闭等单命令转发工具；
//! - `snapshot`：read_text 快照缓存与行号分页渲染；
//! - `recovery`：错误分类与 LLM 感知的自动恢复；
//! - `file_url`：file:// URL 解析与工作区拘禁（高危安全面）。

mod actions;
mod file_url;
mod recovery;
mod snapshot;

use async_trait::async_trait;
use serde_json::{json, Value};
use tauri::Manager;

use actions::{ClickTool, CloseTool, PressTool, TypeTool, WaitForTool};
use file_url::validate_file_url_within_workspace;
use recovery::{classify_browser_error, handle_browser_error, BrowserErrorKind};
use snapshot::{
    format_snapshot_response, invalidate_cached_snapshot, render_cached_page,
    READ_TEXT_DEFAULT_LINE_LIMIT,
};

use super::common::{string_arg, u64_arg, usize_arg, with_compression_parameters};
use crate::agent::llm::{ChatMessage, ChatMessageContentPart, ChatMessageImageSource};
use crate::agent::tools::context::ToolContext;
use crate::agent::tools::registry::AgentTool;
use crate::agent::tools::ToolResult;
use crate::browser::{normalize_browser_url, BrowserManager};

const DEFAULT_BROWSER_TIMEOUT_MS: u64 = 60_000;

pub(super) fn browser_tools() -> Vec<Box<dyn AgentTool>> {
    vec![
        Box::new(OpenUrlTool),
        Box::new(ClickTool),
        Box::new(TypeTool),
        Box::new(PressTool),
        Box::new(WaitForTool),
        Box::new(ReadTextTool),
        Box::new(VisualAnalyzeTool),
        Box::new(CloseTool),
    ]
}

struct OpenUrlTool;
struct ReadTextTool;
struct VisualAnalyzeTool;

#[async_trait]
impl AgentTool for OpenUrlTool {
    fn name(&self) -> &'static str {
        "browser_open_url"
    }

    fn description(&self) -> &'static str {
        "使用项目级 CloakBrowser 打开 URL。支持浏览器引擎可导航的 URL（包括 http、https、file、data、about 等），会自动启动嵌入式浏览器会话，并在右侧浏览器面板实时展示页面。注意：file:// URL 仅允许打开当前工作区内的本地文件，工作区之外的路径会被拒绝。"
    }

    fn parameters(&self) -> Value {
        with_compression_parameters(
            json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "要打开的完整 URL；支持浏览器引擎可导航的协议，例如 http://、https://、file://、data:、about:。file:// 仅允许当前工作区内的本地文件。" },
                    "timeout": { "type": "integer", "description": "超时时间，单位毫秒，默认 60000", "minimum": 1 }
                },
                "required": ["url"]
            }),
            false,
            "浏览器操作结果通常较短，默认关闭压缩。",
        )
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> ToolResult {
        let Some(url) = string_arg(args, "url") else {
            return ToolResult::recoverable_error("错误：缺少必填参数 url");
        };
        let url = match normalize_browser_url(url) {
            Ok(url) => url,
            Err(error) => return ToolResult::recoverable_error(format!("错误：{error}")),
        };
        // file: URL 可被用来读取任意本地文件（随后 browser_read_text 会把内容读入
        // 上下文），属于高危面。这里强制解析出本地路径并校验其必须位于当前工作区内，
        // 越界直接拒绝，不受 restrict_to_workspace 全局开关影响（fail-closed）。
        // 按 scheme 判定（而非 `file://` 前缀）：`file:/path` 单斜杠形式同样是合法
        // file URL，浏览器引擎会将其归一化为 file:///path。
        if url
            .split_once(':')
            .is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case("file"))
        {
            let context = context.clone();
            let url_owned = url.clone();
            let validation = tokio::task::spawn_blocking(move || {
                validate_file_url_within_workspace(&url_owned, &context)
            })
            .await;
            match validation {
                Ok(Ok(())) => {}
                Ok(Err(message)) => return ToolResult::recoverable_error(message),
                Err(error) => {
                    return ToolResult::recoverable_error(format!(
                        "错误：file:// URL 校验任务失败：{error}"
                    ))
                }
            }
        }
        // 即将导航：sidecar 的 ref 映射与缓存快照都会随之失效。提前丢弃缓存，
        // 避免导航后的分页读取返回旧页面内容（即使 open_url 失败，代价也只是
        // 下次分页读取多一次全量抓取）。
        invalidate_cached_snapshot(&context.workspace_id);
        run_browser_command(
            context,
            "open_url",
            json!({ "url": url, "timeout": timeout_arg(args) }),
        )
        .await
    }
}

#[async_trait]
impl AgentTool for ReadTextTool {
    fn name(&self) -> &'static str {
        "browser_read_text"
    }

    fn description(&self) -> &'static str {
        "读取 CloakBrowser 当前页面或指定 ref 元素的可访问性树文本快照，输出为「行号|内容」格式；快照会为可交互/可定位节点生成 ref，后续浏览器自动化统一使用这些 ref。快照较长时超过内联上限（默认 10000 字符）会被截断并注明行位置，此时用 offset/limit 按行号接续读取剩余部分（分页读取的内联上限提高到 20000 字符，一次可读约一两百行）；带行范围的调用读取的是最近一次全量快照（不重新请求页面、ref 保持有效），需要刷新页面状态时省略行范围重新读取。"
    }

    fn parameters(&self) -> Value {
        with_compression_parameters(
            json!({
                "type": "object",
                "properties": {
                    "ref": { "type": "string", "description": "可选。读取某个已知 ref 对应元素的局部 Accessibility Tree；不传则读取整个页面并刷新 ref 映射。局部树的行号独立编号。" },
                    "offset": { "type": "integer", "description": "起始行号，从 1 开始。传了 offset/limit 时读取最近一次全量快照的对应行范围，不会重新请求页面，ref 保持有效；需要最新页面状态时省略行范围重新读取。", "minimum": 1 },
                    "limit": { "type": "integer", "description": "最多读取多少行，默认 2000（即读到快照末尾）。内联字符上限默认 10000，显式指定 offset/limit 分页读取时提高到 20000；输出为 行号|内容，超过上限仍会截断并注明行位置，用 offset 从截断行的下一行接续读取即可。", "minimum": 1 },
                    "max_nodes": { "type": "integer", "description": "最多返回的可访问性节点数，默认 600。仅在不带行范围（重新抓取快照）时生效。", "minimum": 1 },
                    "timeout": { "type": "integer", "description": "超时时间，单位毫秒，默认 60000", "minimum": 1 }
                }
            }),
            false,
            "可访问性树快照经常是后续定位和判断依据，默认关闭压缩；只看页面概览时可开启并写明 compress_intent。",
        )
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> ToolResult {
        ToolResult::from_text(
            async {
                // 与 sidecar 的空 ref 语义对齐：空字符串按未传处理（读取整页）。
                let ref_arg = string_arg(args, "ref").filter(|value| !value.trim().is_empty());
                let offset = usize_arg(args, "offset");
                let limit_arg = usize_arg(args, "limit");
                let has_range = offset.is_some() || limit_arg.is_some();
                let limit = limit_arg.unwrap_or(READ_TEXT_DEFAULT_LINE_LIMIT);

                // 分页读取：全量快照直接命中缓存切片，不重复请求 CDP，也不会刷新
                // sidecar 的 ref 映射（此前下发的 ref 保持有效）。
                if ref_arg.is_none() && has_range {
                    if let Some(page) =
                        render_cached_page(&context.workspace_id, offset.unwrap_or(1), limit)
                    {
                        return page;
                    }
                    // 无缓存（sidecar 重启 / 冷启动）：继续走全量读取后按行范围切片。
                }

                match run_browser_command_value(
                    context,
                    "read_text",
                    json!({
                        "ref": ref_arg,
                        "maxNodes": u64_arg(args, "max_nodes").unwrap_or(600).max(1),
                        "timeout": timeout_arg(args)
                    }),
                )
                .await
                {
                    Ok(value) => format_snapshot_response(
                        value,
                        &context.workspace_id,
                        ref_arg.as_deref(),
                        offset.unwrap_or(1),
                        limit,
                    ),
                    Err(error) => handle_browser_error(context, error).await,
                }
            }
            .await,
        )
    }
}

#[async_trait]
impl AgentTool for VisualAnalyzeTool {
    fn name(&self) -> &'static str {
        "browser_visual_analyze"
    }

    fn description(&self) -> &'static str {
        "对 CloakBrowser 当前可视页面进行轻量视觉理解。工具会在内部截图，并调用已配置的视觉模型按指令分析页面；不会把原始截图 data URL 暴露给聊天上下文。"
    }

    fn parameters(&self) -> Value {
        with_compression_parameters(
            json!({
                "type": "object",
                "properties": {
                    "instruction": { "type": "string", "description": "视觉分析指令：说明当前任务需要关注的页面内容、控件、布局、状态、异常或截图区域线索。" },
                    "timeout": { "type": "integer", "description": "截图超时时间，单位毫秒，默认 60000", "minimum": 1 }
                },
                "required": ["instruction"]
            }),
            false,
            "视觉分析结果已由轻量模型压缩为文本，默认关闭压缩保留完整结果。",
        )
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> ToolResult {
        ToolResult::from_text(execute_visual_analyze(args, context).await)
    }
}

async fn execute_visual_analyze(args: &Value, context: &ToolContext) -> String {
    let Some(instruction) = string_arg(args, "instruction") else {
        return "错误：缺少必填参数 instruction".to_string();
    };
    // 优先使用完整视觉凭据（可指向独立网关/密钥）；未注入 vision_provider
    // 的上下文回退旧行为（聊天 provider + 视觉模型名拼接）。
    let vision_provider = match context.vision_provider.clone() {
        Some(provider) => provider,
        None => {
            let Some(provider) = context.llm_provider.as_ref() else {
                return "错误：浏览器视觉分析缺少 LLM provider，无法调用视觉模型".to_string();
            };
            if context.vision_model.trim().is_empty() {
                return "错误：浏览器视觉分析需要先在 Dispatcher 设置中配置视觉模型".to_string();
            }
            if !provider.is_configured() {
                return "错误：LLM API Key 未配置，无法调用视觉模型".to_string();
            }
            provider.with_model(context.vision_model.trim())
        }
    };

    let screenshot = match run_browser_command_value(
        context,
        "screenshot",
        json!({ "fullPage": false, "timeout": timeout_arg(args) }),
    )
    .await
    {
        Ok(value) => value,
        Err(error) => return format!("错误：{error}"),
    };
    let Some(data_url) = screenshot.get("data").and_then(Value::as_str) else {
        return "错误：浏览器截图结果缺少 data URL，无法进行视觉分析".to_string();
    };

    let prompt = build_visual_analysis_prompt(&instruction);
    match vision_provider
            .chat_stream(
                &[
                    ChatMessage::system(
                        "你是浏览器网页截图的视觉辅助分析器。只基于截图回答，聚焦用户给定指令；不要编造截图中不可见的信息。输出简洁、可执行的中文观察结果。"
                            .to_string(),
                    ),
                    ChatMessage {
                        role: "user".to_string(),
                        content: prompt.clone(),
                        content_parts: vec![
                            ChatMessageContentPart::Text { text: prompt },
                            ChatMessageContentPart::Image {
                                source: ChatMessageImageSource::DataUrl {
                                    data_url: data_url.to_string(),
                                },
                            },
                        ],
                        reasoning_content: None,
                        tool_calls: None,
                        tool_call_id: None,
                        name: None,
                    },
                ],
                &[],
                true,
                |_| {},
            )
            .await
        {
            Ok(response) => {
                let content = response.content.trim();
                if content.is_empty() {
                    "错误：视觉模型返回了空分析结果".to_string()
                } else {
                    content.to_string()
                }
            }
            Err(error) => format!("错误：视觉模型分析网页截图失败：{error}"),
    }
}

async fn run_browser_command(context: &ToolContext, method: &str, params: Value) -> ToolResult {
    match run_browser_command_value(context, method, params).await {
        Ok(value) => browser_value_result(value),
        Err(error) => {
            // For non-ref tools, classify errors but skip auto-snapshot
            let kind = classify_browser_error(&error);
            ToolResult::from_text(match kind {
                BrowserErrorKind::Behavioral => {
                    format!(
                        "错误：{error}\n\n提示：这是一个可恢复的行为错误。请检查当前页面状态，\
                        必要时重新调用 browser_read_text 获取最新快照后重试操作。"
                    )
                }
                BrowserErrorKind::System => format!("错误：浏览器系统错误：{error}"),
                BrowserErrorKind::RefExpired => {
                    // Should not happen for non-ref tools, but handle gracefully
                    handle_browser_error(context, error).await
                }
            })
        }
    }
}

fn browser_value_result(value: Value) -> ToolResult {
    match serde_json::to_string_pretty(&value) {
        Ok(text) => ToolResult::success_data(value, text.clone(), text),
        Err(error) => ToolResult::recoverable_error(format!("错误：浏览器结果序列化失败：{error}")),
    }
}

async fn run_browser_command_value(
    context: &ToolContext,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    let Some(app) = context.app_handle.clone() else {
        return Err("浏览器工具缺少 Tauri AppHandle，无法访问 CloakBrowser 管理器".to_string());
    };
    let manager = app.state::<BrowserManager>();
    manager
        .command(
            app.clone(),
            context.workspace_id.clone(),
            context.workspace.to_string_lossy().to_string(),
            method,
            params,
        )
        .await
}

fn timeout_arg(args: &Value) -> u64 {
    u64_arg(args, "timeout")
        .unwrap_or(DEFAULT_BROWSER_TIMEOUT_MS)
        .max(1)
}

fn build_visual_analysis_prompt(instruction: &str) -> String {
    format!(
        "请分析当前浏览器可视区域截图。\n\n关注内容：\n{}\n\n输出要求：\n- 只描述截图中能确认的事实。\n- 优先指出与任务相关的控件、文字、状态、错误、布局位置和下一步可操作线索。\n- 如果截图不足以判断，请直接说明缺失信息。",
        instruction.trim()
    )
}

fn format_browser_result(value: Value) -> String {
    match serde_json::to_string_pretty(&value) {
        Ok(text) => text,
        Err(error) => format!("错误：浏览器结果序列化失败：{error}"),
    }
}
