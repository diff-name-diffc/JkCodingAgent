use async_trait::async_trait;
use serde_json::{json, Value};
use tauri::Manager;

use super::common::{string_arg, u64_arg, with_result_mode_parameter};
use crate::agent::llm::ChatMessage;
use crate::agent::tools::context::ToolContext;
use crate::agent::tools::registry::AgentTool;
use crate::browser::BrowserManager;

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
struct ClickTool;
struct TypeTool;
struct PressTool;
struct WaitForTool;
struct ReadTextTool;
struct VisualAnalyzeTool;
struct CloseTool;

#[async_trait]
impl AgentTool for OpenUrlTool {
    fn name(&self) -> &'static str {
        "browser_open_url"
    }

    fn description(&self) -> &'static str {
        "使用项目级 CloakBrowser 打开网页。会自动启动嵌入式浏览器会话，并在右侧浏览器面板实时展示页面。"
    }

    fn parameters(&self) -> Value {
        with_result_mode_parameter(
            json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "要打开的 URL，必须包含 http:// 或 https://" },
                    "timeout": { "type": "integer", "description": "超时时间，单位毫秒，默认 30000", "minimum": 1 }
                },
                "required": ["url"]
            }),
            "auto",
            "浏览器操作结果通常较短，默认即可。",
        )
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
        let Some(url) = string_arg(args, "url") else {
            return "错误：缺少必填参数 url".to_string();
        };
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return "错误：url 必须以 http:// 或 https:// 开头".to_string();
        }
        run_browser_command(
            context,
            "open_url",
            json!({ "url": url, "timeout": timeout_arg(args) }),
        )
        .await
    }
}

#[async_trait]
impl AgentTool for ClickTool {
    fn name(&self) -> &'static str {
        "browser_click"
    }

    fn description(&self) -> &'static str {
        "点击 Accessibility Tree 快照中的元素 ref。先调用 browser_read_text 获取页面快照，再使用快照中标注的 ref。"
    }

    fn parameters(&self) -> Value {
        with_result_mode_parameter(
            json!({
                "type": "object",
                "properties": {
                    "ref": { "type": "string", "description": "browser_read_text 返回的元素 ref，例如 r12" },
                    "timeout": { "type": "integer", "description": "超时时间，单位毫秒，默认 30000", "minimum": 1 }
                },
                "required": ["ref"]
            }),
            "auto",
            "点击结果很短，默认即可。",
        )
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
        let Some(ref_id) = string_arg(args, "ref") else {
            return "错误：缺少必填参数 ref；请先调用 browser_read_text 获取元素 ref".to_string();
        };
        run_browser_command(
            context,
            "click",
            json!({
                "ref": ref_id,
                "timeout": timeout_arg(args)
            }),
        )
        .await
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
        with_result_mode_parameter(
            json!({
                "type": "object",
                "properties": {
                    "ref": { "type": "string", "description": "browser_read_text 返回的输入元素 ref，例如 r12" },
                    "text": { "type": "string", "description": "要输入的文本" },
                    "timeout": { "type": "integer", "description": "超时时间，单位毫秒，默认 30000", "minimum": 1 }
                },
                "required": ["ref", "text"]
            }),
            "auto",
            "输入结果很短，默认即可。",
        )
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
        let Some(ref_id) = string_arg(args, "ref") else {
            return "错误：缺少必填参数 ref；请先调用 browser_read_text 获取输入元素 ref"
                .to_string();
        };
        let Some(text) = string_arg(args, "text") else {
            return "错误：缺少必填参数 text".to_string();
        };
        run_browser_command(
            context,
            "type",
            json!({ "ref": ref_id, "text": text, "timeout": timeout_arg(args) }),
        )
        .await
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
        with_result_mode_parameter(
            json!({
                "type": "object",
                "properties": {
                    "key": { "type": "string", "description": "Playwright 按键名称，例如 Enter" }
                },
                "required": ["key"]
            }),
            "auto",
            "按键结果很短，默认即可。",
        )
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
        let Some(key) = string_arg(args, "key") else {
            return "错误：缺少必填参数 key".to_string();
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
        with_result_mode_parameter(
            json!({
                "type": "object",
                "properties": {
                    "load_state": {
                        "type": "string",
                        "description": "Playwright load state",
                        "enum": ["load", "domcontentloaded", "networkidle"]
                    },
                    "timeout": { "type": "integer", "description": "超时时间，单位毫秒，默认 30000", "minimum": 1 }
                }
            }),
            "auto",
            "等待结果很短，默认即可。",
        )
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
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
impl AgentTool for ReadTextTool {
    fn name(&self) -> &'static str {
        "browser_read_text"
    }

    fn description(&self) -> &'static str {
        "读取 CloakBrowser 当前页面或指定 ref 元素的可访问性树文本快照。快照会为可交互/可定位节点生成 ref，后续浏览器自动化统一使用这些 ref。"
    }

    fn parameters(&self) -> Value {
        with_result_mode_parameter(
            json!({
                "type": "object",
                "properties": {
                    "ref": { "type": "string", "description": "可选。读取某个已知 ref 对应元素的局部 Accessibility Tree；不传则读取整个页面并刷新 ref 映射。" },
                    "max_nodes": { "type": "integer", "description": "最多返回的可访问性节点数，默认 600", "minimum": 1 },
                    "timeout": { "type": "integer", "description": "超时时间，单位毫秒，默认 30000", "minimum": 1 }
                }
            }),
            "full",
            "可访问性树快照经常是后续定位和判断依据，默认保留完整结果。",
        )
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
        run_browser_command(
            context,
            "read_text",
            json!({
                "ref": string_arg(args, "ref"),
                "maxNodes": u64_arg(args, "max_nodes").unwrap_or(600).max(1),
                "timeout": timeout_arg(args)
            }),
        )
        .await
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
        with_result_mode_parameter(
            json!({
                "type": "object",
                "properties": {
                    "instruction": { "type": "string", "description": "视觉分析指令：说明当前任务需要关注的页面内容、控件、布局、状态、异常或截图区域线索。" },
                    "timeout": { "type": "integer", "description": "截图超时时间，单位毫秒，默认 30000", "minimum": 1 }
                },
                "required": ["instruction"]
            }),
            "full",
            "视觉分析结果已由轻量模型压缩为文本，默认保留完整结果。",
        )
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
        let Some(instruction) = string_arg(args, "instruction") else {
            return "错误：缺少必填参数 instruction".to_string();
        };
        let Some(provider) = context.llm_provider.as_ref() else {
            return "错误：浏览器视觉分析缺少 LLM provider，无法调用视觉模型".to_string();
        };
        if context.vision_model.trim().is_empty() {
            return "错误：浏览器视觉分析需要先在 Dispatcher 设置中配置视觉模型".to_string();
        }
        if !provider.is_configured() {
            return "错误：LLM API Key 未配置，无法调用视觉模型".to_string();
        }

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

        let vision_provider = provider.with_model(context.vision_model.trim());
        let prompt = build_visual_analysis_prompt(&instruction, data_url);
        match vision_provider
            .chat_stream(
                &[
                    ChatMessage::system(
                        "你是浏览器网页截图的视觉辅助分析器。只基于截图回答，聚焦用户给定指令；不要编造截图中不可见的信息。输出简洁、可执行的中文观察结果。"
                            .to_string(),
                    ),
                    ChatMessage {
                        role: "user".to_string(),
                        content: prompt,
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
        with_result_mode_parameter(
            json!({ "type": "object", "properties": {} }),
            "auto",
            "关闭结果很短，默认即可。",
        )
    }

    async fn execute(&self, _args: &Value, context: &ToolContext) -> String {
        let Some(app) = context.app_handle.clone() else {
            return "错误：浏览器工具缺少 Tauri AppHandle，无法访问 CloakBrowser 管理器"
                .to_string();
        };
        let manager = app.state::<BrowserManager>();
        match manager.stop(&context.workspace_id).await {
            Ok(()) => "CloakBrowser 已关闭".to_string(),
            Err(error) => format!("错误：{error}"),
        }
    }
}

async fn run_browser_command(context: &ToolContext, method: &str, params: Value) -> String {
    match run_browser_command_value(context, method, params).await {
        Ok(value) => format_browser_result(value),
        Err(error) => format!("错误：{error}"),
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
    u64_arg(args, "timeout").unwrap_or(30_000).max(1)
}

fn build_visual_analysis_prompt(instruction: &str, data_url: &str) -> String {
    format!(
        "请分析当前浏览器可视区域截图。\n\n关注内容：\n{}\n\n输出要求：\n- 只描述截图中能确认的事实。\n- 优先指出与任务相关的控件、文字、状态、错误、布局位置和下一步可操作线索。\n- 如果截图不足以判断，请直接说明缺失信息。\n\n![browser screenshot]({})",
        instruction.trim(),
        data_url
    )
}

fn format_browser_result(value: Value) -> String {
    match serde_json::to_string_pretty(&value) {
        Ok(text) => text,
        Err(error) => format!("错误：浏览器结果序列化失败：{error}"),
    }
}
