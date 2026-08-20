use serde_json::Value;
use tauri::ipc::Channel;
use tokio::task::JoinHandle;

use crate::agent::common::{is_tool_error_message, UsageTracker};
use crate::agent::db::{
    DispatcherDb, DispatcherSessionTokenUsageSource, TOOL_RETRY_CONTEXT_PREFIX,
};
use crate::agent::llm::{LlmResponse, LlmUsage, RequestedToolCall};
use crate::agent::run_loop::AgentEvent;
use crate::shared::truncate_for_display;

// ─── Persistent Warning Log ───────────────────────────────────────────────────

/// 警告日志文件上限：超过后滚动为 .old（仅保留最近一份），防止无界增长。
const MAX_WARNING_LOG_BYTES: u64 = 5 * 1024 * 1024;

/// 持久化警告日志：打包后的 Tauri 应用 stderr 通常不落任何文件，
/// 关键链路的失败（用量持久化、学习回路统计、事件推送等）必须同时写盘，
/// 否则静默失效时完全不可诊断。写盘失败时退化为 eprintln。
pub(crate) fn log_warning(message: &str) {
    eprintln!("{message}");
    let Some(log_path) = dirs::home_dir().map(|home| {
        home.join(".jkcodingagent")
            .join("logs")
            .join("orchestrator.log")
    }) else {
        return;
    };
    let entry = format!("{} {message}\n", chrono::Utc::now().to_rfc3339());
    let write = move || {
        if let Some(parent) = log_path.parent() {
            if std::fs::create_dir_all(parent).is_err() {
                return;
            }
        }
        // 简易滚动：超限后重命名为 .old 再重建，避免日志无限增长。
        if let Ok(meta) = std::fs::metadata(&log_path) {
            if meta.len() > MAX_WARNING_LOG_BYTES {
                let mut old_path = log_path.clone();
                old_path.set_extension("log.old");
                let _ = std::fs::rename(&log_path, old_path);
            }
        }
        let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
        else {
            return;
        };
        use std::io::Write;
        let _ = file.write_all(entry.as_bytes());
    };
    // 无 tokio runtime 时（如单元测试）退化为同步写入。
    if tokio::runtime::Handle::try_current().is_ok() {
        tokio::task::spawn_blocking(write);
    } else {
        write();
    }
}

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

    // 「错误：」前缀是工具/运行错误识别契约（is_tool_error_message 与上层分类依赖它）。
    format!("错误：LLM 返回了空响应且没有工具调用，无法继续执行。\nLLM 接口响应内容：\n{response_detail}")
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
    // exec 刻意不走自动重试通道（fail-closed）：命令执行受 AI 审查门禁管控，
    // 自动重试会绕过重新审查。错误结果仍以普通工具消息喂回模型，模型若发起
    // 新的 exec 调用会重新过门禁。除非把可重试性判定与审批状态关联，
    // 否则不要放开此处。
    if tool_name == "exec" {
        return false;
    }
    is_tool_error_message(trimmed)
}

// ─── Token Usage Recording ────────────────────────────────────────────────────

/// 派生任务持久化会话用量，返回任务句柄。
///
/// 调用方负责 await 句柄（见 `record_run_token_usage` 与工具执行收口处）：
/// 应用退出/DB 短暂故障时用量不再静默丢失；失败时先重试一次，仍失败则记录
/// 持久化警告日志。
pub(crate) fn record_session_token_usage(
    db: &DispatcherDb,
    workspace_id: &str,
    model: &str,
    source_kind: DispatcherSessionTokenUsageSource,
    usage: &LlmUsage,
) -> JoinHandle<()> {
    let db = db.clone();
    let wid = workspace_id.to_string();
    let m = model.to_string();
    let u = usage.clone();
    tokio::spawn(async move {
        if db
            .upsert_session_token_usage_async(&wid, &m, source_kind, &u)
            .await
            .is_err()
        {
            // 瞬时失败（如其他写者短暂持锁）重试一次；upsert 幂等，重试安全。
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            if let Err(error) = db
                .upsert_session_token_usage_async(&wid, &m, source_kind, &u)
                .await
            {
                log_warning(&format!(
                    "failed to persist dispatcher session token usage for workspace {wid} and model {m}: {error}"
                ));
            }
        }
    })
}

/// 记录运行用量：会话用量持久化（句柄由调用方收集并 await）+ 运行统计累加并发事件。
///
/// 口径联动（审查项 G8-03）：持久化句柄在工具执行收口处统一 await，
/// 前端运行统计与 DB 会话用量来自同一批请求；持久化失败会留下持久化警告，
/// 两处状态出现偏差时可诊断。
pub(crate) fn record_run_token_usage(
    db: &DispatcherDb,
    workspace_id: &str,
    model: &str,
    source_kind: DispatcherSessionTokenUsageSource,
    usage: &LlmUsage,
    tracker: &mut UsageTracker,
    on_event: &Channel<AgentEvent>,
    pending_persists: &mut Vec<JoinHandle<()>>,
) {
    pending_persists.push(record_session_token_usage(
        db,
        workspace_id,
        model,
        source_kind,
        usage,
    ));
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
        crate::agent::config::DEFAULT_SUMMARY_MODEL.to_string()
    } else {
        trimmed.to_string()
    }
}

// ─── Event Helpers ────────────────────────────────────────────────────────────

pub(crate) fn emit(on_event: &Channel<AgentEvent>, event: AgentEvent) {
    // 前端断开/回执失败时记录持久化警告，避免事件丢失无任何痕迹。
    if let Err(error) = on_event.send(event) {
        log_warning(&format!(
            "agent event 推送前端失败（前端可能已断开）：{error}"
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::{empty_llm_response_error, is_retryable_tool_error};
    use crate::agent::llm::LlmResponse;

    fn response_with_raw(raw_response: &str) -> LlmResponse {
        LlmResponse {
            status_code: 200,
            content: String::new(),
            thinking_content: String::new(),
            thinking_elapsed_ms: 0,
            tool_calls: Vec::new(),
            raw_response: raw_response.to_string(),
            usage: None,
            finish_reason: None,
        }
    }

    #[test]
    fn empty_llm_response_error_carries_error_prefix() {
        let message = empty_llm_response_error(&response_with_raw(""));
        assert!(
            message.starts_with("错误："),
            "错误文案必须以「错误：」开头：{message}"
        );
        assert!(message.contains("<空>"));
    }

    #[test]
    fn retryable_error_classification() {
        assert!(is_retryable_tool_error("read_file", "错误：读取失败"));
        assert!(!is_retryable_tool_error("read_file", "正常输出"));
        assert!(!is_retryable_tool_error("read_file", "   "));
        // exec 永远不可自动重试（审查门禁约束）。
        assert!(!is_retryable_tool_error("exec", "错误：命令不存在"));
    }
}
