use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use parking_lot::Mutex;
use tauri::ipc::Channel;
use tokio::sync::watch;

use super::super::db::{
    DispatcherDb, DispatcherMessageUsageStats, DispatcherSessionTokenUsageSource,
};
use super::super::llm::{
    attach_turn_tool_images, messages_contain_images, ChatMessage, LlmResponse, LlmUsage,
    OpenAiCompatProvider, ToolDefinition,
};
use super::super::run_loop::AgentEvent;
use super::{emit, wait_for_cancellation};

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

/// RAII 用量暂停守卫：构造时暂停 `UsageTracker` 并发出一次 `RunUsageUpdated`，
/// `Drop` 时无条件恢复并再发一次快照。
///
/// 相比手动 pause/resume 配对：持有该守卫的 future 即使 panic、被取消或被丢弃
/// （tokio::select! 丢弃分支、Tauri 命令中止、run loop 提前返回），守卫也会被
/// drop，`UsageTracker` 不会永久停留在 paused 状态（paused_at 残留会持续错误扣减
/// elapsed_ms 并污染主 Agent 的 token 生成速度统计）。
struct UsagePauseGuard<'a> {
    usage_tracker: &'a mut UsageTracker,
    workspace_id: String,
    on_event: &'a Channel<AgentEvent>,
}

impl<'a> UsagePauseGuard<'a> {
    /// 进入暂停状态：暂停计时并立即发出一次用量快照（前端据此停止 live 计时）。
    fn new(
        usage_tracker: &'a mut UsageTracker,
        workspace_id: &str,
        on_event: &'a Channel<AgentEvent>,
    ) -> Self {
        usage_tracker.pause();
        emit(
            on_event,
            AgentEvent::RunUsageUpdated {
                workspace_id: workspace_id.to_string(),
                stats: usage_tracker.snapshot(),
            },
        );
        Self {
            usage_tracker,
            workspace_id: workspace_id.to_string(),
            on_event,
        }
    }
}

impl Drop for UsagePauseGuard<'_> {
    fn drop(&mut self) {
        self.usage_tracker.resume();
        emit(
            self.on_event,
            AgentEvent::RunUsageUpdated {
                workspace_id: self.workspace_id.clone(),
                stats: self.usage_tracker.snapshot(),
            },
        );
    }
}

/// Runs `execute` with the main agent's usage timer paused, then emits a
/// `RunUsageUpdated` event so the frontend can stop padding live elapsed.
/// Used to wrap `call_sub_agent` execution: the sub-agent's wall-clock time
/// must not dilute the main agent's token-generation-speed denominator.
///
/// 内部以 `UsagePauseGuard`（RAII）实现：panic / 取消路径也必然恢复计时。
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
    let guard = UsagePauseGuard::new(usage_tracker, workspace_id, on_event);
    let result = execute().await;
    drop(guard);
    result
}

// ─── LLM Streaming ──────────────────────────────────────────────────────────────

/// 流式生命周期契约（重要）：
/// 本函数只负责发出 `AssistantStarted` 与增量 delta，**不保证发出任何终止事件**，
/// 终止收口是调用方的强契约：
/// - `Cancelled`：调用方必须持久化部分文本并发出终止消息
///   （见 `RunLoopAgent::handle_cancelled_loop`，普通聊天为 `emit_stop_and_finish`）；
/// - `Err`：错误经 `?` 透传后由 `run_agent_turn` 统一发出 `AgentEvent::Failed` 收口。
///
/// 新增调用方必须自行处理这两条终止路径，否则前端会卡在流式状态且部分文本丢失。
///
/// G9-08：`last_seq` 为本次流式实际发出的最后一个 delta 序号（正文与思考增量
/// 共享同一 message_id 计数器）；无 delta 时为 None。调用方收口时应随
/// `AssistantMessage.last_seq` 下发，供前端做去重 / 完整性校验。
pub enum LlmStreamOutcome {
    Cancelled {
        partial: String,
        last_seq: Option<u64>,
    },
    Response {
        response: LlmResponse,
        last_seq: Option<u64>,
    },
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
    // G9-08：同一 message_id 内的正文/思考增量共享一个从 0 开始的单调计数器，
    // 前端可据此检测漏包/乱序/重复；计数在两个 delta 闭包间原子递增。
    let seq_counter = Arc::new(AtomicU64::new(0));
    let msg_id = stream_msg_id.clone();
    let streamed_text_clone = Arc::clone(&streamed_text);
    let delta_seq = Arc::clone(&seq_counter);
    let on_delta = move |delta: &str| {
        let seq = delta_seq.fetch_add(1, Ordering::Relaxed);
        streamed_text_clone.lock().push_str(delta);
        let _ = on_event.send(AgentEvent::AssistantDelta {
            message_id: msg_id.clone(),
            seq,
            delta: delta.to_string(),
        });
    };

    let thinking_msg_id = stream_msg_id.clone();
    let thinking_seq = Arc::clone(&seq_counter);
    let on_thinking_delta = move |delta: &str, elapsed_ms: u64| {
        let seq = thinking_seq.fetch_add(1, Ordering::Relaxed);
        let _ = on_event.send(AgentEvent::AssistantThinkingDelta {
            message_id: thinking_msg_id.clone(),
            seq,
            delta: delta.to_string(),
            elapsed_ms,
        });
    };

    enum StreamSettlement {
        Cancelled(String),
        Response(LlmResponse),
    }

    // 本轮工具引用的图片（fetch_image / generate_image / edit_image / MCP
    // 结果中的 chat-image:// 引用）附加为当前用户消息的视觉输入：
    // messages_contain_images 基于附加后的列表计算，vision 槽位自动切换。
    let effective_messages = attach_turn_tool_images(messages);

    let mut stream_cancel_rx = cancel_rx;
    let settlement = tokio::select! {
        _ = wait_for_cancellation(&mut stream_cancel_rx) => {
            StreamSettlement::Cancelled(streamed_text.lock().clone())
        }
        response = provider.chat_stream_with_thinking(
            &effective_messages,
            tool_definitions,
            messages_contain_images(&effective_messages),
            on_delta,
            on_thinking_delta,
        ) => StreamSettlement::Response(response?)
    };

    // 流结束（正常完成或取消抢占）后 delta 闭包已随 select 分支 drop，
    // 计数器不再增长；末序号供收口方随 AssistantMessage.last_seq 下发对账。
    let last_seq = seq_counter.load(Ordering::Relaxed).checked_sub(1);

    match settlement {
        StreamSettlement::Cancelled(partial) => {
            Ok(LlmStreamOutcome::Cancelled { partial, last_seq })
        }
        StreamSettlement::Response(response) => {
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

            Ok(LlmStreamOutcome::Response { response, last_seq })
        }
    }
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
