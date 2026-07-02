use serde_json::Value;
use tauri::ipc::Channel;

use super::super::common::{is_tool_error_message, UsageTracker};
use super::super::db::{
    DispatcherDb, DispatcherSessionTokenUsageSource, TOOL_RETRY_CONTEXT_PREFIX,
};
use super::super::llm::{LlmResponse, LlmUsage, RequestedToolCall};
use super::types::AgentEvent;
use crate::shared::truncate_for_display;

pub(crate) fn extract_message_content(arguments: &Value) -> Option<String> {
    arguments
        .get("content")
        .and_then(Value::as_str)
        .map(str::to_string)
}

// ─── Tool Error Classification ────────────────────────────────────────────────

pub(crate) fn empty_llm_response_error(response: &LlmResponse) -> String {
    let raw_response = response.raw_response.trim();
    let response_detail = if raw_response.is_empty() {
        "<空>".to_string()
    } else {
        truncate_for_display(raw_response, 4_000, "\n...[LLM 接口响应内容已截断]")
    };

    format!("LLM 返回了空响应且没有工具调用，无法继续执行。\nLLM 接口响应内容：\n{response_detail}")
}

pub(crate) fn build_tool_retry_context(tool_call: &RequestedToolCall, error: &str) -> String {
    let arguments =
        serde_json::to_string_pretty(&tool_call.arguments).unwrap_or_else(|_| "{}".to_string());
    format!(
        "{TOOL_RETRY_CONTEXT_PREFIX}\n\
工具：{}\n\
工具调用 ID：{}\n\
错误详情：{}\n\n\
上次参数：\n{}\n\n\
要求：不要直接把该错误回复给用户。请根据工具 schema 和错误详情修正参数后重试同一个工具；重试成功后，系统会覆盖本次失败工具调用记录。",
        tool_call.name,
        tool_call.id,
        error.trim(),
        truncate_for_display(&arguments, 4_000, "\n...")
    )
}

pub(crate) fn is_retryable_tool_error(tool_name: &str, result: &str) -> bool {
    let trimmed = result.trim();
    if trimmed.is_empty() {
        return false;
    }
    if tool_name == "exec" {
        return false;
    }
    is_tool_error_message(trimmed)
}

// ─── Text Formatting ──────────────────────────────────────────────────────────

pub(crate) fn compact_multiline(content: &str, max_chars: usize) -> String {
    let normalized = content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" / ");
    truncate_for_display(&normalized, max_chars, "...")
}

pub(crate) fn should_include_latest_user_goal(
    latest_user_goal: &str,
    task_description: &str,
) -> bool {
    let normalized_task = compact_multiline(task_description.trim(), 320);
    !normalized_task.is_empty()
        && latest_user_goal != normalized_task
        && !normalized_task.contains(latest_user_goal)
}

pub(crate) fn summarize_dispatch_description(task_description: &str) -> String {
    let normalized = compact_multiline(task_description.trim(), 180);
    if normalized.is_empty() {
        "未命名子任务".to_string()
    } else {
        normalized
    }
}

pub(crate) fn build_stopped_dispatch_reply(partial: &str) -> String {
    let trimmed = partial.trim();
    if trimmed.is_empty() {
        "⏹️ 本轮调度已停止。当前会话上下文与已完成内容均已保留，可稍后继续。".to_string()
    } else {
        format!(
            "{}\n\n[本轮已手动停止。当前会话上下文与以上输出均已保留，可稍后继续。]",
            trimmed
        )
    }
}

// ─── Token Usage Recording ────────────────────────────────────────────────────

pub(crate) fn record_session_token_usage(
    db: &DispatcherDb,
    workspace_id: &str,
    model: &str,
    source_kind: DispatcherSessionTokenUsageSource,
    usage: &LlmUsage,
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
                "failed to persist dispatcher session token usage for workspace {} and model {}: {}",
                wid, m, error
            );
        }
    });
}

pub(crate) fn record_run_token_usage(
    db: &DispatcherDb,
    workspace_id: &str,
    model: &str,
    source_kind: DispatcherSessionTokenUsageSource,
    usage: &LlmUsage,
    tracker: &mut UsageTracker,
    on_event: &Channel<AgentEvent>,
) {
    record_session_token_usage(db, workspace_id, model, source_kind, usage);
    let stats = tracker.record(usage);
    emit(
        on_event,
        AgentEvent::RunUsageUpdated {
            workspace_id: workspace_id.to_string(),
            stats,
        },
    );
}

pub(crate) fn normalize_summary_model(model: &str) -> String {
    let trimmed = model.trim();
    if trimmed.is_empty() {
        super::super::config::DEFAULT_SUMMARY_MODEL.to_string()
    } else {
        trimmed.to_string()
    }
}

// ─── Event Helpers ────────────────────────────────────────────────────────────

pub(crate) fn emit(on_event: &Channel<AgentEvent>, event: AgentEvent) {
    let _ = on_event.send(event);
}

pub(crate) async fn collect_recent_exploration_entries_from_db(
    db: &DispatcherDb,
    workspace_id: &str,
) -> std::result::Result<String, String> {
    const MAX_ENTRIES: usize = 3;
    const MAX_TOTAL_CHARS: usize = 900;

    let rows = db
        .list_recent_exploration_content_async(workspace_id, MAX_ENTRIES)
        .await
        .map_err(|error| error.to_string())?;

    let mut entries = Vec::new();
    let mut total_chars = 0usize;

    // rows come in DESC order; collect then reverse for chronological order
    for (role, tool_name, content) in rows {
        let content = content.trim();
        if content.is_empty() {
            continue;
        }

        let label = match role.as_str() {
            "tool" => tool_name
                .map(|name| format!("工具 {}", name))
                .unwrap_or_else(|| "工具".to_string()),
            "assistant" => "调度结论".to_string(),
            _ => continue,
        };
        let compact = compact_multiline(content, 220);
        if compact.is_empty() {
            continue;
        }

        let candidate = format!("- {}：{}", label, compact);
        let candidate_len = candidate.chars().count();
        if total_chars + candidate_len > MAX_TOTAL_CHARS && !entries.is_empty() {
            break;
        }

        entries.push(candidate);
        total_chars += candidate_len;
        if entries.len() >= MAX_ENTRIES {
            break;
        }
    }

    if entries.is_empty() {
        Ok(String::new())
    } else {
        entries.reverse();
        Ok(entries.join("\n"))
    }
}
