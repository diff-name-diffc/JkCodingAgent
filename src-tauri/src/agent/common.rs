use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use parking_lot::Mutex;
use tokio::sync::watch;

use tauri::ipc::Channel;

use super::db::{
    DispatcherDb, DispatcherMessageRecord, DispatcherMessageUsageStats,
    DispatcherSessionTokenUsageSource, ToolArtifactDraft,
};
use super::llm::{
    messages_contain_inline_images, ChatMessage, FunctionCall, LlmResponse, LlmUsage,
    OpenAiCompatProvider, OutboundToolCall, RequestedToolCall, ToolDefinition,
};
use super::runtime::AgentEvent;
use super::tools::ToolRegistry;

// ─── Usage Tracking ────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct UsageTracker {
    pub started_at: Instant,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

impl UsageTracker {
    pub fn new() -> Self {
        Self {
            started_at: Instant::now(),
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
        }
    }

    pub fn record(&mut self, usage: &LlmUsage) -> DispatcherMessageUsageStats {
        self.prompt_tokens += usage.prompt_tokens;
        self.completion_tokens += usage.completion_tokens;
        self.total_tokens += normalized_total_tokens(usage);
        self.snapshot()
    }

    pub fn snapshot(&self) -> DispatcherMessageUsageStats {
        DispatcherMessageUsageStats {
            prompt_tokens: self.prompt_tokens,
            completion_tokens: self.completion_tokens,
            total_tokens: self.total_tokens,
            elapsed_ms: self.started_at.elapsed().as_millis() as u64,
        }
    }
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
    enable_thinking: bool,
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
            messages_contain_inline_images(messages),
            enable_thinking,
            on_delta,
            on_thinking_delta,
        ) => response
    }?;

    if let Some(usage) = response.usage.as_ref() {
        record_usage(db, workspace_id, model, source_kind, usage, usage_tracker, on_event);
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
        if let Err(error) = db.upsert_session_token_usage_async(&wid, &m, source_kind, &u).await {
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

// ─── Tool Result Preparation (Hybrid: Forced 1000-char + Intent-Driven Summary) ─

/// If a tool result exceeds this threshold in characters, compression is forced
/// regardless of the model's `compress` parameter.
pub const FORCED_COMPRESS_THRESHOLD: usize = 1_000;

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

    let model_compress = args.get("compress").and_then(|v| v.as_bool()).unwrap_or(false);
    let compress_intent = args
        .get("compress_intent")
        .and_then(|v| v.as_str())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let char_count = trimmed.chars().count();
    let exceeds_threshold = char_count > FORCED_COMPRESS_THRESHOLD;

    let needs_summary = model_compress || exceeds_threshold;

    if needs_summary {
        PreparedToolResult {
            display_content: String::new(),
            context_payload: String::new(),
            result_mode: "pending_summary",
            raw_output: trimmed.to_string(),
            needs_summary: true,
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

// ─── Tool Result Persistence ─────────────────────────────────────────────────────

pub async fn persist_tool_result_raw(
    db: &DispatcherDb,
    workspace_id: &str,
    on_event: &Channel<AgentEvent>,
    tool_call: &RequestedToolCall,
    result: &str,
) -> Result<DispatcherMessageRecord> {
    let tool_message = db
        .add_visible_tool_result_async(
            workspace_id,
            result,
            result,
            Some(&tool_call.id),
            Some(&tool_call.name),
            Some("raw"),
            &[],
        )
        .await?;

    emit(
        on_event,
        AgentEvent::ToolFinished {
            tool_call_id: Some(tool_call.id.clone()),
            name: tool_call.name.clone(),
            display_text: tool_message.content.clone(),
            result_mode: "raw".to_string(),
            detail_refs: Vec::new(),
        },
    );

    Ok(tool_message)
}

/// Shared summary-aware tool result persistence used by both dispatcher and plain chat.
/// Calls the summary model if the result exceeds `FORCED_COMPRESS_THRESHOLD` or the model
/// requested compression via `compress=true`. On summary failure, falls back to
/// rule-based structured extraction (zero LLM call), then to hard truncation.
#[allow(clippy::too_many_arguments)]
pub async fn persist_tool_result_with_compression<FUsage>(
    db: &DispatcherDb,
    workspace_id: &str,
    on_event: &Channel<AgentEvent>,
    tool_call: &RequestedToolCall,
    result: &str,
    summary_provider: &OpenAiCompatProvider,
    summary_model: &str,
    on_usage: FUsage,
) -> Result<DispatcherMessageRecord>
where
    FUsage: FnMut(&LlmUsage) + Send,
{
    use crate::agent::summary::{extract_structured_summary, summarize_tool_result};

    let prepared = prepare_tool_result(&tool_call.name, &tool_call.arguments, result);

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
        if thinking_content.is_empty() { None } else { Some(thinking_content) },
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
            let args_json =
                serde_json::to_string(&enriched).unwrap_or_else(|_| "{}".to_string());
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
            let args_json =
                serde_json::to_string(&enriched).unwrap_or_else(|_| "{}".to_string());
            (tc.id.clone(), args_json)
        })
        .collect()
}

// ─── Vision Model Selection ──────────────────────────────────────────────────────

pub fn select_provider_for_messages(
    provider: &OpenAiCompatProvider,
    messages: &[ChatMessage],
    vision_model: &str,
    on_event: &Channel<AgentEvent>,
    notify_user: bool,
) -> Result<OpenAiCompatProvider> {
    if !messages_contain_inline_images(messages) {
        return Ok(provider.clone());
    }

    if vision_model.trim().is_empty() {
        anyhow::bail!(
            "检测到用户上传了图片，但视觉模型未配置。请先在设置中配置视觉模型后重试。"
        );
    }

    let selected = provider.with_model(vision_model.trim());
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

pub fn is_parallel_readonly_tool_call(tool_call: &RequestedToolCall) -> bool {
    matches!(
        tool_call.name.as_str(),
        "read_file" | "list_dir" | "glob" | "grep"
    )
}

pub fn readonly_tool_run_end(tool_calls: &[RequestedToolCall], start: usize) -> usize {
    tool_calls
        .iter()
        .enumerate()
        .skip(start)
        .find_map(|(index, tool_call)| {
            (!is_parallel_readonly_tool_call(tool_call)).then_some(index)
        })
        .unwrap_or(tool_calls.len())
}
