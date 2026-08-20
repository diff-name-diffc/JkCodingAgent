use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::{json, Value};
use tauri::Manager;

use super::common::{
    canonicalize_existing_prefix, string_arg, u64_arg, usize_arg, with_compression_parameters,
};
use crate::agent::llm::{ChatMessage, ChatMessageContentPart, ChatMessageImageSource};
use crate::agent::tools::context::ToolContext;
use crate::agent::tools::registry::AgentTool;
use crate::agent::tools::ToolResult;
use crate::browser::{normalize_browser_url, BrowserManager};

const DEFAULT_BROWSER_TIMEOUT_MS: u64 = 60_000;

/// Browser error classification for LLM-aware error handling.
#[derive(Debug, Clone, PartialEq)]
enum BrowserErrorKind {
    /// Element ref expired due to page navigation or DOM change.
    /// Recoverable: auto-fetch fresh snapshot and let LLM retry.
    RefExpired,
    /// System-level error (process crash, network failure, etc.).
    /// Fatal: report immediately, no auto-recovery.
    System,
    /// Transient behavioral error (timeout, page not ready, etc.).
    /// Recoverable: LLM can adjust strategy and retry.
    Behavioral,
}

fn classify_browser_error(error: &str) -> BrowserErrorKind {
    if error.starts_with("[ref_expired]") {
        BrowserErrorKind::RefExpired
    } else if error.contains("超时")
        || error.contains("Timeout")
        || error.contains("timeout")
        || error.contains("尚未启动")
        || error.contains("not ready")
    {
        BrowserErrorKind::Behavioral
    } else {
        BrowserErrorKind::System
    }
}

/// When a ref-expired error is detected, automatically fetch a fresh accessibility
/// snapshot so the LLM receives up-to-date element refs in the same turn.
async fn auto_recover_snapshot(context: &ToolContext, original_error: &str) -> String {
    let recovery_result = run_browser_command_value(
        context,
        "read_text",
        json!({
            "ref": Value::Null,
            "maxNodes": 600,
            "timeout": timeout_arg(&json!({"timeout": 30_000}))
        }),
    )
    .await;

    match recovery_result {
        Ok(snapshot_value) => {
            // 复用 read_text 的行号分页渲染：恢复快照同样登记缓存，模型可直接用
            // offset/limit 接续读取，而不必整包重读。
            let snapshot_text = format_snapshot_response(
                snapshot_value,
                &context.workspace_id,
                None,
                1,
                READ_TEXT_DEFAULT_LINE_LIMIT,
            );
            format!(
                "错误：[ref_expired] {original_error}\n\n\
                ⚠️ 页面元素引用已失效（可能是页面发生了导航或内容变化）。\n\
                已自动获取最新页面快照，请基于以下新快照重新选择目标元素：\n\n\
                {snapshot_text}"
            )
        }
        Err(recovery_error) => {
            format!(
                "错误：[ref_expired] {original_error}\n\n\
                ⚠️ 页面元素引用已失效。尝试自动获取新快照时也失败了：{recovery_error}\n\
                请先调用 browser_read_text 获取最新快照，再重试操作。"
            )
        }
    }
}

/// Format a browser error with LLM-friendly guidance based on error classification.
async fn handle_browser_error(context: &ToolContext, error: String) -> String {
    match classify_browser_error(&error) {
        BrowserErrorKind::RefExpired => auto_recover_snapshot(context, &error).await,
        BrowserErrorKind::Behavioral => {
            format!(
                "错误：{error}\n\n提示：这是一个可恢复的行为错误。请检查当前页面状态，\
                必要时重新调用 browser_read_text 获取最新快照后重试操作。"
            )
        }
        BrowserErrorKind::System => {
            // System errors: report loudly, no auto-recovery
            format!("错误：浏览器系统错误：{error}")
        }
    }
}

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
        snapshot_cache().lock().remove(&context.workspace_id);
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

// ─── read_text 快照分页 ─────────────────────────────────────────────────────

/// read_text 不传 limit 时读取的行数上限（与 read_file 的默认 limit 对齐）。
const READ_TEXT_DEFAULT_LINE_LIMIT: usize = 2_000;

/// sidecar read_text 结果的解析产物：结构化元信息 + 快照树行。
#[derive(Clone)]
struct SnapshotContent {
    url: String,
    node_count: u64,
    emitted: u64,
    ref_count: u64,
    truncated: bool,
    lines: Vec<String>,
}

/// 每个 workspace 缓存最近一次全量快照。带 offset/limit 的分页读取直接命中缓存：
/// 不再重复请求 CDP，既保证行号与模型刚看到的快照一致，也避免刷新 sidecar 的
/// ref 映射导致此前下发的 ref 全部失效。
struct CachedSnapshot {
    snapshot_id: u64,
    content: SnapshotContent,
}

/// 全局快照序号：让模型能察觉两次分页读取之间快照是否已被刷新。
static SNAPSHOT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn snapshot_cache() -> &'static Mutex<HashMap<String, CachedSnapshot>> {
    static CACHE: OnceLock<Mutex<HashMap<String, CachedSnapshot>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn store_snapshot(workspace_id: &str, content: SnapshotContent) -> u64 {
    let snapshot_id = SNAPSHOT_SEQUENCE.fetch_add(1, Ordering::Relaxed) + 1;
    snapshot_cache().lock().insert(
        workspace_id.to_string(),
        CachedSnapshot {
            snapshot_id,
            content,
        },
    );
    snapshot_id
}

/// 从 sidecar read_text 返回值提取快照内容。sidecar 的 text 自带
/// `# Accessibility Tree Snapshot\nurl: …\nnode_count: …\ntruncated: …\n\n` 头部，
/// 元信息改由结构化字段渲染，只有头部之后的树行才参与行号分页——否则整棵树
/// 会挤在 pretty JSON 的单个字符串行里，行级截断定位完全失效。
fn extract_snapshot(value: &Value) -> Option<SnapshotContent> {
    let text = value.get("text")?.as_str()?;
    let tree = text
        .split_once("\n\n")
        .map(|(_, tree)| tree)
        .unwrap_or(text);
    Some(SnapshotContent {
        url: value
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        node_count: value.get("nodeCount").and_then(Value::as_u64).unwrap_or(0),
        emitted: value
            .get("emittedNodeCount")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        ref_count: value.get("refCount").and_then(Value::as_u64).unwrap_or(0),
        truncated: value
            .get("truncated")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        lines: tree.lines().map(str::to_string).collect(),
    })
}

/// 渲染快照的一页：元信息头 + `行号|内容` 树行切片，行号格式与 read_file 一致，
/// 通用管线的字符截断定位器可直接命中行号前缀。offset/limit 语义与 read_file
/// 对齐：1 起始、包含边界、越界时返回可用内容。
fn render_snapshot_page(
    content: &SnapshotContent,
    ref_arg: Option<&str>,
    snapshot_id: Option<u64>,
    offset: usize,
    limit: usize,
) -> String {
    let total = content.lines.len();
    let start = offset.max(1);
    let limit = limit.max(1);

    let mut header = match ref_arg {
        Some(ref_id) => format!("# Accessibility Tree Snapshot（ref={ref_id} 局部树）"),
        None => "# Accessibility Tree Snapshot".to_string(),
    };
    header.push_str(&format!("\nurl: {}", content.url));
    if ref_arg.is_some() {
        header.push_str(&format!(
            "\nnodes: {} emitted / {} refs",
            content.emitted, content.ref_count
        ));
    } else {
        header.push_str(&format!(
            "\nnodes: {} total / {} emitted / {} refs",
            content.node_count, content.emitted, content.ref_count
        ));
    }
    header.push_str(&format!("\nsnapshot_truncated: {}", content.truncated));
    if let Some(id) = snapshot_id {
        header.push_str(&format!("\nsnapshot: #{id}"));
    }

    if start > total {
        return format!("{header}\n\n（起始行 {start} 超出快照总行数 {total}，无内容返回）");
    }

    let end = (start + limit - 1).min(total);
    let mut parts = vec![
        header,
        format!("lines: {start}-{end} / {total}"),
        String::new(),
    ];
    for (index, line) in content.lines[start - 1..end].iter().enumerate() {
        parts.push(format!("{}|{}", start + index, line));
    }
    // 只陈述剩余行数，不引导下一步动作——是否继续读取由 Agent 决定
    // （头部的 lines: start-end / total 已足够推算后续行号）。
    if end < total {
        parts.push(String::new());
        parts.push(format!("（还有 {} 行未返回）", total - end));
    }
    parts.join("\n")
}

/// 全量/局部读取 sidecar 快照后：登记缓存（仅全量）并渲染行号分页结果。
/// sidecar 返回结构不符合预期时退回整包 JSON 展示。
fn format_snapshot_response(
    value: Value,
    workspace_id: &str,
    ref_arg: Option<&str>,
    offset: usize,
    limit: usize,
) -> String {
    let Some(content) = extract_snapshot(&value) else {
        return format_browser_result(value);
    };
    let snapshot_id = (ref_arg.is_none()).then(|| store_snapshot(workspace_id, content.clone()));
    render_snapshot_page(&content, ref_arg, snapshot_id, offset, limit)
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
                    let cache = snapshot_cache().lock();
                    if let Some(snapshot) = cache.get(&context.workspace_id) {
                        return render_snapshot_page(
                            &snapshot.content,
                            None,
                            Some(snapshot.snapshot_id),
                            offset.unwrap_or(1),
                            limit,
                        );
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
                snapshot_cache().lock().remove(&context.workspace_id);
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

/// 解析 `file:` URL 中的本地路径部分。同时覆盖 `file:///path`、`file://localhost/path`
/// 与单斜杠 `file:/path`（RFC 8089 合法形式，浏览器引擎会归一化为三斜杠）。
/// 仅接受空主机或 `localhost`；其他主机（`file://host/...`）无法在本地校验，直接拒绝。
fn file_url_to_path(url: &str) -> Result<std::path::PathBuf, String> {
    let Some((scheme, rest)) = url.split_once(':') else {
        return Err("错误：不是有效的 file:// URL".to_string());
    };
    if !scheme.eq_ignore_ascii_case("file") {
        return Err("错误：不是有效的 file:// URL".to_string());
    }
    let path = match rest.strip_prefix("//") {
        Some(after_authority) => {
            // rest 形如 `[host]/path`；host 与 path 以第一个 `/` 分隔。
            let Some(slash_index) = after_authority.find('/') else {
                return Err(format!("错误：file:// URL 缺少本地路径：{url}"));
            };
            let host = &after_authority[..slash_index];
            if !host.is_empty() && !host.eq_ignore_ascii_case("localhost") {
                return Err(format!("错误：file:// URL 不支持非本地主机：{host}"));
            }
            &after_authority[slash_index..]
        }
        // 单斜杠 `file:/path`（无 authority）。
        None => rest,
    };
    if path.is_empty() {
        return Err(format!("错误：file:// URL 缺少本地路径：{url}"));
    }
    Ok(std::path::PathBuf::from(percent_decode(path)))
}

/// 解码 URL 百分号转义（如 `%20` -> 空格），避免编码后的路径绕过校验。
/// 纯字节处理：先校验 `%` 后两字节均为 ASCII 十六进制位再解码，
/// 避免在多字节 UTF-8 字符上做 `&str` 切片导致 panic。
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && bytes[index + 1].is_ascii_hexdigit()
            && bytes[index + 2].is_ascii_hexdigit()
        {
            // 已确保为 ASCII 十六进制位，from_utf8 必然成功。
            if let Ok(hex) = std::str::from_utf8(&bytes[index + 1..index + 3]) {
                if let Ok(byte) = u8::from_str_radix(hex, 16) {
                    decoded.push(byte);
                    index += 3;
                    continue;
                }
            }
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&decoded).to_string()
}

/// 校验 `file://` URL 指向的本地路径必须位于当前工作区内。
/// 通过 canonicalize（解析符号链接）后做前缀包含判断，防止 `..` 或符号链接逃逸。
fn validate_file_url_within_workspace(url: &str, context: &ToolContext) -> Result<(), String> {
    let path = file_url_to_path(url)?;
    let candidate = canonicalize_existing_prefix(&path)
        .map_err(|error| format!("错误：无法解析 file:// URL 路径：{error}"))?;
    let workspace = context
        .workspace
        .canonicalize()
        .map_err(|error| format!("错误：解析工作区路径失败：{error}"))?;
    if !candidate.starts_with(&workspace) {
        return Err(format!(
            "错误：file:// URL 指向工作区之外的路径，已拒绝打开：{url}"
        ));
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::{extract_snapshot, file_url_to_path, percent_decode, render_snapshot_page};
    use serde_json::json;

    #[test]
    fn file_url_to_path_parses_local_paths() {
        assert_eq!(
            file_url_to_path("file:///etc/passwd").unwrap(),
            std::path::PathBuf::from("/etc/passwd")
        );
        assert_eq!(
            file_url_to_path("file:///Users/jk/a.png").unwrap(),
            std::path::PathBuf::from("/Users/jk/a.png")
        );
        assert_eq!(
            file_url_to_path("file://localhost/tmp/x.html").unwrap(),
            std::path::PathBuf::from("/tmp/x.html")
        );
        // 单斜杠形式（RFC 8089）同样必须被解析并纳入校验
        assert_eq!(
            file_url_to_path("file:/etc/passwd").unwrap(),
            std::path::PathBuf::from("/etc/passwd")
        );
    }

    #[test]
    fn file_url_to_path_decodes_percent_escapes() {
        assert_eq!(
            file_url_to_path("file:///dir/file%20name.png").unwrap(),
            std::path::PathBuf::from("/dir/file name.png")
        );
    }

    #[test]
    fn file_url_to_path_rejects_remote_hosts_and_missing_path() {
        assert!(file_url_to_path("file://remote/share/x").is_err());
        assert!(file_url_to_path("file://localhost").is_err());
        assert!(file_url_to_path("file://").is_err());
        assert!(file_url_to_path("https://example.com").is_err());
    }

    #[test]
    fn percent_decode_handles_utf8_and_leaves_plus_literal() {
        assert_eq!(percent_decode("a%20b"), "a b");
        // UTF-8 多字节序列（中 = E4 B8 AD）
        assert_eq!(percent_decode("%E4%B8%AD"), "中");
        // 文件路径中 `+` 是字面量，不应解码为空格
        assert_eq!(percent_decode("a+b"), "a+b");
        // 非法转义按原样保留
        assert_eq!(percent_decode("%ZZ"), "%ZZ");
    }

    // ─── read_text 快照分页 ───

    fn sample_snapshot_value() -> serde_json::Value {
        json!({
            "text": "# Accessibility Tree Snapshot\nurl: https://example.com/\nnode_count: 5\ntruncated: false\n\n- button [ref=r1] \"登录\"\n  - textbox [ref=r2] \"用户名\" value=\"jk\"\n- link [ref=r3] \"下一页\"",
            "nodeCount": 5,
            "emittedNodeCount": 3,
            "refCount": 3,
            "truncated": false,
            "url": "https://example.com/"
        })
    }

    #[test]
    fn extract_snapshot_separates_sidecar_header_from_tree_lines() {
        let content = extract_snapshot(&sample_snapshot_value()).unwrap();
        assert_eq!(content.url, "https://example.com/");
        assert_eq!(content.node_count, 5);
        assert_eq!(content.emitted, 3);
        assert_eq!(content.ref_count, 3);
        assert!(!content.truncated);
        assert_eq!(
            content.lines,
            vec![
                "- button [ref=r1] \"登录\"",
                "  - textbox [ref=r2] \"用户名\" value=\"jk\"",
                "- link [ref=r3] \"下一页\""
            ]
        );
    }

    #[test]
    fn render_snapshot_page_numbers_all_lines_by_default() {
        let content = extract_snapshot(&sample_snapshot_value()).unwrap();
        let page = render_snapshot_page(&content, None, Some(7), 1, 2_000);
        assert!(page.contains("# Accessibility Tree Snapshot\nurl: https://example.com/"));
        assert!(page.contains("nodes: 5 total / 3 emitted / 3 refs"));
        assert!(page.contains("snapshot: #7"));
        assert!(page.contains("lines: 1-3 / 3"));
        assert!(page.contains("1|- button [ref=r1] \"登录\""));
        assert!(page.contains("3|- link [ref=r3] \"下一页\""));
        // 已读到末行，不再输出翻页提示
        assert!(!page.contains("继续读取"));
    }

    #[test]
    fn render_snapshot_page_slices_inclusive_range_and_states_remaining_neutrally() {
        let content = extract_snapshot(&sample_snapshot_value()).unwrap();
        let page = render_snapshot_page(&content, None, None, 2, 1);
        assert!(page.contains("lines: 2-2 / 3"));
        assert!(page.contains("2|  - textbox [ref=r2] \"用户名\" value=\"jk\""));
        assert!(!page.contains("1|- button"));
        // 只陈述剩余行数，不出现指令式引导
        assert!(page.contains("（还有 1 行未返回）"));
        assert!(!page.contains("继续读取"));
        assert!(!page.contains("offset=3"));
    }

    #[test]
    fn render_snapshot_page_reports_offset_beyond_total() {
        let content = extract_snapshot(&sample_snapshot_value()).unwrap();
        let page = render_snapshot_page(&content, None, None, 10, 50);
        assert!(page.contains("起始行 10 超出快照总行数 3，无内容返回"));
        assert!(!page.contains("请省略"));
        assert!(!page.contains("|-"));
    }

    #[test]
    fn render_snapshot_page_marks_partial_tree_scope() {
        let content = extract_snapshot(&sample_snapshot_value()).unwrap();
        let page = render_snapshot_page(&content, Some("r12"), None, 1, 2_000);
        assert!(page.contains("# Accessibility Tree Snapshot（ref=r12 局部树）"));
        assert!(page.contains("nodes: 3 emitted / 3 refs"));
    }
}
