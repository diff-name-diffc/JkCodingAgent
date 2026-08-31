use anyhow::Result;
use tauri::ipc::Channel;

use super::super::db::{DispatcherDb, DispatcherMessageRecord, ToolArtifactDraft};
use super::super::llm::{LlmUsage, OpenAiCompatProvider, RequestedToolCall};
use super::super::run_loop::AgentEvent;
use super::super::tools::{ToolRegistry, ToolResultPolicy};
use super::{emit, serialize_tool_arguments};
use crate::mcp::McpScope;

// ─── Tool Result Preparation (Explicit Compression + Bounded Raw Output) ───

/// Raw tool results remain inline up to this many characters. Longer results are
/// clipped with an explicit locator while the complete output remains an artifact.
pub const TOOL_RESULT_INLINE_MAX_CHARS: usize = 8_000;

/// 读取类工具未显式分页时的内联上限：读取结果通常是要精读的正文或检索命中，
/// 8000 字符仍可能不够。
pub const TOOL_RESULT_INLINE_MAX_CHARS_READ: usize = 10_000;

/// 显式按行分页读取时的内联上限。分页本身就是模型在主动控制读取粒度，
/// 一页只给 8000 字符会导致接续读取的往返次数过多。
pub const TOOL_RESULT_INLINE_MAX_CHARS_PAGED: usize = 20_000;

/// 内容读取类工具：结果主体是供模型精读的文本。这些工具享有两档内联预算——
/// 默认 READ（10000），显式传入 offset/limit 分页读取时 PAGED（20000）；
/// 其余工具维持 8000。
/// 注意：effective_args 会注入 schema default，列入此处的工具其 offset/limit
/// 参数不得声明 default，否则无法区分显式分页与默认读取。
const INLINE_READ_TOOLS: &[&str] = &[
    "read_file",
    "browser_read_text",
    "grep",
    "glob",
    "list_dir",
    "graph_plan_report",
];

/// 按工具与入参决定本次调用的内联字符上限。
fn inline_max_chars(tool_name: &str, args: &serde_json::Value) -> usize {
    if !INLINE_READ_TOOLS.contains(&tool_name) {
        return TOOL_RESULT_INLINE_MAX_CHARS;
    }
    if args.get("offset").is_some() || args.get("limit").is_some() {
        TOOL_RESULT_INLINE_MAX_CHARS_PAGED
    } else {
        TOOL_RESULT_INLINE_MAX_CHARS_READ
    }
}

/// 未携带 ToolSpec 的旧调用/纯函数测试使用的保守摘要阈值。
#[cfg(test)]
pub const TOOL_RESULT_SUMMARY_MIN_CHARS: usize = 5_000;

pub(super) struct PreparedToolResult {
    pub display_content: String,
    pub context_payload: String,
    pub result_mode: &'static str,
    pub raw_output: String,
    pub needs_summary: bool,
    /// 模型调用工具时声明的信息提取意图（一句话描述期望从结果中提取什么）
    pub compress_intent: Option<String>,
}

#[cfg(test)]
pub(super) fn prepare_tool_result(
    tool_name: &str,
    args: &serde_json::Value,
    raw_output: &str,
) -> PreparedToolResult {
    prepare_tool_result_with_policy(
        tool_name,
        args,
        raw_output,
        &ToolResultPolicy {
            default_compress: false,
            force_compress_after_chars: TOOL_RESULT_SUMMARY_MIN_CHARS,
            persist_raw_artifact: true,
        },
    )
}

fn prepare_tool_result_with_policy(
    tool_name: &str,
    args: &serde_json::Value,
    raw_output: &str,
    policy: &ToolResultPolicy,
) -> PreparedToolResult {
    let trimmed = raw_output.trim();
    if trimmed.is_empty() {
        return PreparedToolResult {
            display_content: String::new(),
            context_payload: String::new(),
            result_mode: "raw",
            raw_output: String::new(),
            needs_summary: false,
            compress_intent: None,
        };
    }

    let model_compress = args
        .get("compress")
        .and_then(|v| v.as_bool())
        .unwrap_or(policy.default_compress);
    let compress_intent = args
        .get("compress_intent")
        .and_then(|v| v.as_str())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let char_count = trimmed.chars().count();
    let max_inline = inline_max_chars(tool_name, args);
    let needs_summary = model_compress && char_count > policy.force_compress_after_chars;

    if needs_summary {
        PreparedToolResult {
            display_content: String::new(),
            context_payload: String::new(),
            result_mode: "pending_summary",
            raw_output: trimmed.to_string(),
            needs_summary: true,
            compress_intent,
        }
    } else if char_count > max_inline {
        let truncated = truncate_tool_result(trimmed, char_count, max_inline);
        PreparedToolResult {
            display_content: truncated.clone(),
            context_payload: truncated,
            result_mode: "truncated",
            raw_output: trimmed.to_string(),
            needs_summary: false,
            compress_intent,
        }
    } else {
        PreparedToolResult {
            display_content: trimmed.to_string(),
            context_payload: trimmed.to_string(),
            result_mode: "raw",
            raw_output: trimmed.to_string(),
            needs_summary: false,
            compress_intent,
        }
    }
}

fn truncate_tool_result(raw_output: &str, char_count: usize, max_chars: usize) -> String {
    let prefix = raw_output.chars().take(max_chars).collect::<String>();
    let truncated_at_output_line = prefix.chars().filter(|ch| *ch == '\n').count() + 1;
    let total_lines = raw_output.lines().count().max(1);
    let source_line_marker = source_line_number_at_cut(&prefix)
        .map(|line| format!("（该行标注的源码/匹配行号为 {line}）"))
        .unwrap_or_default();

    format!(
        "{prefix}\n\n[结果已截断：仅返回前 {max_chars} / {char_count} 字符；截断发生在原始结果第 {truncated_at_output_line} 个输出行{source_line_marker}，原始结果共 {total_lines} 行。完整原始结果见工具产物。]"
    )
}

fn source_line_number_at_cut(prefix: &str) -> Option<usize> {
    let current_line = prefix.rsplit('\n').next()?.trim_start();
    let digit_count = current_line
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .count();
    if digit_count == 0 {
        return None;
    }
    let (digits, suffix) = current_line.split_at(digit_count);
    matches!(suffix.chars().next(), Some('|' | ':' | '-'))
        .then(|| digits.parse::<usize>().ok())
        .flatten()
}

fn bound_inline_tool_result(content: String) -> String {
    // 摘要结果的展示上限保持紧凑值：压缩后的内容本就该足够精炼。
    let char_count = content.chars().count();
    if char_count > TOOL_RESULT_INLINE_MAX_CHARS {
        truncate_tool_result(&content, char_count, TOOL_RESULT_INLINE_MAX_CHARS)
    } else {
        content
    }
}

// ─── Tool Result Persistence ─────────────────────────────────────────────────────

/// Shared summary-aware tool result persistence used by both dispatcher and plain chat.
/// Calls the summary model only when `compress=true` and the result exceeds
/// `TOOL_RESULT_SUMMARY_MIN_CHARS`. Otherwise large inline results are explicitly
/// truncated while their complete raw output remains available as an artifact.
#[allow(clippy::too_many_arguments)]
pub async fn persist_tool_result_with_compression<FUsage>(
    db: &DispatcherDb,
    workspace_id: &str,
    on_event: &Channel<AgentEvent>,
    tool_call: &RequestedToolCall,
    registry: &ToolRegistry,
    scope: &McpScope,
    result: &str,
    summary_provider: &OpenAiCompatProvider,
    summary_model: &str,
    on_usage: FUsage,
) -> Result<DispatcherMessageRecord>
where
    FUsage: FnMut(&LlmUsage) + Send,
{
    use crate::agent::summary::{extract_structured_summary, summarize_tool_result};

    // Compression policy must use the same schema-expanded arguments as execution;
    // otherwise an omitted default could behave differently after the tool returns.
    let effective_arguments = registry.effective_args(&tool_call.name, &tool_call.arguments);
    let result_policy = registry
        .spec_by_name(scope, &tool_call.name, true)
        .map(|spec| spec.result_policy)
        .unwrap_or_else(|| ToolResultPolicy::new(false));
    // G9-14：序列化失败不再静默降级为 `{}`——错误上抛，由运行循环以 Failed 收口，
    // 保证模型/前端看到的参数与工具实际执行所用的 effective_args 永远一致。
    let arguments_json = serialize_tool_arguments(&tool_call.name, &effective_arguments)?;
    let prepared = prepare_tool_result_with_policy(
        &tool_call.name,
        &effective_arguments,
        result,
        &result_policy,
    );

    if !prepared.needs_summary {
        let artifacts = result_policy
            .persist_raw_artifact
            .then(|| build_tool_artifact(&tool_call.name, result))
            .into_iter()
            .collect::<Vec<_>>();
        let tool_message = db
            .add_visible_tool_result_async(
                workspace_id,
                &prepared.display_content,
                &prepared.context_payload,
                Some(&tool_call.id),
                Some(&tool_call.name),
                Some(prepared.result_mode),
                &artifacts,
            )
            .await?;
        emit(
            on_event,
            AgentEvent::ToolFinished {
                tool_call_id: tool_call.id.clone(),
                name: tool_call.name.clone(),
                arguments: arguments_json,
                display_text: tool_message.plain_text(),
                context_payload: prepared.context_payload.clone(),
                result_mode: prepared.result_mode.to_string(),
                detail_refs: tool_message.tool_artifacts.clone(),
            },
        );
        return Ok(tool_message);
    }

    let user_question = db
        .get_latest_user_message_content_async(workspace_id)
        .await
        .ok()
        .flatten();
    let user_question_ref = user_question.as_deref();

    match summarize_tool_result(
        summary_provider,
        summary_model,
        &tool_call.name,
        &prepared.raw_output,
        user_question_ref,
        prepared.compress_intent.as_deref(),
        on_usage,
    )
    .await
    {
        Ok(summary) => {
            let mode = if prepared.compress_intent.is_some() {
                "intent_compressed"
            } else {
                "conservative_summary"
            };
            persist_tool_result_with_summary(
                db,
                workspace_id,
                on_event,
                tool_call,
                &arguments_json,
                result,
                ToolResultPresentation {
                    display_content: summary.display_content,
                    context_payload: summary.context_payload,
                    result_mode: mode,
                    persist_raw_artifact: result_policy.persist_raw_artifact,
                },
            )
            .await
        }
        Err(error) => {
            eprintln!(
                "summarize_tool_result failed for {}: {}, falling back to structured extraction",
                tool_call.name,
                error.message()
            );
            let structured = extract_structured_summary(&tool_call.name, &prepared.raw_output);
            persist_tool_result_with_summary(
                db,
                workspace_id,
                on_event,
                tool_call,
                &arguments_json,
                result,
                ToolResultPresentation {
                    display_content: structured.clone(),
                    context_payload: structured,
                    result_mode: "structured_fallback",
                    persist_raw_artifact: result_policy.persist_raw_artifact,
                },
            )
            .await
        }
    }
}

struct ToolResultPresentation {
    pub display_content: String,
    pub context_payload: String,
    pub result_mode: &'static str,
    pub persist_raw_artifact: bool,
}

async fn persist_tool_result_with_summary(
    db: &DispatcherDb,
    workspace_id: &str,
    on_event: &Channel<AgentEvent>,
    tool_call: &RequestedToolCall,
    arguments_json: &str,
    result: &str,
    presentation: ToolResultPresentation,
) -> Result<DispatcherMessageRecord> {
    let ToolResultPresentation {
        display_content,
        context_payload,
        result_mode,
        persist_raw_artifact,
    } = presentation;
    let display_content = bound_inline_tool_result(display_content);
    let context_payload = bound_inline_tool_result(context_payload);
    let artifacts = persist_raw_artifact
        .then(|| build_tool_artifact(&tool_call.name, result))
        .into_iter()
        .collect::<Vec<_>>();
    let tool_message = db
        .add_visible_tool_result_async(
            workspace_id,
            &display_content,
            &context_payload,
            Some(&tool_call.id),
            Some(&tool_call.name),
            Some(result_mode),
            &artifacts,
        )
        .await?;

    emit(
        on_event,
        AgentEvent::ToolFinished {
            tool_call_id: tool_call.id.clone(),
            name: tool_call.name.clone(),
            arguments: arguments_json.to_string(),
            display_text: tool_message.plain_text(),
            context_payload: tool_message
                .context_payload
                .clone()
                .unwrap_or_else(|| tool_message.plain_text()),
            result_mode: result_mode.to_string(),
            detail_refs: tool_message.tool_artifacts.clone(),
        },
    );

    Ok(tool_message)
}

fn build_tool_artifact(tool_name: &str, raw_output: &str) -> ToolArtifactDraft {
    ToolArtifactDraft::raw_tool_output(tool_name, raw_output)
}
