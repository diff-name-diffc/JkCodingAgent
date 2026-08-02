use tokio::time::{timeout, Duration};

use super::llm::{
    messages_contain_images, ChatMessage, ChatMessageContentPart, LlmUsage, OpenAiCompatProvider,
};

const SUMMARY_TIMEOUT_SECS: u64 = 120;
const SUMMARY_DEBUG_PREVIEW_CHARS: usize = 1_200;
const SESSION_TITLE_SOURCE_MAX_CHARS: usize = 6_000;
const SESSION_TITLE_MESSAGE_MAX_CHARS: usize = 1_200;
const SESSION_TITLE_FALLBACK_MAX_CHARS: usize = 24;
const SESSION_TITLE_CANDIDATE_SANITY_MAX_CHARS: usize = 80;
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

    /// 诊断上下文（模型/提示词预览）。当前仅用于排障日志，保留给调用方按需打印。
    #[allow(dead_code)]
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

// ─── Tool Result Summary (LLM-based, intent-aware) ─────────────────────────────

pub async fn summarize_tool_result<FUsage>(
    provider: &OpenAiCompatProvider,
    summary_model: &str,
    tool_name: &str,
    raw_output: &str,
    user_question: Option<&str>,
    compress_intent: Option<&str>,
    on_usage: FUsage,
) -> Result<ToolSummaryResult, SummaryError>
where
    FUsage: FnMut(&LlmUsage) + Send,
{
    let normalized = normalize_tool_output(raw_output);
    let normalized_trimmed = normalized.trim();

    let prompt = build_tool_summary_prompt(
        tool_name,
        normalized_trimmed,
        user_question,
        compress_intent,
    );
    let summary = summarize_with_model(
        provider,
        summary_model,
        build_text_summary_messages(prompt),
        |_| {},
        on_usage,
    )
    .await?;
    let (context_payload, display_content) = parse_dual_tool_summary(summary);

    if context_payload.is_empty() && display_content.is_empty() {
        return Err(SummaryError::new(
            format!("工具结果摘要返回空内容：{tool_name}"),
            format!(
                "tool_name={tool_name}, output_chars={}",
                raw_output.chars().count()
            ),
        ));
    }

    Ok(ToolSummaryResult {
        display_content,
        context_payload,
    })
}

/// Build the summary prompt. Two branches:
/// - With `compress_intent`: intent-focused extraction (user_question + intent + raw_output)
/// - Without intent: general high-fidelity compression with user_question context
fn build_tool_summary_prompt(
    tool_name: &str,
    raw_output: &str,
    user_question: Option<&str>,
    compress_intent: Option<&str>,
) -> String {
    let focus = tool_summary_focus(tool_name);
    let truncated_output = truncate_for_summary(raw_output);

    if let Some(intent) = compress_intent {
        let user_ctx = user_question
            .filter(|q| !q.trim().is_empty())
            .map(|q| format!("\n<用户原始问题>\n{q}\n</用户原始问题>\n"))
            .unwrap_or_default();

        return format!(
            "你是调度 Agent 的工具结果信息提取器。模型调用此工具的目的是：\n\
             <提取意图>\n{intent}\n</提取意图>\n\
             {user_ctx}\
             要求：\n\
             - 只保留与「提取意图」直接相关的事实，不要猜测，不要添加原文没有的信息。\n\
             - 与提取意图无关的信息可以忽略；相关的路径、行号、符号名、配置键、错误文本、命令结果必须保留。\n\
             - 如果内容主要是代码、配置、逐行检索结果、文件清单或其他精确检索输出，只能做最轻量压缩，严禁改写代码含义、删除关键行号、文件名或配置键；{focus}\n\
             - 输出必须严格分成两个区块，且只能使用下面的标签，不能额外添加解释、标题或 Markdown 代码块。\n\
             - `<DISPLAY_SUMMARY>`：写给前端用户展示，人类友好，用 1-3 句话概括本次工具调用针对提取意图发现了什么关键信息。\n\
             - `<CONTEXT_PAYLOAD>`：写给主模型上下文，高信息密度，保留与意图相关的关键实体名、文件路径、符号名、行号、错误文本和数量。\n\
             - 严格使用以下格式：\n\
             <DISPLAY_SUMMARY>\n...\n</DISPLAY_SUMMARY>\n\
             <CONTEXT_PAYLOAD>\n...\n</CONTEXT_PAYLOAD>\n\
             工具名：{tool_name}\n\
             工具原始输出如下：\n{truncated_output}"
        );
    }

    let user_ctx = user_question
        .filter(|q| !q.trim().is_empty())
        .map(|q| format!("当前用户问题是：\n{q}\n"))
        .unwrap_or_default();

    format!(
        "你在为调度 Agent 生成两份不同用途的工具结果摘要：一份用于继续注入模型上下文，一份用于前端展示给用户。\n\
         {user_ctx}\
         要求：\n\
         - 只保留原文里明确出现的事实，不要猜测。\n\
         - 输出必须严格分成两个区块，且只能使用下面的标签，不要额外添加解释、标题或 Markdown 代码块。\n\
         - `<DISPLAY_SUMMARY>`：写给前端展示，要求对人类更友好，聚焦结论、关键事实和为什么值得关注，可以比上下文回写更易读，但不能脱离原文事实。\n\
         - `<CONTEXT_PAYLOAD>`：写给主模型，要求高信息密度，尽量保留原始顺序、关键实体名、文件路径、符号名、配置键、错误文本、数量和退出状态；\
           如果内容主要是代码、配置、逐行检索结果、文件清单或其他精确检索输出，只能做最轻量压缩，严禁改写代码含义、删除关键行号、文件名或配置键；{focus}\n\
         - 如果内容是命令输出，优先保留命令结果、失败原因、关键日志、测试失败项和退出状态。\n\
         - 需要压缩，但不要过度归纳；宁可稍长，也不要丢掉影响后续判断的细节。\n\
         - 严格使用以下格式输出：\n\
         <DISPLAY_SUMMARY>\n...\n</DISPLAY_SUMMARY>\n\
         <CONTEXT_PAYLOAD>\n...\n</CONTEXT_PAYLOAD>\n\
         工具名：{tool_name}\n\
         工具原始输出如下：\n{truncated_output}"
    )
}

fn truncate_for_summary(raw_output: &str) -> String {
    let char_count = raw_output.chars().count();
    if char_count <= DUAL_TOOL_SUMMARY_CONTEXT_MAX_CHARS {
        return raw_output.to_string();
    }
    let truncated: String = raw_output
        .chars()
        .take(DUAL_TOOL_SUMMARY_CONTEXT_MAX_CHARS)
        .collect();
    format!(
        "{truncated}\n\n[原始输出已截断至 {DUAL_TOOL_SUMMARY_CONTEXT_MAX_CHARS} 字符，原文共 {char_count} 字符]"
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
    let context_payload = extract_tagged_block(&output, "CONTEXT_PAYLOAD", "DISPLAY_SUMMARY");
    let display_summary = extract_tagged_block(&output, "DISPLAY_SUMMARY", "CONTEXT_PAYLOAD");

    match (context_payload, display_summary) {
        (Some(context_payload), Some(display_summary)) => (context_payload, display_summary),
        // 摘要模型只产出一个区块时，另一侧复用同一内容，避免回退到带协议标签的原文。
        (Some(context_payload), None) => (context_payload.clone(), context_payload),
        (None, Some(display_summary)) => (display_summary.clone(), display_summary),
        (None, None) => {
            let fallback = strip_dual_summary_tags(&output);
            (fallback.clone(), fallback)
        }
    }
}

/// 提取 `<TAG>…</TAG>` 区块。摘要模型并不总是闭合标签（实测会出现
/// `<DISPLAY_SUMMARY>` 后直接跟 `<CONTEXT_PAYLOAD>` 的输出），因此区块
/// 也在「另一个区块的起始标签」或文本末尾处截止。起始标签缺失或内容为空时
/// 返回 None。
fn extract_tagged_block(output: &str, tag: &str, other_tag: &str) -> Option<String> {
    let start_tag = format!("<{tag}>");
    let start = output.find(&start_tag)?;
    let rest = &output[start + start_tag.len()..];
    let end = [format!("</{tag}>"), format!("<{other_tag}>")]
        .iter()
        .filter_map(|marker| rest.find(marker))
        .min()
        .unwrap_or(rest.len());
    let trimmed = rest[..end].trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// 解析彻底失败时剥掉双区块协议标签，避免 `<DISPLAY_SUMMARY>` /
/// `<CONTEXT_PAYLOAD>` 泄漏到聊天界面与模型上下文。
fn strip_dual_summary_tags(output: &str) -> String {
    output
        .replace("<DISPLAY_SUMMARY>", "")
        .replace("</DISPLAY_SUMMARY>", "")
        .replace("<CONTEXT_PAYLOAD>", "")
        .replace("</CONTEXT_PAYLOAD>", "")
        .trim()
        .to_string()
}

// ─── Rule-Based Structured Extraction (zero-LLM fallback) ──────────────────────

const STRUCTURED_SUMMARY_MAX_CHARS: usize = 8_000;
const STRUCTURED_SUMMARY_BODY_CHARS: usize = 6_000;

/// Pure rule-based extraction of key information from a tool result.
/// Zero LLM calls. Used as fallback when the summary model fails, reducing
/// the probability of the main model's content-moderation filter firing by
/// stripping large code blocks and repetitive log lines.
pub fn extract_structured_summary(tool_name: &str, raw_output: &str) -> String {
    let mut sections: Vec<String> = Vec::new();
    let char_count = raw_output.chars().count();
    let line_count = raw_output.lines().count();

    sections.push(format!(
        "[{tool_name}: {char_count} chars, {line_count} lines]"
    ));

    match tool_name {
        "exec" => {
            let lines: Vec<&str> = raw_output.lines().collect();
            if let Some(exit) = lines.iter().rev().find(|l| {
                let t = l.trim().to_lowercase();
                t.starts_with('$')
                    || t.contains("exit")
                    || t.starts_with("error")
                    || t.starts_with("退出状态")
            }) {
                sections.push(format!("退出/状态: {exit}"));
            }
            let errors: Vec<&&str> = lines
                .iter()
                .filter(|l| {
                    let t = l.trim().to_lowercase();
                    t.contains("error")
                        || t.contains("fail")
                        || t.contains("panic")
                        || t.contains("failed")
                        || t.contains("失败")
                })
                .take(10)
                .collect();
            if !errors.is_empty() {
                sections.push(format!(
                    "错误/失败:\n{}",
                    errors.iter().map(|s| **s).collect::<Vec<_>>().join("\n")
                ));
            }
            let head: Vec<&str> = lines.iter().take(20).copied().collect();
            sections.push(head.join("\n"));
            if lines.len() > 30 {
                sections.push(format!("...(省略 {} 行)...", lines.len() - 30));
                let tail: Vec<&str> = lines.iter().rev().take(10).rev().copied().collect();
                sections.push(tail.join("\n"));
            }
        }
        "read_file" => {
            let symbols: Vec<&str> = raw_output
                .lines()
                .filter(|l| {
                    // Strip leading line-number prefix (e.g., "1|  " or "42 |  ")
                    let stripped = l.split_once('|').map(|(_, rest)| rest).unwrap_or(l);
                    let t = stripped.trim();
                    t.starts_with("fn ")
                        || t.starts_with("pub fn")
                        || t.starts_with("pub(crate)")
                        || t.starts_with("async fn")
                        || t.starts_with("pub async fn")
                        || t.starts_with("class ")
                        || t.starts_with("def ")
                        || t.starts_with("import ")
                        || t.starts_with("from ")
                        || t.starts_with("const ")
                        || t.starts_with("interface ")
                        || t.starts_with("type ")
                        || t.starts_with("export ")
                        || t.starts_with("struct ")
                        || t.starts_with("enum ")
                })
                .take(50)
                .collect();
            if !symbols.is_empty() {
                sections.push(format!(
                    "符号定义:\n{}",
                    symbols.into_iter().collect::<Vec<_>>().join("\n")
                ));
            }
            sections.push(truncate_middle(
                raw_output,
                STRUCTURED_SUMMARY_BODY_CHARS,
                "代码体",
            ));
        }
        "grep" => {
            let matches: Vec<&str> = raw_output
                .lines()
                .filter(|l| l.contains(':') || l.contains("-->"))
                .take(100)
                .collect();
            sections.push(matches.into_iter().collect::<Vec<_>>().join("\n"));
            let total = raw_output.lines().count();
            if total > 100 {
                sections.push(format!("...(共 {total} 条匹配)..."));
            }
        }
        "glob" | "list_dir" => {
            let entries: Vec<&str> = raw_output.lines().take(200).collect();
            sections.push(entries.into_iter().collect::<Vec<_>>().join("\n"));
            let total = raw_output.lines().count();
            if total > 200 {
                sections.push(format!("...(共 {total} 项)..."));
            }
        }
        _ => {
            sections.push(truncate_middle(
                raw_output,
                STRUCTURED_SUMMARY_BODY_CHARS,
                "内容",
            ));
        }
    }

    let result = sections.join("\n");
    if result.chars().count() > STRUCTURED_SUMMARY_MAX_CHARS {
        truncate_middle(&result, STRUCTURED_SUMMARY_MAX_CHARS, "摘要")
    } else {
        result
    }
}

fn truncate_middle(text: &str, max_chars: usize, label: &str) -> String {
    let chars = text.chars().count();
    if chars <= max_chars {
        return text.to_string();
    }
    let half = max_chars / 2;
    let head: String = text.chars().take(half).collect();
    let tail: String = text.chars().skip(chars - half).collect();
    format!(
        "{head}\n[...省略 {} {label}字符...]\n{tail}",
        chars - max_chars
    )
}

// ─── Session Title Generation (uses LLM) ──────────────────────────────────────────

pub async fn summarize_session_title<FUsage>(
    provider: &OpenAiCompatProvider,
    summary_model: &str,
    messages: &[SessionTitleMessage],
    fallback_source: &str,
    current_user_parts: &[ChatMessageContentPart],
    on_usage: FUsage,
) -> Result<String, SummaryError>
where
    FUsage: FnMut(&LlmUsage) + Send,
{
    let prompt = build_session_title_prompt(messages, fallback_source);
    let title_messages = build_session_title_messages(prompt, current_user_parts);
    let raw_title =
        summarize_with_model(provider, summary_model, title_messages, |_| {}, on_usage).await?;

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
        build_text_summary_messages(build_keywords_prompt(qa_text, existing_keywords_json)),
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
    messages: Vec<ChatMessage>,
    on_delta: impl FnMut(&str),
    mut on_usage: impl FnMut(&LlmUsage) + Send,
) -> Result<String, SummaryError> {
    let summary_provider = provider.with_model(summary_model);
    let prompt = messages
        .first()
        .map(|message| message.content.as_str())
        .unwrap_or_default();
    let debug_context = build_summary_debug_context(&summary_provider, &prompt);
    let enable_multimodal = messages_contain_images(&messages);
    let response = timeout(
        Duration::from_secs(SUMMARY_TIMEOUT_SECS),
        summary_provider.chat_stream(&messages, &[], enable_multimodal, on_delta),
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

fn build_text_summary_messages(prompt: String) -> Vec<ChatMessage> {
    vec![ChatMessage::system(prompt)]
}

fn build_session_title_messages(
    prompt: String,
    current_user_parts: &[ChatMessageContentPart],
) -> Vec<ChatMessage> {
    let has_image = current_user_parts
        .iter()
        .any(|part| matches!(part, ChatMessageContentPart::Image { .. }));
    if !has_image {
        return build_text_summary_messages(prompt);
    }

    let mut parts = vec![ChatMessageContentPart::Text {
        text: "当前用户消息包含以下文本和图片，请结合图片内容生成标题。".to_string(),
    }];
    parts.extend(current_user_parts.iter().cloned());

    vec![
        ChatMessage::system(prompt),
        ChatMessage {
            role: "user".to_string(),
            content: "当前用户消息包含文本和图片，请结合图片内容生成标题。".to_string(),
            content_parts: parts,
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        },
    ]
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

fn build_session_title_prompt(messages: &[SessionTitleMessage], fallback_source: &str) -> String {
    let source = build_session_title_source(messages, fallback_source);
    let prompt = truncate_session_title_source(&source);
    format!(
        "你是桌面 AI 编程工具的会话标题生成器。请根据最近多条聊天消息生成一个极短中文标题。\n\
要求：\n\
- 只输出标题本身，不要解释，不要加引号、编号、Markdown 或「标题：」前缀。\n\
- 标题应相当于 5-10 个中文字符；英文缩写按一个完整词处理，不要为了凑字符数截断词语。\n\
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
        truncate_fallback_title(clean_title_line(fallback_source))
    };

    let title = clean_title_line(candidate);
    if title.is_empty()
        || title == "新会话"
        || title.chars().count() > SESSION_TITLE_CANDIDATE_SANITY_MAX_CHARS
    {
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
                '"' | '\''
                    | '`'
                    | '\u{201C}'
                    | '\u{201D}'
                    | '\u{2018}'
                    | '\u{2019}'
                    | '。'
                    | '，'
                    | ','
                    | '.'
                    | '：'
                    | ':'
            )
    });

    trimmed.split_whitespace().collect::<Vec<_>>().join("")
}

fn truncate_fallback_title(title: String) -> String {
    if title.chars().count() <= SESSION_TITLE_FALLBACK_MAX_CHARS {
        return title;
    }

    let mut truncated = title
        .chars()
        .take(SESSION_TITLE_FALLBACK_MAX_CHARS.saturating_sub(1))
        .collect::<String>();
    truncated.push('…');
    truncated
}

#[cfg(test)]
mod tests {
    use super::{normalize_session_title, parse_dual_tool_summary};

    #[test]
    fn mixed_language_title_keeps_complete_term() {
        assert_eq!(
            normalize_session_title("GPUCUDA查看命令", "查看 GPU CUDA 命令"),
            "GPUCUDA查看命令"
        );
    }

    #[test]
    fn dual_summary_parses_fully_closed_tags() {
        let (context, display) = parse_dual_tool_summary(
            "<DISPLAY_SUMMARY>\n给人看的摘要\n</DISPLAY_SUMMARY>\n<CONTEXT_PAYLOAD>\n给模型的负载\n</CONTEXT_PAYLOAD>"
                .to_string(),
        );
        assert_eq!(display, "给人看的摘要");
        assert_eq!(context, "给模型的负载");
    }

    #[test]
    fn dual_summary_tolerates_unclosed_display_tag() {
        // 实测场景：摘要模型漏掉 </DISPLAY_SUMMARY>，直接接 <CONTEXT_PAYLOAD>。
        let (context, display) = parse_dual_tool_summary(
            "<DISPLAY_SUMMARY>\n搜索命中 6 个文件。\n\n<CONTEXT_PAYLOAD>\n共 6 个文件 / 34 处匹配\n</CONTEXT_PAYLOAD>"
                .to_string(),
        );
        assert_eq!(display, "搜索命中 6 个文件。");
        assert_eq!(context, "共 6 个文件 / 34 处匹配");
    }

    #[test]
    fn dual_summary_tolerates_unclosed_context_tag() {
        let (context, display) = parse_dual_tool_summary(
            "<DISPLAY_SUMMARY>\n摘要\n</DISPLAY_SUMMARY>\n<CONTEXT_PAYLOAD>\n负载到结尾"
                .to_string(),
        );
        assert_eq!(display, "摘要");
        assert_eq!(context, "负载到结尾");
    }

    #[test]
    fn dual_summary_single_block_reuses_content_for_both_sides() {
        let (context, display) =
            parse_dual_tool_summary("<DISPLAY_SUMMARY>\n只有摘要\n</DISPLAY_SUMMARY>".to_string());
        assert_eq!(display, "只有摘要");
        assert_eq!(context, "只有摘要");
    }

    #[test]
    fn dual_summary_fallback_strips_protocol_tags() {
        let (context, display) = parse_dual_tool_summary(
            "<DISPLAY_SUMMARY>\n</DISPLAY_SUMMARY>\n正文内容 <CONTEXT_PAYLOAD>".to_string(),
        );
        assert!(!display.contains("DISPLAY_SUMMARY"));
        assert!(!display.contains("CONTEXT_PAYLOAD"));
        assert_eq!(display, "正文内容");
        assert_eq!(context, "正文内容");
    }
}

fn build_keywords_prompt(qa_text: &str, existing_keywords_json: &str) -> String {
    let qa_truncated = if qa_text.chars().count() > SESSION_KEYWORDS_QA_MAX_CHARS {
        let mut t = qa_text
            .chars()
            .take(SESSION_KEYWORDS_QA_MAX_CHARS)
            .collect::<String>();
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
