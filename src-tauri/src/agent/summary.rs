use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use serde_json::Value;
use tokio::time::{timeout, Duration};

use super::llm::{ChatMessage, OpenAiCompatProvider};

const SUMMARY_MODE_THRESHOLD_CHARS: usize = 240;
const SUMMARY_MODE_THRESHOLD_LINES: usize = 24;
const FULL_RESULT_MAX_CHARS: usize = 12_000;
const FULL_RESULT_MAX_LINES: usize = 400;
const HIGH_FIDELITY_SUMMARY_THRESHOLD_CHARS: usize = 1_000;
const SUMMARY_MODEL: &str = "qwen3.6-flash";
const SUMMARY_TIMEOUT_SECS: u64 = 120;
const SUMMARY_DEBUG_PREVIEW_CHARS: usize = 1_200;

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

pub async fn prepare_tool_result<FStart, FDelta>(
    provider: &OpenAiCompatProvider,
    tool_name: &str,
    args: &Value,
    raw_output: &str,
    on_display_stream_start: FStart,
    on_display_delta: FDelta,
) -> Result<PreparedToolResult, SummaryError>
where
    FStart: Fn(&'static str) + Send + Sync,
    FDelta: Fn(&str) + Send + Sync,
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

pub async fn summarize_dispatch_result(
    provider: &OpenAiCompatProvider,
    dispatch_result: &str,
) -> Result<String, SummaryError> {
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

    summarize_with_model(provider, build_dispatch_summary_prompt(trimmed), |_| {}).await
}

pub fn build_summary_failure_message(error: &str) -> String {
    format!(
        "检测到摘要服务调用失败，当前轮次已停止。\n\n请检查以下配置后重试：\n- Dispatcher 设置中的 API Key 是否有效\n- Dispatcher 设置中的 API Base 是否可访问\n- 云端模型 `{SUMMARY_MODEL}` 当前是否可用\n\n错误详情：\n{error}"
    )
}

async fn summarize_with_model(
    provider: &OpenAiCompatProvider,
    prompt: String,
    on_delta: impl FnMut(&str),
) -> Result<String, SummaryError> {
    let summary_provider = provider.with_model(SUMMARY_MODEL);
    let debug_context = build_summary_debug_context(&summary_provider, &prompt);
    let response = timeout(
        Duration::from_secs(SUMMARY_TIMEOUT_SECS),
        summary_provider.chat_stream(&[ChatMessage::system(prompt)], &[], on_delta),
    )
    .await
    .map_err(|_| {
        SummaryError::new(
            format!("摘要模型 `{SUMMARY_MODEL}` 调用超时（>{SUMMARY_TIMEOUT_SECS}s）"),
            debug_context.clone(),
        )
    })?
    .map_err(|error| {
        SummaryError::new(
            format!("摘要模型 `{SUMMARY_MODEL}` 调用失败：{error}"),
            debug_context.clone(),
        )
    })?;

    let content = response.content.trim().to_string();
    if content.is_empty() {
        return Err(SummaryError::new(
            format!("摘要模型 `{SUMMARY_MODEL}` 返回空结果"),
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
        "read_file" | "list_dir" | "glob" | "grep" => ToolOutputKind::Exact,
        "exec" => ToolOutputKind::Command,
        "write_file" | "edit_file" => ToolOutputKind::Mutation,
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
        build_artifact_preview, build_prompt_preview, build_summary_debug_context,
        build_summary_failure_message, decide_tool_result_action, extract_tagged_block,
        normalize_tool_output, parse_dual_tool_summary, ToolResultAction,
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
    fn failure_message_mentions_cloud_summary_model() {
        let message = build_summary_failure_message("401 unauthorized");
        assert!(message.contains("摘要服务调用失败"));
        assert!(message.contains("qwen3.6-flash"));
        assert!(message.contains("401 unauthorized"));
    }
}
