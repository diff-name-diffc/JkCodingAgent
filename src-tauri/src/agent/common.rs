use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use parking_lot::Mutex;
use tokio::sync::watch;

use tauri::ipc::Channel;

use super::db::{
    DispatcherDb, DispatcherMessageRecord, DispatcherMessageUsageStats,
    DispatcherSessionTokenUsageSource, ToolArtifactDraft,
};
use super::llm::{
    messages_contain_images, ChatMessage, FunctionCall, LlmResponse, LlmUsage,
    OpenAiCompatProvider, OutboundToolCall, RequestedToolCall, ToolDefinition,
};
use super::run_loop::AgentEvent;
use super::tools::ToolRegistry;

// ─── Usage Tracking ────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct UsageTracker {
    pub started_at: Instant,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    paused_at: Option<Instant>,
    paused_accum_ms: u64,
}

impl UsageTracker {
    pub fn new() -> Self {
        Self {
            started_at: Instant::now(),
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
            paused_at: None,
            paused_accum_ms: 0,
        }
    }

    pub fn record(&mut self, usage: &LlmUsage) -> DispatcherMessageUsageStats {
        self.prompt_tokens += usage.prompt_tokens;
        self.completion_tokens += usage.completion_tokens;
        self.total_tokens += normalized_total_tokens(usage);
        self.snapshot()
    }

    pub fn snapshot(&self) -> DispatcherMessageUsageStats {
        let mut elapsed = self.started_at.elapsed();
        if let Some(paused_at) = self.paused_at {
            elapsed = elapsed.saturating_sub(paused_at.elapsed());
        }
        elapsed = elapsed.saturating_sub(Duration::from_millis(self.paused_accum_ms));
        DispatcherMessageUsageStats {
            prompt_tokens: self.prompt_tokens,
            completion_tokens: self.completion_tokens,
            total_tokens: self.total_tokens,
            elapsed_ms: elapsed.as_millis() as u64,
            paused: self.paused_at.is_some(),
        }
    }

    /// Pause the usage timer. Sub-agent execution time should not inflate
    /// the main agent's token generation speed denominator.
    pub fn pause(&mut self) {
        if self.paused_at.is_none() {
            self.paused_at = Some(Instant::now());
        }
    }

    /// Resume the usage timer after a sub-agent call completes.
    pub fn resume(&mut self) {
        if let Some(paused_at) = self.paused_at.take() {
            self.paused_accum_ms += paused_at.elapsed().as_millis() as u64;
        }
    }
}

/// Runs `execute` with the main agent's usage timer paused, then emits a
/// `RunUsageUpdated` event so the frontend can stop padding live elapsed.
/// Used to wrap `call_sub_agent` execution: the sub-agent's wall-clock time
/// must not dilute the main agent's token-generation-speed denominator.
pub async fn with_usage_paused<F, Fut, T>(
    usage_tracker: &mut UsageTracker,
    workspace_id: &str,
    on_event: &Channel<AgentEvent>,
    execute: F,
) -> T
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = T>,
{
    usage_tracker.pause();
    emit(
        on_event,
        AgentEvent::RunUsageUpdated {
            workspace_id: workspace_id.to_string(),
            stats: usage_tracker.snapshot(),
        },
    );
    let result = execute().await;
    usage_tracker.resume();
    emit(
        on_event,
        AgentEvent::RunUsageUpdated {
            workspace_id: workspace_id.to_string(),
            stats: usage_tracker.snapshot(),
        },
    );
    result
}

// ─── LLM Streaming ──────────────────────────────────────────────────────────────

pub enum LlmStreamOutcome {
    Cancelled(String),
    Response(LlmResponse),
}

#[allow(clippy::too_many_arguments)]
pub async fn stream_llm_response(
    db: &DispatcherDb,
    workspace_id: &str,
    model: &str,
    source_kind: DispatcherSessionTokenUsageSource,
    usage_tracker: &mut UsageTracker,
    on_event: &Channel<AgentEvent>,
    provider: &OpenAiCompatProvider,
    messages: &[ChatMessage],
    tool_definitions: &[ToolDefinition],
    cancel_rx: watch::Receiver<bool>,
) -> Result<LlmStreamOutcome> {
    let stream_msg_id = uuid::Uuid::new_v4().to_string();
    emit(
        on_event,
        AgentEvent::AssistantStarted {
            message_id: stream_msg_id.clone(),
        },
    );

    let streamed_text = Arc::new(Mutex::new(String::new()));
    let msg_id = stream_msg_id.clone();
    let streamed_text_clone = Arc::clone(&streamed_text);
    let on_delta = move |delta: &str| {
        streamed_text_clone.lock().push_str(delta);
        let _ = on_event.send(AgentEvent::AssistantDelta {
            message_id: msg_id.clone(),
            delta: delta.to_string(),
        });
    };

    let thinking_msg_id = stream_msg_id.clone();
    let on_thinking_delta = move |delta: &str, elapsed_ms: u64| {
        let _ = on_event.send(AgentEvent::AssistantThinkingDelta {
            message_id: thinking_msg_id.clone(),
            delta: delta.to_string(),
            elapsed_ms,
        });
    };

    let mut stream_cancel_rx = cancel_rx;
    let response = tokio::select! {
        _ = wait_for_cancellation(&mut stream_cancel_rx) => {
            let partial = streamed_text.lock().clone();
            return Ok(LlmStreamOutcome::Cancelled(partial));
        }
        response = provider.chat_stream_with_thinking(
            messages,
            tool_definitions,
            messages_contain_images(messages),
            on_delta,
            on_thinking_delta,
        ) => response
    }?;

    if let Some(usage) = response.usage.as_ref() {
        record_usage(
            db,
            workspace_id,
            model,
            source_kind,
            usage,
            usage_tracker,
            on_event,
        );
    }

    Ok(LlmStreamOutcome::Response(response))
}

fn record_usage(
    db: &DispatcherDb,
    workspace_id: &str,
    model: &str,
    source_kind: DispatcherSessionTokenUsageSource,
    usage: &LlmUsage,
    tracker: &mut UsageTracker,
    on_event: &Channel<AgentEvent>,
) {
    let db = db.clone();
    let wid = workspace_id.to_string();
    let m = model.to_string();
    let u = usage.clone();
    tokio::spawn(async move {
        if let Err(error) = db
            .upsert_session_token_usage_async(&wid, &m, source_kind, &u)
            .await
        {
            eprintln!(
                "failed to persist session token usage for workspace {} and model {}: {}",
                wid, m, error
            );
        }
    });

    let stats = tracker.record(usage);
    emit(
        on_event,
        AgentEvent::RunUsageUpdated {
            workspace_id: workspace_id.to_string(),
            stats,
        },
    );
}

fn normalized_total_tokens(usage: &LlmUsage) -> u64 {
    if usage.total_tokens > 0 {
        usage.total_tokens
    } else {
        usage.prompt_tokens + usage.completion_tokens
    }
}

// ─── Tool Result Preparation (Explicit Compression + Bounded Raw Output) ───

/// Raw tool results remain inline up to this many characters. Longer results are
/// clipped with an explicit locator while the complete output remains an artifact.
pub const TOOL_RESULT_INLINE_MAX_CHARS: usize = 2_000;

/// Semantic compression is worthwhile only for genuinely large results and must
/// always be explicitly enabled by the tool call's `compress` argument.
pub const TOOL_RESULT_SUMMARY_MIN_CHARS: usize = 5_000;

pub struct PreparedToolResult {
    pub display_content: String,
    pub context_payload: String,
    pub result_mode: &'static str,
    pub raw_output: String,
    pub needs_summary: bool,
    /// 模型调用工具时声明的信息提取意图（一句话描述期望从结果中提取什么）
    pub compress_intent: Option<String>,
}

pub fn prepare_tool_result(
    _tool_name: &str,
    args: &serde_json::Value,
    raw_output: &str,
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
        .unwrap_or(false);
    let compress_intent = args
        .get("compress_intent")
        .and_then(|v| v.as_str())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let char_count = trimmed.chars().count();
    let needs_summary = model_compress && char_count > TOOL_RESULT_SUMMARY_MIN_CHARS;

    if needs_summary {
        PreparedToolResult {
            display_content: String::new(),
            context_payload: String::new(),
            result_mode: "pending_summary",
            raw_output: trimmed.to_string(),
            needs_summary: true,
            compress_intent,
        }
    } else if char_count > TOOL_RESULT_INLINE_MAX_CHARS {
        let truncated = truncate_tool_result(trimmed, char_count);
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

fn truncate_tool_result(raw_output: &str, char_count: usize) -> String {
    let prefix = raw_output
        .chars()
        .take(TOOL_RESULT_INLINE_MAX_CHARS)
        .collect::<String>();
    let truncated_at_output_line = prefix.chars().filter(|ch| *ch == '\n').count() + 1;
    let total_lines = raw_output.lines().count().max(1);
    let source_line_marker = source_line_number_at_cut(&prefix)
        .map(|line| format!("（该行标注的源码/匹配行号为 {line}）"))
        .unwrap_or_default();

    format!(
        "{prefix}\n\n[结果已截断：仅返回前 {TOOL_RESULT_INLINE_MAX_CHARS} / {char_count} 字符；截断发生在原始结果第 {truncated_at_output_line} 个输出行{source_line_marker}，原始结果共 {total_lines} 行。完整原始结果见工具产物。]"
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
    let char_count = content.chars().count();
    if char_count > TOOL_RESULT_INLINE_MAX_CHARS {
        truncate_tool_result(&content, char_count)
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
    let prepared = prepare_tool_result(&tool_call.name, &effective_arguments, result);

    if !prepared.needs_summary {
        let tool_message = db
            .add_visible_tool_result_async(
                workspace_id,
                &prepared.display_content,
                &prepared.context_payload,
                Some(&tool_call.id),
                Some(&tool_call.name),
                Some(prepared.result_mode),
                &[build_tool_artifact(&tool_call.name, result)],
            )
            .await?;
        emit(
            on_event,
            AgentEvent::ToolFinished {
                tool_call_id: Some(tool_call.id.clone()),
                name: tool_call.name.clone(),
                display_text: tool_message.content.clone(),
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
                result,
                summary.display_content,
                summary.context_payload,
                mode,
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
                result,
                structured.clone(),
                structured,
                "structured_fallback",
            )
            .await
        }
    }
}

pub async fn persist_tool_result_with_summary(
    db: &DispatcherDb,
    workspace_id: &str,
    on_event: &Channel<AgentEvent>,
    tool_call: &RequestedToolCall,
    result: &str,
    display_content: String,
    context_payload: String,
    result_mode: &'static str,
) -> Result<DispatcherMessageRecord> {
    let display_content = bound_inline_tool_result(display_content);
    let context_payload = bound_inline_tool_result(context_payload);
    let tool_message = db
        .add_visible_tool_result_async(
            workspace_id,
            &display_content,
            &context_payload,
            Some(&tool_call.id),
            Some(&tool_call.name),
            Some(result_mode),
            &[build_tool_artifact(&tool_call.name, result)],
        )
        .await?;

    emit(
        on_event,
        AgentEvent::ToolFinished {
            tool_call_id: Some(tool_call.id.clone()),
            name: tool_call.name.clone(),
            display_text: tool_message.content.clone(),
            result_mode: result_mode.to_string(),
            detail_refs: tool_message.tool_artifacts.clone(),
        },
    );

    Ok(tool_message)
}

// ─── Assistant Message Persistence ───────────────────────────────────────────────

pub async fn persist_assistant_message(
    db: &DispatcherDb,
    workspace_id: &str,
    content: &str,
    usage_stats: &DispatcherMessageUsageStats,
) -> Result<DispatcherMessageRecord> {
    db.add_visible_message_with_usage_async(workspace_id, "assistant", content, usage_stats)
        .await
}

pub async fn persist_tool_calls_message(
    db: &DispatcherDb,
    workspace_id: &str,
    content: &str,
    tool_calls: &[OutboundToolCall],
    thinking_content: &str,
    thinking_elapsed_ms: Option<u64>,
) -> Result<DispatcherMessageRecord> {
    db.add_visible_message_with_tools_and_thinking_async(
        workspace_id,
        "assistant",
        content,
        None,
        None,
        None,
        Some(tool_calls),
        if thinking_content.is_empty() {
            None
        } else {
            Some(thinking_content)
        },
        thinking_elapsed_ms.unwrap_or(0),
    )
    .await
}

pub fn build_tool_calls_payload(
    tool_calls: &[RequestedToolCall],
    registry: &ToolRegistry,
) -> Vec<OutboundToolCall> {
    tool_calls
        .iter()
        .map(|call| {
            let enriched = registry.effective_args(&call.name, &call.arguments);
            let args_json = serde_json::to_string(&enriched).unwrap_or_else(|_| "{}".to_string());
            OutboundToolCall {
                id: call.id.clone(),
                kind: "function".to_string(),
                function: FunctionCall {
                    name: call.name.clone(),
                    arguments: args_json,
                },
            }
        })
        .collect()
}

pub fn build_args_map(
    tool_calls: &[RequestedToolCall],
    registry: &ToolRegistry,
) -> HashMap<String, String> {
    tool_calls
        .iter()
        .map(|tc| {
            let enriched = registry.effective_args(&tc.name, &tc.arguments);
            let args_json = serde_json::to_string(&enriched).unwrap_or_else(|_| "{}".to_string());
            (tc.id.clone(), args_json)
        })
        .collect()
}

// ─── Vision Model Selection ──────────────────────────────────────────────────────

/// Pick the provider for one run-loop iteration.
///
/// When the pending messages contain images, the pre-built `vision_provider`
/// (constructed from the configured vision model's own url/apiKey/model) is
/// used instead of the chat provider — the vision model may live on a
/// different gateway, so swapping only the model name is not enough.
pub fn select_provider_for_messages(
    provider: &OpenAiCompatProvider,
    messages: &[ChatMessage],
    vision_provider: Option<&OpenAiCompatProvider>,
    on_event: &Channel<AgentEvent>,
    notify_user: bool,
) -> Result<OpenAiCompatProvider> {
    if !messages_contain_images(messages) {
        return Ok(provider.clone());
    }

    let Some(vision) = vision_provider else {
        anyhow::bail!("检测到用户上传了图片，但视觉模型未配置。请先在设置中配置视觉模型后重试。");
    };

    let selected = vision.clone();
    if notify_user && selected.model() != provider.model() {
        emit(
            on_event,
            AgentEvent::ModelSwitched {
                from_model: provider.model().to_string(),
                to_model: selected.model().to_string(),
                reason: "检测到用户上传了图片".to_string(),
            },
        );
    }

    Ok(selected)
}

// ─── Cancellation ────────────────────────────────────────────────────────────────

pub fn cancellation_requested(cancel_rx: &watch::Receiver<bool>) -> bool {
    *cancel_rx.borrow()
}

pub async fn wait_for_cancellation(cancel_rx: &mut watch::Receiver<bool>) {
    if cancellation_requested(cancel_rx) {
        return;
    }

    while cancel_rx.changed().await.is_ok() {
        if cancellation_requested(cancel_rx) {
            return;
        }
    }
}

// ─── Utility ─────────────────────────────────────────────────────────────────────

pub fn emit(on_event: &Channel<AgentEvent>, event: AgentEvent) {
    let _ = on_event.send(event);
}

fn build_tool_artifact(tool_name: &str, raw_output: &str) -> ToolArtifactDraft {
    let preview = raw_output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(3)
        .collect::<Vec<_>>()
        .join(" / ");
    let preview = if preview.is_empty() {
        "原始结果为空白或仅包含空行".to_string()
    } else if preview.chars().count() > 160 {
        format!("{}...", preview.chars().take(160).collect::<String>())
    } else {
        preview
    };

    ToolArtifactDraft {
        kind: "tool_raw_output".to_string(),
        title: format!("{tool_name} 原始结果"),
        preview,
        content: raw_output.to_string(),
        char_count: raw_output.chars().count(),
        line_count: raw_output.lines().count().max(1),
    }
}

pub fn is_parallel_readonly_tool_call(
    registry: &ToolRegistry,
    workspace: &std::path::Path,
    tool_call: &RequestedToolCall,
) -> bool {
    registry.is_parallel_readonly(workspace, &tool_call.name, true)
}

pub fn readonly_tool_run_end(
    registry: &ToolRegistry,
    workspace: &std::path::Path,
    tool_calls: &[RequestedToolCall],
    start: usize,
) -> usize {
    tool_calls
        .iter()
        .enumerate()
        .skip(start)
        .find_map(|(index, tool_call)| {
            (!is_parallel_readonly_tool_call(registry, workspace, tool_call)).then_some(index)
        })
        .unwrap_or(tool_calls.len())
}

// ─── Tool Outcome Classification ──────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
#[cfg(test)]
pub enum ToolOutcome {
    Ok,
    RecoverableError { message: String },
    FatalError { message: String },
}

#[cfg(test)]
pub fn classify_tool_result(result: &str) -> ToolOutcome {
    let trimmed = result.trim();
    if trimmed.is_empty() {
        return ToolOutcome::Ok;
    }
    if let Some(msg) = trimmed.strip_prefix(crate::agent::sub_agent::tool::SUB_AGENT_FAILURE_PREFIX)
    {
        return ToolOutcome::FatalError {
            message: msg.to_string(),
        };
    }
    if is_tool_error_message(trimmed) {
        return ToolOutcome::RecoverableError {
            message: trimmed.to_string(),
        };
    }
    ToolOutcome::Ok
}

pub(crate) fn is_tool_error_message(message: &str) -> bool {
    let message = message.trim();
    message.starts_with("错误：")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{classify_tool_result, prepare_tool_result, ToolOutcome};

    #[test]
    fn classifies_tool_error_as_recoverable_without_matching_specific_text() {
        assert_eq!(
            classify_tool_result("错误：任意工具错误都应先交回模型修正"),
            ToolOutcome::RecoverableError {
                message: "错误：任意工具错误都应先交回模型修正".to_string()
            }
        );
    }

    #[test]
    fn keeps_sub_agent_failure_fatal() {
        assert_eq!(
            classify_tool_result("__SUB_AGENT_FAILURE__:子智能体初始化失败"),
            ToolOutcome::FatalError {
                message: "子智能体初始化失败".to_string()
            }
        );
    }

    #[test]
    fn compress_false_truncates_without_requesting_summary() {
        let raw = (1..=300)
            .map(|line| format!("{line}|0123456789"))
            .collect::<Vec<_>>()
            .join("\n");

        let prepared = prepare_tool_result("read_file", &json!({ "compress": false }), &raw);

        assert!(!prepared.needs_summary);
        assert_eq!(prepared.result_mode, "truncated");
        assert!(prepared.display_content.contains("结果已截断"));
        assert!(prepared.display_content.contains("截断发生在原始结果第"));
        assert!(prepared
            .display_content
            .contains("该行标注的源码/匹配行号为"));
        assert_eq!(
            prepared
                .display_content
                .split("\n\n[")
                .next()
                .unwrap_or_default()
                .chars()
                .count(),
            2_000
        );
        assert_eq!(prepared.raw_output, raw);
    }

    #[test]
    fn compress_true_summarizes_only_above_five_thousand_characters() {
        let medium = "x".repeat(5_000);
        let large = "x".repeat(5_001);

        let medium_prepared = prepare_tool_result("grep", &json!({ "compress": true }), &medium);
        let large_prepared = prepare_tool_result("grep", &json!({ "compress": true }), &large);

        assert!(!medium_prepared.needs_summary);
        assert_eq!(medium_prepared.result_mode, "truncated");
        assert!(large_prepared.needs_summary);
        assert_eq!(large_prepared.result_mode, "pending_summary");
    }

    #[test]
    fn compress_false_never_summarizes_large_results() {
        let raw = "x".repeat(6_000);

        let prepared = prepare_tool_result("grep", &json!({ "compress": false }), &raw);

        assert!(!prepared.needs_summary);
        assert_eq!(prepared.result_mode, "truncated");
    }
}
