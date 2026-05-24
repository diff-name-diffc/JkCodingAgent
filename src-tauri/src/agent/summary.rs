use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use serde_json::Value;
use tokio::time::{timeout, Duration};

use super::llm::{ChatMessage, LlmUsage, OpenAiCompatProvider};

const SUMMARY_MODE_THRESHOLD_CHARS: usize = 240;
const SUMMARY_MODE_THRESHOLD_LINES: usize = 24;
const FULL_RESULT_MAX_CHARS: usize = 12_000;
const FULL_RESULT_MAX_LINES: usize = 400;
const HIGH_FIDELITY_SUMMARY_THRESHOLD_CHARS: usize = 1_000;
const SUMMARY_TIMEOUT_SECS: u64 = 120;
const SUMMARY_DEBUG_PREVIEW_CHARS: usize = 1_200;
const SESSION_TITLE_SOURCE_MAX_CHARS: usize = 6_000;
const SESSION_TITLE_MESSAGE_MAX_CHARS: usize = 1_200;
const SESSION_TITLE_MAX_CHARS: usize = 10;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SummaryError {
    message: String,
    debug_context: String,
}

impl SummaryError {
    fn new(message: impl Into<String>, debug_context: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            debug_context: debug_context.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn debug_context(&self) -> &str {
        &self.debug_context
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolArtifactDraft {
    pub kind: String,
    pub title: String,
    pub preview: String,
    pub content: String,
    pub char_count: usize,
    pub line_count: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedToolResult {
    pub context_payload: String,
    pub display_content: String,
    pub result_mode: &'static str,
    pub artifacts: Vec<ToolArtifactDraft>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionTitleMessage {
    pub role: String,
    pub content: String,
}

#[allow(clippy::too_many_arguments)]
pub async fn prepare_tool_result<FStart, FDelta, FUsage>(
    provider: &OpenAiCompatProvider,
    summary_model: &str,
    tool_name: &str,
    args: &Value,
    raw_output: &str,
    on_display_stream_start: FStart,
    on_display_delta: FDelta,
    on_usage: FUsage,
) -> Result<PreparedToolResult, SummaryError>
where
    FStart: Fn(&'static str) + Send + Sync,
    FDelta: Fn(&str) + Send + Sync,
    FUsage: FnMut(&LlmUsage) + Send,
{
    let trimmed_raw = raw_output.trim();
    if trimmed_raw.is_empty() {
        return Ok(PreparedToolResult {
            context_payload: String::new(),
            display_content: String::new(),
            result_mode: tool_result_mode_label(ToolResultAction::KeepRaw),
            artifacts: Vec::new(),
        });
    }

    let normalized = normalize_tool_output(raw_output);
    let normalized_trimmed = normalized.trim();
    let artifacts = vec![build_raw_tool_artifact(tool_name, raw_output)];
    let on_display_delta = Arc::new(on_display_delta);

    match decide_tool_result_action(tool_name, args, trimmed_raw) {
        ToolResultAction::KeepRaw => Ok(PreparedToolResult {
            context_payload: trimmed_raw.to_string(),
            display_content: trimmed_raw.to_string(),
            result_mode: tool_result_mode_label(ToolResultAction::KeepRaw),
            artifacts,
        }),
        ToolResultAction::HighFidelitySummarize => {
            let result_mode = tool_result_mode_label(ToolResultAction::HighFidelitySummarize);
            let display_stream = Arc::new(Mutex::new(TaggedBlockStream::new("DISPLAY_SUMMARY")));
            let emitted_summary = Arc::new(AtomicBool::new(false));
            on_display_stream_start(result_mode);
            let raw_summary = summarize_with_model(
                provider,
                summary_model,
                build_dual_tool_summary_prompt(tool_name, normalized_trimmed),
                {
                    let display_stream = Arc::clone(&display_stream);
                    let emitted_summary = Arc::clone(&emitted_summary);
                    let on_display_delta = Arc::clone(&on_display_delta);
                    move |delta| {
                        let streamed = display_stream
                            .lock()
                            .expect("tool summary stream poisoned")
                            .push(delta);
                        if streamed.is_empty() {
                            return;
                        }
                        emitted_summary.store(true, Ordering::Relaxed);
                        on_display_delta(&streamed);
                    }
                },
                on_usage,
            )
            .await?;
            let (context_payload, display_content) = parse_dual_tool_summary(raw_summary);
            if !emitted_summary.load(Ordering::Relaxed) && !display_content.is_empty() {
                on_display_delta(&display_content);
            }
            Ok(PreparedToolResult {
                context_payload,
                display_content,
                result_mode,
                artifacts,
            })
        }
    }
}

pub async fn summarize_dispatch_result<FUsage>(
    provider: &OpenAiCompatProvider,
    summary_model: &str,
    dispatch_result: &str,
    on_usage: FUsage,
) -> Result<String, SummaryError>
where
    FUsage: FnMut(&LlmUsage) + Send,
{
    let trimmed = dispatch_result.trim();
    if trimmed.is_empty() {
        return Ok(trimmed.to_string());
    }

    if !exceeds_limits(
        trimmed,
        SUMMARY_MODE_THRESHOLD_CHARS,
        SUMMARY_MODE_THRESHOLD_LINES,
    ) {
        return Ok(trimmed.to_string());
    }

    summarize_with_model(
        provider,
        summary_model,
        build_dispatch_summary_prompt(trimmed),
        |_| {},
        on_usage,
    )
    .await
}

pub async fn summarize_session_title<FUsage>(
    provider: &OpenAiCompatProvider,
    summary_model: &str,
    messages: &[SessionTitleMessage],
    fallback_source: &str,
    on_usage: FUsage,
) -> Result<String, SummaryError>
where
    FUsage: FnMut(&LlmUsage) + Send,
{
    let raw_title = summarize_with_model(
        provider,
        summary_model,
        build_session_title_prompt(messages, fallback_source),
        |_| {},
        on_usage,
    )
    .await?;

    Ok(normalize_session_title(&raw_title, fallback_source))
}

pub fn fallback_session_title(user_prompt: &str) -> String {
    normalize_session_title(user_prompt, "新会话")
}

async fn summarize_with_model(
    provider: &OpenAiCompatProvider,
    summary_model: &str,
    prompt: String,
    on_delta: impl FnMut(&str),
    mut on_usage: impl FnMut(&LlmUsage) + Send,
) -> Result<String, SummaryError> {
    let summary_provider = provider.with_model(summary_model);
    let debug_context = build_summary_debug_context(&summary_provider, &prompt);
    let response = timeout(
        Duration::from_secs(SUMMARY_TIMEOUT_SECS),
        summary_provider.chat_stream(&[ChatMessage::system(prompt)], &[], false, on_delta),
    )
    .await
    .map_err(|_| {
        SummaryError::new(
            format!("摘要模型 `{summary_model}` 调用超时（>{SUMMARY_TIMEOUT_SECS}s）"),
            debug_context.clone(),
        )
    })?
    .map_err(|error| {
        SummaryError::new(
            format!("摘要模型 `{summary_model}` 调用失败：{error}"),
            debug_context.clone(),
        )
    })?;

    if let Some(usage) = response.usage.as_ref() {
        on_usage(usage);
    }

    let content = response.content.trim().to_string();
    if content.is_empty() {
        return Err(SummaryError::new(
            format!("摘要模型 `{summary_model}` 返回空结果"),
            debug_context,
        ));
    }

    Ok(content)
}

fn build_summary_debug_context(provider: &OpenAiCompatProvider, prompt: &str) -> String {
    format!(
        "调用方式：OpenAI 兼容流式摘要请求\n模型：{}\n超时阈值：{} 秒\nprompt 字符数：{}\nprompt 行数：{}\nprompt 预览：\n{}",
        provider.model(),
        SUMMARY_TIMEOUT_SECS,
        prompt.chars().count(),
        prompt.lines().count().max(1),
        build_prompt_preview(prompt),
    )
}

fn build_prompt_preview(prompt: &str) -> String {
    let total_chars = prompt.chars().count();
    if total_chars <= SUMMARY_DEBUG_PREVIEW_CHARS {
        return prompt.to_string();
    }

    let preview = prompt
        .chars()
        .take(SUMMARY_DEBUG_PREVIEW_CHARS)
        .collect::<String>();
    format!(
        "{preview}\n...（已截断，预览 {} / {} 字符）",
        SUMMARY_DEBUG_PREVIEW_CHARS, total_chars
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ToolResultMode {
    Auto,
    Full,
    Summary,
}

fn build_dual_tool_summary_prompt(tool_name: &str, raw_output: &str) -> String {
    let focus = tool_summary_focus(tool_name);
    format!(
        "你在为调度 Agent 生成两份不同用途的结果：一份用于继续注入模型上下文，一份用于前端展示给用户。\n\
要求：\n\
- 只保留原文里明确出现的事实，不要猜测。\n\
- 输出必须严格分成两个区块，且只能使用下面的标签，不要额外添加解释、标题或 Markdown 代码块。\n\
- `<DISPLAY_SUMMARY>`：写给前端展示，要求对人类更友好，聚焦结论、关键事实和为什么值得关注，可以比上下文回写更易读，但不能脱离原文事实。\n\
- `<CONTEXT_PAYLOAD>`：写给主模型，要求高信息密度，尽量保留原始顺序、关键实体名、文件路径、符号名、配置键、错误文本、数量和退出状态；如果内容主要是代码、配置、逐行检索结果、文件清单或其他精确检索输出，只能做最轻量压缩，严禁改写代码含义、删除关键行号、文件名或配置键；{focus}\n\
- 如果内容是命令输出，优先保留命令结果、失败原因、关键日志、测试失败项和退出状态。\n\
- 需要压缩，但不要过度归纳；宁可稍长，也不要丢掉影响后续判断的细节。\n\
- 严格使用以下格式输出：\n\
<DISPLAY_SUMMARY>\n\
...\n\
</DISPLAY_SUMMARY>\n\
<CONTEXT_PAYLOAD>\n\
...\n\
</CONTEXT_PAYLOAD>\n\
工具名：{tool_name}\n\
工具原始输出如下：\n{}",
        raw_output.trim()
    )
}

fn build_dispatch_summary_prompt(dispatch_result: &str) -> String {
    format!(
        "你是调度 Agent 的子任务回流摘要器。请把下面的子智能体执行结果压缩成可直接注入主调度上下文的中文摘要。\n\
要求：\n\
- 只保留事实，不要臆测。\n\
- 必须保留：当前状态、已完成事项、失败/阻塞点、关键证据、建议下一步。\n\
- 如有测试、构建、lint、报错、修改文件、退出状态，优先保留。\n\
- 可以比工具结果摘要更激进地压缩冗余过程描述，但不能丢掉结论依据。\n\
- 合并重复进度、重复日志和礼貌性措辞；只保留对主调度继续决策有价值的部分。\n\
- 使用下面结构输出，最多 6 行：\n\
【子任务回流摘要】\n\
- 状态：...\n\
- 已完成：...\n\
- 阻塞/风险：...\n\
- 关键证据：...\n\
- 建议下一步：...\n\n\
子任务原始结果如下：\n{}",
        dispatch_result.trim()
    )
}

fn build_session_title_prompt(messages: &[SessionTitleMessage], fallback_source: &str) -> String {
    let source = build_session_title_source(messages, fallback_source);
    let prompt = truncate_session_title_source(&source);
    format!(
        "你是桌面 AI 编程工具的会话标题生成器。请根据最近多条聊天消息生成一个极短中文标题。\n\
要求：\n\
- 只输出标题本身，不要解释，不要加引号、编号、Markdown 或“标题：”前缀。\n\
- 标题必须是 5-10 个中文字符；不要超过 10 个字符。\n\
- 标题应是名词短语，概括最后一轮完整对话的核心任务；如果最后用户消息是“另外/继续/这个”等追加要求，必须结合前文对象。\n\
- 优先保留关键模块、功能、错误或对象；删除“帮我”“看看”“优化一下”“问题”“任务”等水词。\n\
- 不要输出完整句子，不要包含标点。\n\n\
最近对话如下（按时间顺序；最后一轮优先）：\n{}",
        prompt.trim()
    )
}

fn build_session_title_source(messages: &[SessionTitleMessage], fallback_source: &str) -> String {
    let mut source = String::new();

    for message in messages {
        let content = truncate_session_title_message(&message.content);
        if content.trim().is_empty() {
            continue;
        }

        source.push('【');
        source.push_str(session_title_role_label(&message.role));
        source.push_str("】\n");
        source.push_str(content.trim());
        source.push_str("\n\n");
    }

    if source.trim().is_empty() {
        fallback_source.trim().to_string()
    } else {
        source
    }
}

fn session_title_role_label(role: &str) -> &'static str {
    match role {
        "user" => "用户",
        "assistant" => "助手",
        "tool" => "工具结果",
        _ => "消息",
    }
}

fn truncate_session_title_message(source: &str) -> String {
    let mut truncated = source
        .chars()
        .take(SESSION_TITLE_MESSAGE_MAX_CHARS)
        .collect::<String>();
    if source.chars().count() > SESSION_TITLE_MESSAGE_MAX_CHARS {
        truncated.push_str("\n...");
    }
    truncated
}

fn truncate_session_title_source(source: &str) -> String {
    let mut truncated = source
        .chars()
        .take(SESSION_TITLE_SOURCE_MAX_CHARS)
        .collect::<String>();
    if source.chars().count() > SESSION_TITLE_SOURCE_MAX_CHARS {
        truncated.push_str("\n...");
    }
    truncated
}

fn normalize_session_title(candidate: &str, fallback_source: &str) -> String {
    let fallback = if fallback_source == "新会话" {
        "新会话".to_string()
    } else {
        truncate_title(clean_title_line(fallback_source))
    };

    let title = truncate_title(clean_title_line(candidate));
    if title.is_empty() || title == "新会话" {
        return fallback;
    }

    title
}

fn clean_title_line(raw: &str) -> String {
    let line = raw
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();

    let without_prefix = line
        .trim_start_matches(['-', '*', '#', '>', ' ', '\t'])
        .trim_start_matches("标题：")
        .trim_start_matches("标题:")
        .trim_start_matches("会话标题：")
        .trim_start_matches("会话标题:")
        .trim();

    let trimmed = without_prefix.trim_matches(|ch: char| {
        ch.is_whitespace()
            || matches!(
                ch,
                '"' | '\'' | '`' | '“' | '”' | '‘' | '’' | '。' | '，' | ',' | '.' | '：' | ':'
            )
    });

    trimmed.split_whitespace().collect::<Vec<_>>().join("")
}

fn truncate_title(title: String) -> String {
    title.chars().take(SESSION_TITLE_MAX_CHARS).collect()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ToolOutputKind {
    Exact,
    Command,
    Mutation,
    Message,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ToolResultAction {
    KeepRaw,
    HighFidelitySummarize,
}

fn tool_result_mode_label(action: ToolResultAction) -> &'static str {
    match action {
        ToolResultAction::KeepRaw => "raw",
        ToolResultAction::HighFidelitySummarize => "conservative_summary",
    }
}

fn decide_tool_result_action(tool_name: &str, args: &Value, raw_output: &str) -> ToolResultAction {
    let mode = requested_result_mode(args).unwrap_or_else(|| default_result_mode(tool_name));
    let kind = tool_output_kind(tool_name);
    if should_keep_raw_for_exactness(tool_name, kind, raw_output) {
        return ToolResultAction::KeepRaw;
    }

    match mode {
        ToolResultMode::Summary => summarize_if_large_enough(raw_output),
        ToolResultMode::Full => {
            if exceeds_limits(raw_output, FULL_RESULT_MAX_CHARS, FULL_RESULT_MAX_LINES) {
                ToolResultAction::HighFidelitySummarize
            } else {
                ToolResultAction::KeepRaw
            }
        }
        ToolResultMode::Auto => match kind {
            ToolOutputKind::Exact => ToolResultAction::KeepRaw,
            ToolOutputKind::Command | ToolOutputKind::Other => {
                summarize_if_large_enough(raw_output)
            }
            ToolOutputKind::Mutation | ToolOutputKind::Message => ToolResultAction::KeepRaw,
        },
    }
}

fn summarize_if_large_enough(raw_output: &str) -> ToolResultAction {
    if raw_output.chars().count() > HIGH_FIDELITY_SUMMARY_THRESHOLD_CHARS {
        ToolResultAction::HighFidelitySummarize
    } else {
        ToolResultAction::KeepRaw
    }
}

fn requested_result_mode(args: &Value) -> Option<ToolResultMode> {
    let raw = args.get("result_mode")?.as_str()?;
    match raw.trim().to_ascii_lowercase().as_str() {
        "auto" => Some(ToolResultMode::Auto),
        "full" => Some(ToolResultMode::Full),
        "summary" => Some(ToolResultMode::Summary),
        _ => None,
    }
}

fn default_result_mode(tool_name: &str) -> ToolResultMode {
    match tool_output_kind(tool_name) {
        ToolOutputKind::Exact => ToolResultMode::Full,
        ToolOutputKind::Command
        | ToolOutputKind::Mutation
        | ToolOutputKind::Message
        | ToolOutputKind::Other => ToolResultMode::Auto,
    }
}

fn tool_output_kind(tool_name: &str) -> ToolOutputKind {
    match tool_name {
        "read_file"
        | "list_dir"
        | "glob"
        | "grep"
        | "browser_read_text"
        | "browser_visual_analyze" => ToolOutputKind::Exact,
        "exec" => ToolOutputKind::Command,
        "write_file" | "edit_file" | "generate_image" | "edit_image" => ToolOutputKind::Mutation,
        "message" => ToolOutputKind::Message,
        _ => ToolOutputKind::Other,
    }
}

fn tool_summary_focus(tool_name: &str) -> &'static str {
    match tool_name {
        "read_file" => "保留关键文件路径、符号名、行号范围、配置键和能支持判断的核心实现细节",
        "list_dir" | "glob" => "保留目录层级、关键文件名、数量和显著的结构特征",
        "grep" => "保留匹配文件路径、行号、命中片段、上下文和能支撑后续 read_file 的关键关键词",
        "exec" => "保留命令结果、错误文本、失败项、退出状态、关键路径和数量统计",
        _ => "保留后续判断最依赖的事实、路径、标识符和数量信息",
    }
}

fn parse_dual_tool_summary(output: String) -> (String, String) {
    let parsed = (
        extract_tagged_block(&output, "CONTEXT_PAYLOAD"),
        extract_tagged_block(&output, "DISPLAY_SUMMARY"),
    );

    match parsed {
        (Some(context_payload), Some(display_summary))
            if !context_payload.is_empty() && !display_summary.is_empty() =>
        {
            (context_payload, display_summary)
        }
        _ => {
            let fallback = output.trim().to_string();
            (fallback.clone(), fallback)
        }
    }
}

fn extract_tagged_block(output: &str, tag: &str) -> Option<String> {
    let start_tag = format!("<{tag}>");
    let end_tag = format!("</{tag}>");
    let start = output.find(&start_tag)?;
    let content_start = start + start_tag.len();
    let end = output[content_start..].find(&end_tag)?;
    let content = &output[content_start..content_start + end];
    let trimmed = content.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[derive(Debug)]
struct TaggedBlockStream {
    start_tag: String,
    end_tag: String,
    buffer: String,
    trim_prefix_bytes: usize,
    emitted_bytes: usize,
}

impl TaggedBlockStream {
    fn new(tag: &str) -> Self {
        Self {
            start_tag: format!("<{tag}>"),
            end_tag: format!("</{tag}>"),
            buffer: String::new(),
            trim_prefix_bytes: 0,
            emitted_bytes: 0,
        }
    }

    fn push(&mut self, delta: &str) -> String {
        if delta.is_empty() {
            return String::new();
        }
        self.buffer.push_str(delta);

        let Some(start) = self.buffer.find(&self.start_tag) else {
            return String::new();
        };
        let content_start = start + self.start_tag.len();
        let content_tail = &self.buffer[content_start..];
        let content_end = content_tail
            .find(&self.end_tag)
            .unwrap_or(content_tail.len());
        let content = &content_tail[..content_end];

        if self.emitted_bytes == 0 {
            let trimmed = content.trim_start_matches(['\r', '\n']);
            self.trim_prefix_bytes = content.len() - trimmed.len();
        }

        let visible_start = self.trim_prefix_bytes + self.emitted_bytes;
        if content.len() <= visible_start {
            return String::new();
        }

        let segment = &content[visible_start..];
        self.emitted_bytes += segment.len();
        segment.to_string()
    }
}

fn build_raw_tool_artifact(tool_name: &str, raw_output: &str) -> ToolArtifactDraft {
    ToolArtifactDraft {
        kind: "tool_raw_output".to_string(),
        title: format!("{tool_name} 原始结果"),
        preview: build_artifact_preview(raw_output),
        content: raw_output.to_string(),
        char_count: raw_output.chars().count(),
        line_count: raw_output.lines().count().max(1),
    }
}

fn build_artifact_preview(raw_output: &str) -> String {
    let preview = raw_output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(3)
        .collect::<Vec<_>>()
        .join(" / ");

    if preview.is_empty() {
        "原始结果为空白或仅包含空行".to_string()
    } else if preview.chars().count() > 160 {
        let shortened = preview.chars().take(160).collect::<String>();
        format!("{shortened}...")
    } else {
        preview
    }
}

fn should_keep_raw_for_exactness(tool_name: &str, kind: ToolOutputKind, raw_output: &str) -> bool {
    matches!(kind, ToolOutputKind::Exact)
        || tool_name == "message"
        || looks_like_code_or_precise_retrieval(raw_output)
}

fn looks_like_code_or_precise_retrieval(raw_output: &str) -> bool {
    if raw_output.contains("```") {
        return true;
    }

    let lines = raw_output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(40)
        .collect::<Vec<_>>();
    if lines.is_empty() {
        return false;
    }

    let numbered_read_lines = lines
        .iter()
        .filter(|line| {
            line.split_once('|')
                .and_then(|(prefix, _)| prefix.parse::<usize>().ok())
                .is_some()
        })
        .count();
    if numbered_read_lines >= 3 {
        return true;
    }

    let code_like_lines = lines
        .iter()
        .filter(|line| {
            line.starts_with("fn ")
                || line.starts_with("pub ")
                || line.starts_with("impl ")
                || line.starts_with("struct ")
                || line.starts_with("enum ")
                || line.starts_with("class ")
                || line.starts_with("def ")
                || line.starts_with("function ")
                || line.starts_with("import ")
                || line.starts_with("export ")
                || line.starts_with("const ")
                || line.starts_with("let ")
                || line.starts_with("var ")
                || line.starts_with("#include")
                || line.ends_with('{')
                || line.ends_with("};")
                || line.contains("=>")
                || line.contains("::")
                || line.contains("</")
        })
        .count();

    code_like_lines >= 3 && code_like_lines * 2 >= lines.len().min(10)
}

fn normalize_tool_output(raw_output: &str) -> String {
    raw_output
        .split('\n')
        .map(normalize_tool_output_line)
        .collect::<Vec<_>>()
        .join("\n")
}

fn normalize_tool_output_line(line: &str) -> String {
    if line.trim().is_empty() {
        return String::new();
    }

    let indent_len = line
        .char_indices()
        .find(|(_, ch)| *ch != ' ' && *ch != '\t')
        .map(|(index, _)| index)
        .unwrap_or(line.len());
    let (indent, rest) = line.split_at(indent_len);
    let mut normalized = String::with_capacity(line.len());
    normalized.push_str(indent);

    let mut space_run = 0usize;
    for ch in rest.chars() {
        if ch == ' ' {
            space_run += 1;
            if space_run == 1 {
                normalized.push(ch);
            }
            continue;
        }

        if space_run > 2 {
            normalized.pop();
            normalized.push(' ');
        }
        space_run = 0;
        normalized.push(ch);
    }

    if space_run > 2 {
        normalized.pop();
        normalized.push(' ');
    }

    normalized.trim_end_matches(' ').to_string()
}

fn exceeds_limits(raw_output: &str, max_chars: usize, max_lines: usize) -> bool {
    raw_output.chars().count() > max_chars || raw_output.lines().count() > max_lines
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        build_artifact_preview, build_prompt_preview, build_session_title_prompt,
        build_session_title_source, build_summary_debug_context, decide_tool_result_action,
        extract_tagged_block, fallback_session_title, normalize_session_title,
        normalize_tool_output, parse_dual_tool_summary, SessionTitleMessage, ToolResultAction,
        SESSION_TITLE_MAX_CHARS, HIGH_FIDELITY_SUMMARY_THRESHOLD_CHARS,
    };
    use crate::agent::llm::OpenAiCompatProvider;

    fn summary_provider() -> OpenAiCompatProvider {
        OpenAiCompatProvider::new(
            "key".to_string(),
            "https://example.com/v1".to_string(),
            "qwen3.6-plus".to_string(),
            2048,
            0.1,
        )
    }

    #[test]
    fn exact_tools_keep_medium_sized_raw_output_by_default() {
        let action = decide_tool_result_action("read_file", &json!({}), &"a".repeat(2_000));
        assert_eq!(action, ToolResultAction::KeepRaw);
    }

    #[test]
    fn grep_is_treated_as_exact_output() {
        let action = decide_tool_result_action("grep", &json!({}), &"a".repeat(2_000));
        assert_eq!(action, ToolResultAction::KeepRaw);
    }

    #[test]
    fn exec_auto_mode_uses_high_fidelity_summary_for_large_output() {
        let action = decide_tool_result_action("exec", &json!({}), &"a".repeat(2_000));
        assert_eq!(action, ToolResultAction::HighFidelitySummarize);
    }

    #[test]
    fn explicit_full_mode_prevents_normal_summary_until_hard_limit() {
        let action = decide_tool_result_action(
            "exec",
            &json!({ "result_mode": "full" }),
            &"a".repeat(2_000),
        );
        assert_eq!(action, ToolResultAction::KeepRaw);
    }

    #[test]
    fn explicit_summary_mode_requests_high_fidelity_summary() {
        let action = decide_tool_result_action(
            "exec",
            &json!({ "result_mode": "summary" }),
            &"a".repeat(1_500),
        );
        assert_eq!(action, ToolResultAction::HighFidelitySummarize);
    }

    #[test]
    fn short_summary_mode_still_keeps_raw() {
        let action = decide_tool_result_action("exec", &json!({ "result_mode": "summary" }), "ok");
        assert_eq!(action, ToolResultAction::KeepRaw);
    }

    #[test]
    fn full_mode_falls_back_to_high_fidelity_summary_for_oversized_output() {
        let action = decide_tool_result_action(
            "exec",
            &json!({ "result_mode": "full" }),
            &"a".repeat(20_000),
        );
        assert_eq!(action, ToolResultAction::HighFidelitySummarize);
    }

    #[test]
    fn code_like_exec_output_keeps_raw() {
        let action = decide_tool_result_action(
            "exec",
            &json!({}),
            "fn main() {\n  let value = 1;\n  println!(\"{}\", value);\n}\n",
        );
        assert_eq!(action, ToolResultAction::KeepRaw);
    }

    #[test]
    fn normalize_tool_output_collapses_blank_line_spaces_and_long_space_runs() {
        let normalized = normalize_tool_output("alpha   beta\n   \n  gamma    delta  ");
        assert_eq!(normalized, "alpha beta\n\n  gamma delta");
    }

    #[test]
    fn parse_dual_tool_summary_prefers_tagged_sections() {
        let parsed = parse_dual_tool_summary(
            "<CONTEXT_PAYLOAD>\nctx\n</CONTEXT_PAYLOAD>\n<DISPLAY_SUMMARY>\nui\n</DISPLAY_SUMMARY>"
                .to_string(),
        );
        assert_eq!(parsed, ("ctx".to_string(), "ui".to_string()));
    }

    #[test]
    fn extract_tagged_block_trims_internal_padding() {
        let block = extract_tagged_block("<A>\n  value  \n</A>", "A");
        assert_eq!(block, Some("value".to_string()));
    }

    #[test]
    fn build_artifact_preview_uses_first_non_empty_lines() {
        let preview = build_artifact_preview("\nalpha\nbeta\n\ngamma\ndelta");
        assert_eq!(preview, "alpha / beta / gamma");
    }

    #[test]
    fn summary_debug_context_contains_model_and_prompt_metadata() {
        let context = build_summary_debug_context(&summary_provider(), "alpha\nbeta");
        assert!(context.contains("模型：qwen3.6-plus"));
        assert!(context.contains("prompt 字符数：10"));
        assert!(context.contains("prompt 行数：2"));
        assert!(context.contains("alpha\nbeta"));
    }

    #[test]
    fn prompt_preview_truncates_long_input() {
        let preview = build_prompt_preview(&"a".repeat(1_500));
        assert!(preview.contains("已截断"));
        assert!(preview.contains("1200 / 1500"));
    }

    #[test]
    fn session_title_normalization_removes_common_prefixes() {
        let title = normalize_session_title("标题：`优化 Agent 会话命名`", "用户问题");
        assert_eq!(title, "优化Agent会话命");
    }

    #[test]
    fn session_title_falls_back_to_prompt_when_model_is_generic() {
        let title = normalize_session_title("新会话", "修复会话标题一直显示新会话的问题");
        assert_eq!(title, "修复会话标题一直显示");
    }

    #[test]
    fn fallback_session_title_truncates_long_prompt() {
        let title =
            fallback_session_title("请帮我实现一个非常非常非常非常非常非常长的会话标题生成逻辑");
        assert!(title.chars().count() <= 10);
    }

    #[test]
    fn session_title_prompt_uses_recent_dialogue_context() {
        let prompt = build_session_title_prompt(
            &[
                SessionTitleMessage {
                    role: "user".to_string(),
                    content: "优化聊天 Markdown 代码块样式".to_string(),
                },
                SessionTitleMessage {
                    role: "assistant".to_string(),
                    content: "已调整亮色和暗色代码块主题。".to_string(),
                },
                SessionTitleMessage {
                    role: "user".to_string(),
                    content: "另外标题也太长了".to_string(),
                },
            ],
            "另外标题也太长了",
        );

        assert!(prompt.contains("最近多条聊天消息"));
        assert!(prompt.contains("5-10 个中文字符"));
        assert!(prompt.contains("如果最后用户消息是“另外/继续/这个”等追加要求"));
        assert!(prompt.contains("【用户】\n优化聊天 Markdown 代码块样式"));
        assert!(prompt.contains("【助手】\n已调整亮色和暗色代码块主题。"));
    }

    #[test]
    fn session_title_source_falls_back_when_dialogue_is_empty() {
        let source = build_session_title_source(&[], "修复标题生成");
        assert_eq!(source, "修复标题生成");
    }

    // --- Additional tests for uncovered functions ---

    #[test]
    fn session_title_source_builds_from_messages() {
        let messages = vec![
            SessionTitleMessage { role: "user".to_string(), content: "hello".to_string() },
            SessionTitleMessage { role: "assistant".to_string(), content: "world".to_string() },
        ];
        let source = build_session_title_source(&messages, "fallback");
        assert!(source.contains("【用户】"));
        assert!(source.contains("hello"));
        assert!(source.contains("【助手】"));
        assert!(source.contains("world"));
    }

    #[test]
    fn session_title_source_skips_empty_messages() {
        let messages = vec![
            SessionTitleMessage { role: "user".to_string(), content: "  ".to_string() },
            SessionTitleMessage { role: "assistant".to_string(), content: "actual content".to_string() },
        ];
        let source = build_session_title_source(&messages, "fallback");
        assert!(!source.contains("【用户】"));
        assert!(source.contains("【助手】"));
        assert!(source.contains("actual content"));
    }

    #[test]
    fn session_title_role_label_maps_known_roles() {
        assert_eq!(super::session_title_role_label("user"), "用户");
        assert_eq!(super::session_title_role_label("assistant"), "助手");
        assert_eq!(super::session_title_role_label("tool"), "工具结果");
    }

    #[test]
    fn session_title_role_label_defaults_for_unknown() {
        assert_eq!(super::session_title_role_label("system"), "消息");
        assert_eq!(super::session_title_role_label("unknown"), "消息");
    }

    #[test]
    fn truncate_session_title_message_short_input_unchanged() {
        let msg = "short message";
        let truncated = super::truncate_session_title_message(msg);
        assert_eq!(truncated, msg);
    }

    #[test]
    fn truncate_session_title_message_long_input_is_cut() {
        let long_msg = "x".repeat(2000);
        let truncated = super::truncate_session_title_message(&long_msg);
        assert!(truncated.len() < long_msg.len());
        assert!(truncated.ends_with("\n..."));
    }

    #[test]
    fn truncate_session_title_source_short_input_unchanged() {
        let source = "short source content";
        let truncated = super::truncate_session_title_source(source);
        assert_eq!(truncated, source);
    }

    #[test]
    fn truncate_session_title_source_long_input_is_cut() {
        let long_source = "y".repeat(10000);
        let truncated = super::truncate_session_title_source(&long_source);
        assert!(truncated.len() < long_source.len());
        assert!(truncated.ends_with("\n..."));
    }

    #[test]
    fn clean_title_line_strips_markdown_prefixes() {
        assert_eq!(super::clean_title_line("# Hello"), "Hello");
        assert_eq!(super::clean_title_line("## World"), "World");
        assert_eq!(super::clean_title_line("> Quote"), "Quote");
        // split_whitespace().join("") removes all internal whitespace
        assert_eq!(super::clean_title_line("- List item"), "Listitem");
        assert_eq!(super::clean_title_line("* Bold item"), "Bolditem");
    }

    #[test]
    fn clean_title_line_strips_chinese_title_prefix() {
        assert_eq!(super::clean_title_line("标题：测试标题"), "测试标题");
        assert_eq!(super::clean_title_line("标题:测试标题"), "测试标题");
        assert_eq!(super::clean_title_line("会话标题：会话名称"), "会话名称");
    }

    #[test]
    fn clean_title_line_strips_quotes_and_punctuation() {
        assert_eq!(super::clean_title_line("'测试'"), "测试");
        assert_eq!(super::clean_title_line("\"test\""), "test");
        assert_eq!(super::clean_title_line("`code`"), "code");
    }

    #[test]
    fn clean_title_line_returns_empty_for_empty_input() {
        assert_eq!(super::clean_title_line(""), "");
        assert_eq!(super::clean_title_line("   "), "");
    }

    #[test]
    fn clean_title_line_skips_blank_leading_lines() {
        assert_eq!(super::clean_title_line("\n\nactual title"), "actualtitle");
    }

    #[test]
    fn truncate_title_caps_at_max_chars() {
        let long_title = "很长的标题".repeat(5); // 25 chars
        let truncated = super::truncate_title(long_title);
        assert_eq!(truncated.chars().count(), SESSION_TITLE_MAX_CHARS);
    }

    #[test]
    fn truncate_title_short_input_unchanged() {
        let short = "短标题";
        let truncated = super::truncate_title(short.to_string());
        assert_eq!(truncated, "短标题");
    }

    #[test]
    fn normalize_session_title_returns_fallback_when_model_says_new_session() {
        let title = normalize_session_title("新会话", "实际用户问题");
        assert_eq!(title, "实际用户问题");
    }

    #[test]
    fn normalize_session_title_returns_fallback_when_model_returns_empty() {
        let title = normalize_session_title("", "用户原始问题");
        assert_eq!(title, "用户原始问题");
    }

    #[test]
    fn normalize_session_title_prefers_model_output_when_valid() {
        let title = normalize_session_title("代码重构优化", "用户原始问题");
        assert_eq!(title, "代码重构优化");
    }

    #[test]
    fn normalize_session_title_truncates_long_model_output() {
        let long = "这是一个非常非常非常长的标题不应该被完整保留";
        let title = normalize_session_title(long, "fallback");
        assert!(title.chars().count() <= SESSION_TITLE_MAX_CHARS);
    }

    #[test]
    fn tool_output_kind_maps_known_tools() {
        assert_eq!(super::tool_output_kind("read_file"), super::ToolOutputKind::Exact);
        assert_eq!(super::tool_output_kind("list_dir"), super::ToolOutputKind::Exact);
        assert_eq!(super::tool_output_kind("glob"), super::ToolOutputKind::Exact);
        assert_eq!(super::tool_output_kind("grep"), super::ToolOutputKind::Exact);
        assert_eq!(super::tool_output_kind("browser_read_text"), super::ToolOutputKind::Exact);
        assert_eq!(super::tool_output_kind("browser_visual_analyze"), super::ToolOutputKind::Exact);
        assert_eq!(super::tool_output_kind("exec"), super::ToolOutputKind::Command);
        assert_eq!(super::tool_output_kind("write_file"), super::ToolOutputKind::Mutation);
        assert_eq!(super::tool_output_kind("edit_file"), super::ToolOutputKind::Mutation);
        assert_eq!(super::tool_output_kind("generate_image"), super::ToolOutputKind::Mutation);
        assert_eq!(super::tool_output_kind("edit_image"), super::ToolOutputKind::Mutation);
        assert_eq!(super::tool_output_kind("message"), super::ToolOutputKind::Message);
        assert_eq!(super::tool_output_kind("unknown_tool"), super::ToolOutputKind::Other);
    }

    #[test]
    fn default_result_mode_returns_full_for_exact_tools() {
        assert_eq!(super::default_result_mode("read_file"), super::ToolResultMode::Full);
    }

    #[test]
    fn default_result_mode_returns_auto_for_command_tools() {
        assert_eq!(super::default_result_mode("exec"), super::ToolResultMode::Auto);
    }

    #[test]
    fn default_result_mode_returns_auto_for_unknown_tools() {
        assert_eq!(super::default_result_mode("custom_tool"), super::ToolResultMode::Auto);
    }

    #[test]
    fn requested_result_mode_parses_known_modes() {
        assert_eq!(
            super::requested_result_mode(&json!({"result_mode": "auto"})),
            Some(super::ToolResultMode::Auto)
        );
        assert_eq!(
            super::requested_result_mode(&json!({"result_mode": "full"})),
            Some(super::ToolResultMode::Full)
        );
        assert_eq!(
            super::requested_result_mode(&json!({"result_mode": "summary"})),
            Some(super::ToolResultMode::Summary)
        );
    }

    #[test]
    fn requested_result_mode_returns_none_for_unknown_mode() {
        assert_eq!(
            super::requested_result_mode(&json!({"result_mode": "unknown"})),
            None
        );
    }

    #[test]
    fn requested_result_mode_returns_none_when_missing() {
        assert_eq!(super::requested_result_mode(&json!({})), None);
    }

    #[test]
    fn requested_result_mode_is_case_insensitive() {
        assert_eq!(
            super::requested_result_mode(&json!({"result_mode": "FULL"})),
            Some(super::ToolResultMode::Full)
        );
        assert_eq!(
            super::requested_result_mode(&json!({"result_mode": "Summary"})),
            Some(super::ToolResultMode::Summary)
        );
    }

    #[test]
    fn summarize_if_large_enough_keeps_raw_for_short_output() {
        let action = super::summarize_if_large_enough("short");
        assert_eq!(action, ToolResultAction::KeepRaw);
    }

    #[test]
    fn summarize_if_large_enough_summarizes_for_large_output() {
        let large = "x".repeat(HIGH_FIDELITY_SUMMARY_THRESHOLD_CHARS + 1);
        let action = super::summarize_if_large_enough(&large);
        assert_eq!(action, ToolResultAction::HighFidelitySummarize);
    }

    #[test]
    fn exceeds_limits_detects_char_limit() {
        let text = "a".repeat(100);
        assert!(super::exceeds_limits(&text, 99, 200));
        assert!(!super::exceeds_limits(&text, 101, 200));
    }

    #[test]
    fn exceeds_limits_detects_line_limit() {
        let text = "line1\nline2\nline3";
        assert!(super::exceeds_limits(text, 100, 2));
        assert!(!super::exceeds_limits(text, 100, 4));
    }

    #[test]
    fn tool_result_mode_label_returns_correct_strings() {
        assert_eq!(super::tool_result_mode_label(ToolResultAction::KeepRaw), "raw");
        assert_eq!(
            super::tool_result_mode_label(ToolResultAction::HighFidelitySummarize),
            "conservative_summary"
        );
    }

    #[test]
    fn looks_like_code_or_precise_retrieval_detects_code_blocks() {
        assert!(super::looks_like_code_or_precise_retrieval("```rust\nfn main() {}\n```"));
    }

    #[test]
    fn looks_like_code_or_precise_retrieval_detects_numbered_lines() {
        let numbered = "1|first line\n2|second line\n3|third line";
        assert!(super::looks_like_code_or_precise_retrieval(numbered));
    }

    #[test]
    fn looks_like_code_or_precise_retrieval_detects_rust_code() {
        let code = "fn main() {\n  pub fn helper() {}\n  impl Foo {}\n}";
        assert!(super::looks_like_code_or_precise_retrieval(code));
    }

    #[test]
    fn looks_like_code_or_precise_retrieval_detects_python_code() {
        let code = "def foo():\n  import os\n  class Bar:\n    pass";
        assert!(super::looks_like_code_or_precise_retrieval(code));
    }

    #[test]
    fn looks_like_code_or_precise_retrieval_rejects_plain_text() {
        assert!(!super::looks_like_code_or_precise_retrieval("This is just plain text output"));
    }

    #[test]
    fn should_keep_raw_for_exactness_keeps_exact_tools() {
        assert!(super::should_keep_raw_for_exactness(
            "read_file",
            super::ToolOutputKind::Exact,
            "output"
        ));
    }

    #[test]
    fn should_keep_raw_for_exactness_keeps_message_tool() {
        assert!(super::should_keep_raw_for_exactness(
            "message",
            super::ToolOutputKind::Message,
            "output"
        ));
    }

    #[test]
    fn should_keep_raw_for_exactness_keeps_code_output() {
        assert!(super::should_keep_raw_for_exactness(
            "exec",
            super::ToolOutputKind::Command,
            "```js\nconsole.log('hi');\n```"
        ));
    }

    #[test]
    fn build_dispatch_summary_prompt_contains_required_structure() {
        let prompt = super::build_dispatch_summary_prompt("test result");
        assert!(prompt.contains("【子任务回流摘要】"));
        assert!(prompt.contains("状态："));
        assert!(prompt.contains("已完成："));
        assert!(prompt.contains("阻塞/风险："));
        assert!(prompt.contains("关键证据："));
        assert!(prompt.contains("建议下一步："));
        assert!(prompt.contains("test result"));
    }

    #[test]
    fn build_dual_tool_summary_prompt_contains_tag_format() {
        let prompt = super::build_dual_tool_summary_prompt("exec", "some output");
        assert!(prompt.contains("<DISPLAY_SUMMARY>"));
        assert!(prompt.contains("</DISPLAY_SUMMARY>"));
        assert!(prompt.contains("<CONTEXT_PAYLOAD>"));
        assert!(prompt.contains("</CONTEXT_PAYLOAD>"));
        assert!(prompt.contains("some output"));
    }

    #[test]
    fn parse_dual_tool_summary_falls_back_when_tags_missing() {
        let output = "just plain text without tags".to_string();
        let (context, display) = parse_dual_tool_summary(output.clone());
        assert_eq!(context, output);
        assert_eq!(display, output);
    }

    #[test]
    fn parse_dual_tool_summary_falls_back_when_context_empty() {
        let output = "<CONTEXT_PAYLOAD>\n\n</CONTEXT_PAYLOAD>\n<DISPLAY_SUMMARY>\nreal\n</DISPLAY_SUMMARY>".to_string();
        let (context, display) = parse_dual_tool_summary(output);
        // Falls back to full output since one tag is empty
        assert_eq!(context, display);
    }

    #[test]
    fn extract_tagged_block_returns_none_for_missing_tag() {
        assert_eq!(extract_tagged_block("no tags here", "MISSING"), None);
    }

    #[test]
    fn extract_tagged_block_returns_none_for_empty_content() {
        assert_eq!(extract_tagged_block("<A>\n   \n</A>", "A"), None);
    }

    #[test]
    fn build_raw_tool_artifact_fields_are_correct() {
        let artifact = super::build_raw_tool_artifact("grep", "line1\nline2\nline3");
        assert_eq!(artifact.kind, "tool_raw_output");
        assert_eq!(artifact.title, "grep 原始结果");
        assert_eq!(artifact.content, "line1\nline2\nline3");
        assert_eq!(artifact.char_count, 17);
        assert_eq!(artifact.line_count, 3);
    }

    #[test]
    fn build_artifact_preview_caps_at_160_chars() {
        let long_line = "x".repeat(200);
        let preview = build_artifact_preview(&long_line);
        assert!(preview.len() <= 170); // 160 chars + "..."
        assert!(preview.ends_with("..."));
    }

    #[test]
    fn build_artifact_preview_returns_default_for_blank_input() {
        let preview = build_artifact_preview("\n  \n  \n");
        assert_eq!(preview, "原始结果为空白或仅包含空行");
    }

    #[test]
    fn tag_block_stream_emits_nothing_before_start_tag() {
        let mut stream = super::TaggedBlockStream::new("DISPLAY_SUMMARY");
        assert_eq!(stream.push("some text without tag"), "");
    }

    #[test]
    fn tag_block_stream_emits_content_after_start_tag() {
        let mut stream = super::TaggedBlockStream::new("A");
        let tag = "<A>hello</A>";
        let emitted = stream.push(tag);
        assert!(emitted.contains("hello"));
    }

    #[test]
    fn tag_block_stream_handles_incremental_push() {
        let mut stream = super::TaggedBlockStream::new("A");
        assert_eq!(stream.push("<A>"), "");
        let emitted = stream.push("content");
        assert!(emitted.contains("content"));
    }

    #[test]
    fn tag_block_stream_returns_empty_for_empty_delta() {
        let mut stream = super::TaggedBlockStream::new("A");
        assert_eq!(stream.push(""), "");
    }

    #[test]
    fn normalize_tool_output_removes_trailing_spaces() {
        let result = normalize_tool_output("hello   ");
        assert_eq!(result, "hello");
    }

    #[test]
    fn normalize_tool_output_collapses_consecutive_blank_lines() {
        let result = normalize_tool_output("line1\n   \n   \nline2");
        assert_eq!(result, "line1\n\n\nline2");
    }

    #[test]
    fn build_session_title_prompt_includes_fallback_when_messages_empty() {
        let prompt = build_session_title_prompt(&[], "fallback prompt text");
        assert!(prompt.contains("fallback prompt text"));
    }

    #[test]
    fn tool_summary_focus_returns_non_empty_for_known_tools() {
        assert!(!super::tool_summary_focus("read_file").is_empty());
        assert!(!super::tool_summary_focus("glob").is_empty());
        assert!(!super::tool_summary_focus("grep").is_empty());
        assert!(!super::tool_summary_focus("exec").is_empty());
        assert!(!super::tool_summary_focus("unknown").is_empty());
    }
}
