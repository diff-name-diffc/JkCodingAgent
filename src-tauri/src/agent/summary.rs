use tokio::time::{timeout, Duration};

use super::llm::{ChatMessage, LlmUsage, OpenAiCompatProvider};

const SUMMARY_MODE_THRESHOLD_CHARS: usize = 240;
const SUMMARY_MODE_THRESHOLD_LINES: usize = 24;
const SUMMARY_TIMEOUT_SECS: u64 = 120;
const SUMMARY_DEBUG_PREVIEW_CHARS: usize = 1_200;
const SESSION_TITLE_SOURCE_MAX_CHARS: usize = 6_000;
const SESSION_TITLE_MESSAGE_MAX_CHARS: usize = 1_200;
const SESSION_TITLE_MAX_CHARS: usize = 10;
const SESSION_KEYWORDS_QA_MAX_CHARS: usize = 3_000;
const SESSION_KEYWORDS_MAX: usize = 15;

pub const DUAL_TOOL_SUMMARY_CONTEXT_MAX_CHARS: usize = 24_000;

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
pub struct ToolSummaryResult {
    pub display_content: String,
    pub context_payload: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionTitleMessage {
    pub role: String,
    pub content: String,
}

// ─── Dispatch Result Summary (still uses LLM for subprocess result compression) ──

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

// ─── Tool Result Summary (LLM-based, only for large tool outputs) ───────────────

pub async fn summarize_tool_result<FUsage>(
    provider: &OpenAiCompatProvider,
    summary_model: &str,
    tool_name: &str,
    raw_output: &str,
    on_usage: FUsage,
) -> Result<ToolSummaryResult, SummaryError>
where
    FUsage: FnMut(&LlmUsage) + Send,
{
    let normalized = normalize_tool_output(raw_output);
    let normalized_trimmed = normalized.trim();

    let prompt = build_dual_tool_summary_prompt(tool_name, normalized_trimmed);
    let summary = summarize_with_model(provider, summary_model, prompt, |_| {}, on_usage).await?;
    let (context_payload, display_content) = parse_dual_tool_summary(summary);

    if context_payload.is_empty() && display_content.is_empty() {
        return Err(SummaryError::new(
            format!("工具结果摘要返回空内容：{tool_name}"),
            format!("tool_name={tool_name}, output_chars={}", raw_output.chars().count()),
        ));
    }

    Ok(ToolSummaryResult {
        display_content,
        context_payload,
    })
}

fn build_dual_tool_summary_prompt(tool_name: &str, raw_output: &str) -> String {
    let focus = tool_summary_focus(tool_name);
    let truncated_output = if raw_output.chars().count() > DUAL_TOOL_SUMMARY_CONTEXT_MAX_CHARS {
        let truncated = raw_output.chars().take(DUAL_TOOL_SUMMARY_CONTEXT_MAX_CHARS).collect::<String>();
        format!("{truncated}\n\n[原始输出已截断至 {} 字符，原文共 {} 字符]",
            truncated.chars().count(), raw_output.chars().count())
    } else {
        raw_output.to_string()
    };

    format!(
        "你在为调度 Agent 生成两份不同用途的工具结果：一份用于继续注入模型上下文，一份用于前端展示给用户。\n\
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
工具原始输出如下：\n{truncated_output}"
    )
}

fn tool_summary_focus(tool_name: &str) -> &str {
    match tool_name {
        "read_file" => "保留关键文件路径、符号名、行号范围、配置键和能支持判断的核心实现细节",
        "list_dir" | "glob" => "保留目录层级、关键文件名、数量和显著的结构特征",
        "grep" => "保留匹配文件路径、行号、命中片段、上下文和能支撑后续 read_file 的关键关键词",
        "exec" => "保留命令结果、错误文本、失败项、退出状态、关键路径和数量统计",
        _ => "保留后续判断最依赖的事实、路径、标识符和数量信息",
    }
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

// ─── Session Title Generation (uses LLM) ──────────────────────────────────────────

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

// ─── Session Keywords (uses LLM) ──────────────────────────────────────────────────

pub async fn summarize_session_keywords<FUsage>(
    provider: &OpenAiCompatProvider,
    summary_model: &str,
    qa_text: &str,
    existing_keywords_json: &str,
    on_usage: FUsage,
) -> Result<String, SummaryError>
where
    FUsage: FnMut(&LlmUsage) + Send,
{
    summarize_with_model(
        provider,
        summary_model,
        build_keywords_prompt(qa_text, existing_keywords_json),
        |_| {},
        on_usage,
    )
    .await
}

pub fn parse_keyword_actions(raw: &str) -> Vec<super::db::KeywordAction> {
    let text = raw.trim();
    let json_str = if text.starts_with("```") {
        text.lines()
            .skip(1)
            .take_while(|line| !line.trim().starts_with("```"))
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        text.to_string()
    };
    serde_json::from_str::<Vec<super::db::KeywordAction>>(&json_str).unwrap_or_default()
}

// ─── Internal Helpers ─────────────────────────────────────────────────────────────

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
- 只输出标题本身，不要解释，不要加引号、编号、Markdown 或「标题：」前缀。\n\
- 标题必须是 5-10 个中文字符；不要超过 10 个字符。\n\
- 标题应是名词短语，概括最后一轮完整对话的核心任务；如果最后用户消息是「另外/继续/这个」等追加要求，必须结合前文对象。\n\
- 优先保留关键模块、功能、错误或对象；删除「帮我」「看看」「优化一下」「问题」「任务」等水词。\n\
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
                '"' | '\'' | '`' | '\u{201C}' | '\u{201D}' | '\u{2018}' | '\u{2019}'
                    | '。' | '，' | ',' | '.' | '：' | ':'
            )
    });

    trimmed.split_whitespace().collect::<Vec<_>>().join("")
}

fn truncate_title(title: String) -> String {
    title.chars().take(SESSION_TITLE_MAX_CHARS).collect()
}

fn build_keywords_prompt(qa_text: &str, existing_keywords_json: &str) -> String {
    let qa_truncated = if qa_text.chars().count() > SESSION_KEYWORDS_QA_MAX_CHARS {
        let mut t = qa_text.chars().take(SESSION_KEYWORDS_QA_MAX_CHARS).collect::<String>();
        t.push_str("\n...(truncated)");
        t
    } else {
        qa_text.to_string()
    };
    format!(
        "你是一个会话关键字维护助手。你的任务是根据最新一轮对话内容，维护一组关键字来描述这个会话的主题。

现有关键字（JSON 数组）：
{existing_keywords_json}

最新一轮对话（用户 + AI 助手的一问一答）：
{qa_truncated}

规则：
1. 只输出 JSON 数组，不要添加其他任何内容（不要 markdown 代码块包裹）
2. 最多 {max_keywords} 个关键字
3. 关键字必须简洁：2-20 字符的术语或短语
4. 保留仍然相关的关键字（\"keep\"）
5. 添加新出现的主题、技术、工具、概念（\"add\"），权重 1-10
6. 相似关键字可以合并为一个（\"merge\"）
7. 不再相关的旧关键字标记为删除（\"remove\"）
8. 代码标识符（函数名、类名、变量名）优先
9. 文件名/路径保留最后一级

输出格式（严格 JSON）：
[
  {{\"action\":\"keep\",\"keyword\":\"原关键字\"}},
  {{\"action\":\"add\",\"keyword\":\"新关键字\",\"weight\":7.5}},
  {{\"action\":\"merge\",\"from\":[\"旧1\",\"旧2\"],\"to\":\"合并后\",\"weight\":6.0}},
  {{\"action\":\"remove\",\"keyword\":\"要删除的关键字\"}}
]",
        max_keywords = SESSION_KEYWORDS_MAX
    )
}

fn exceeds_limits(raw_output: &str, max_chars: usize, max_lines: usize) -> bool {
    raw_output.chars().count() > max_chars || raw_output.lines().count() > max_lines
}

#[cfg(test)]
mod tests {
    use super::{
        build_dispatch_summary_prompt, build_prompt_preview, build_session_title_prompt,
        build_session_title_source, build_summary_debug_context, exceeds_limits,
        fallback_session_title, normalize_session_title, session_title_role_label,
        truncate_session_title_message, truncate_session_title_source,
        truncate_title, clean_title_line, SessionTitleMessage, SESSION_TITLE_MAX_CHARS,
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
    fn build_dispatch_summary_prompt_contains_required_structure() {
        let prompt = build_dispatch_summary_prompt("test result");
        assert!(prompt.contains("【子任务回流摘要】"));
        assert!(prompt.contains("状态："));
        assert!(prompt.contains("已完成："));
        assert!(prompt.contains("阻塞/风险："));
        assert!(prompt.contains("关键证据："));
        assert!(prompt.contains("建议下一步："));
        assert!(prompt.contains("test result"));
    }

    #[test]
    fn exceeds_limits_detects_char_limit() {
        let text = "a".repeat(100);
        assert!(exceeds_limits(&text, 99, 200));
        assert!(!exceeds_limits(&text, 101, 200));
    }

    #[test]
    fn exceeds_limits_detects_line_limit() {
        let text = "line1\nline2\nline3";
        assert!(exceeds_limits(text, 100, 2));
        assert!(!exceeds_limits(text, 100, 4));
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
        let title = fallback_session_title("请帮我实现一个非常非常非常非常非常非常长的会话标题生成逻辑");
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
        assert!(prompt.contains("如果最后用户消息是"));
        assert!(prompt.contains("【用户】\n优化聊天 Markdown 代码块样式"));
        assert!(prompt.contains("【助手】\n已调整亮色和暗色代码块主题。"));
    }

    #[test]
    fn session_title_source_falls_back_when_dialogue_is_empty() {
        let source = build_session_title_source(&[], "修复标题生成");
        assert_eq!(source, "修复标题生成");
    }

    #[test]
    fn session_title_source_builds_from_messages() {
        let messages = vec![
            SessionTitleMessage {
                role: "user".to_string(),
                content: "hello".to_string(),
            },
            SessionTitleMessage {
                role: "assistant".to_string(),
                content: "world".to_string(),
            },
        ];
        let source = build_session_title_source(&messages, "fallback");
        assert!(source.contains("【用户】"));
        assert!(source.contains("hello"));
        assert!(source.contains("【助手】"));
        assert!(source.contains("world"));
    }

    #[test]
    fn session_title_role_label_maps_known_roles() {
        assert_eq!(session_title_role_label("user"), "用户");
        assert_eq!(session_title_role_label("assistant"), "助手");
        assert_eq!(session_title_role_label("tool"), "工具结果");
        assert_eq!(session_title_role_label("system"), "消息");
    }

    #[test]
    fn truncate_session_title_message_short_input_unchanged() {
        let msg = "short message";
        let truncated = truncate_session_title_message(msg);
        assert_eq!(truncated, msg);
    }

    #[test]
    fn truncate_session_title_source_short_input_unchanged() {
        let source = "short source content";
        let truncated = truncate_session_title_source(source);
        assert_eq!(truncated, source);
    }

    #[test]
    fn clean_title_line_strips_markdown_prefixes() {
        assert_eq!(clean_title_line("# Hello"), "Hello");
        assert_eq!(clean_title_line("## World"), "World");
        assert_eq!(clean_title_line("> Quote"), "Quote");
    }

    #[test]
    fn clean_title_line_strips_chinese_title_prefix() {
        assert_eq!(clean_title_line("标题：测试标题"), "测试标题");
        assert_eq!(clean_title_line("标题:测试标题"), "测试标题");
    }

    #[test]
    fn clean_title_line_returns_empty_for_empty_input() {
        assert_eq!(clean_title_line(""), "");
        assert_eq!(clean_title_line("   "), "");
    }

    #[test]
    fn truncate_title_caps_at_max_chars() {
        let long = "很长的标题".repeat(5);
        let truncated = truncate_title(long);
        assert_eq!(truncated.chars().count(), SESSION_TITLE_MAX_CHARS);
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
    fn normalize_session_title_returns_fallback_when_model_returns_empty() {
        let title = normalize_session_title("", "用户原始问题");
        assert_eq!(title, "用户原始问题");
    }
}
