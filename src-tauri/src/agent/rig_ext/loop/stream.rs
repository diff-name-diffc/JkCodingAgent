//! rig 流消费：逐条把 `StreamedAssistantContent` 翻译成 `AgentEvent` 增量，
//! 流终态聚合 choice 拆分正文/思考/工具调用。

use std::collections::HashSet;
use std::time::Instant;

use anyhow::Result;
use futures::StreamExt;
use rig::message::{AssistantContent, ToolCall};
use rig::streaming::{StreamedAssistantContent, StreamingCompletionResponse};
use tauri::ipc::Channel;
use tokio::sync::watch;

use crate::agent::common::{emit, wait_for_cancellation};
use crate::agent::rig_ext::events::AgentEvent;

pub(super) struct StreamConsumption {
    /// 已流出的正文（取消收口用）。
    pub(super) partial_text: String,
    /// 思考耗时（首个思考增量起算，对齐旧实现）。
    pub(super) thinking_elapsed_ms: u64,
    /// 本次流式实际发出的最后一个 delta 序号；无 delta 为 None。
    pub(super) last_seq: Option<u64>,
    pub(super) cancelled: bool,
}

/// 流式进度的共享视图：`drive` 的取消分支赢掉与 `consume_stream` 自带取消
/// 检查的竞态时，流式 future 被整体 drop，已累积的部分正文/序号经此句柄
/// 带出给取消收口，不再随 future 一起丢弃。
#[derive(Default)]
pub(super) struct StreamProgress {
    /// 已流出的正文（随增量就地追加）。
    pub(super) partial_text: String,
    /// 已发出的 delta 计数；最后一个已发序号 = `seq.checked_sub(1)`。
    pub(super) seq: u64,
}

/// 消费一条 rig 流：逐条把增量翻译成 AgentEvent；取消时中止流并保留部分正文。
/// 流项错误 fail-closed（对齐旧实现：任何 SSE 协议错误 = 整轮请求失败）。
/// 增量就地写入 `progress`：即使本 future 被 `drive` 的取消分支中途 drop，
/// 已流出的部分正文仍可由调用方取回落库。
pub(super) async fn consume_stream(
    stream: &mut StreamingCompletionResponse,
    on_event: &Channel<AgentEvent>,
    cancel_rx: watch::Receiver<bool>,
    progress: &mut StreamProgress,
) -> Result<StreamConsumption> {
    let message_id = uuid::Uuid::new_v4().to_string();
    emit(
        on_event,
        AgentEvent::AssistantStarted {
            message_id: message_id.clone(),
        },
    );

    let mut thinking_started_at: Option<Instant> = None;
    let mut thinking_elapsed_ms = 0_u64;
    // 已见增量的 reasoning 关联键：完整 Reasoning 块是对其增量的替代而非追加，
    // 避免思考内容重复下发。
    let mut seen_reasoning_ids: HashSet<String> = HashSet::new();
    let mut cancelled = false;
    let mut cancel_rx = cancel_rx;

    loop {
        let item = tokio::select! {
            _ = wait_for_cancellation(&mut cancel_rx) => {
                cancelled = true;
                stream.cancel();
                break;
            }
            item = stream.next() => item,
        };

        let content = match item {
            None => break,
            Some(Err(error)) => {
                return Err(anyhow::anyhow!("LLM 流式响应分片失败：{error}"));
            }
            Some(Ok(content)) => content,
        };

        match content {
            StreamedAssistantContent::Text(text) => {
                let this_seq = progress.seq;
                progress.seq += 1;
                progress.partial_text.push_str(&text.text);
                emit(
                    on_event,
                    AgentEvent::AssistantDelta {
                        message_id: message_id.clone(),
                        seq: this_seq,
                        delta: text.text,
                    },
                );
            }
            StreamedAssistantContent::ReasoningDelta { id, reasoning, .. } => {
                if !reasoning.is_empty() {
                    seen_reasoning_ids.insert(id);
                    let started_at = thinking_started_at.get_or_insert_with(Instant::now);
                    thinking_elapsed_ms = started_at.elapsed().as_millis() as u64;
                    let this_seq = progress.seq;
                    progress.seq += 1;
                    emit(
                        on_event,
                        AgentEvent::AssistantThinkingDelta {
                            message_id: message_id.clone(),
                            seq: this_seq,
                            delta: reasoning,
                            elapsed_ms: thinking_elapsed_ms,
                        },
                    );
                }
            }
            StreamedAssistantContent::Reasoning { reasoning, id } => {
                // 完整块只在其增量未出现过时补发（替代语义见 rig 文档）。
                if !seen_reasoning_ids.contains(&id) {
                    let display = reasoning.display_text();
                    if !display.is_empty() {
                        let started_at = thinking_started_at.get_or_insert_with(Instant::now);
                        thinking_elapsed_ms = started_at.elapsed().as_millis() as u64;
                        let this_seq = progress.seq;
                        progress.seq += 1;
                        emit(
                            on_event,
                            AgentEvent::AssistantThinkingDelta {
                                message_id: message_id.clone(),
                                seq: this_seq,
                                delta: display,
                                elapsed_ms: thinking_elapsed_ms,
                            },
                        );
                    }
                }
            }
            // 工具调用增量前端不消费（收齐后以 ToolPlanned/Started/Finished 配对下发）；
            // 聚合结果从流终态的 choice 读取，此处不重复收集。
            StreamedAssistantContent::ToolCall { .. }
            | StreamedAssistantContent::ToolCallDelta { .. }
            | StreamedAssistantContent::Final(_)
            | StreamedAssistantContent::Unknown(_) => {}
        }
    }

    Ok(StreamConsumption {
        partial_text: std::mem::take(&mut progress.partial_text),
        thinking_elapsed_ms,
        last_seq: progress.seq.checked_sub(1),
        cancelled,
    })
}

/// 聚合 choice → (正文, 思考, 工具调用)。`<think>` 标签正文拆入思考
/// （对齐旧 `split_tagged_thinking`：DeepSeek 等把思考混在 content 里的方言）。
pub(super) fn split_choice(choice: &[AssistantContent]) -> (String, String, Vec<ToolCall>) {
    let mut text = String::new();
    let mut thinking = String::new();
    let mut tool_calls = Vec::new();
    for content in choice {
        match content {
            AssistantContent::Text(t) => text.push_str(&t.text),
            AssistantContent::Reasoning(r) => {
                let display = r.display_text();
                if !display.is_empty() {
                    if !thinking.is_empty() {
                        thinking.push('\n');
                    }
                    thinking.push_str(&display);
                }
            }
            AssistantContent::ToolCall(call) => tool_calls.push(call.clone()),
            AssistantContent::Image(_) => {}
        }
    }

    let (visible, tagged_thinking) = split_tagged_thinking(&text);
    if !tagged_thinking.trim().is_empty() {
        if !thinking.trim().is_empty() {
            thinking.push_str("\n\n");
        }
        thinking.push_str(tagged_thinking.trim());
    }
    (visible.trim().to_string(), thinking, tool_calls)
}

/// 与旧客户端层的 `split_tagged_thinking` 同一实现（私有不可复用，
/// Phase 5 归一）：把 `<think>…</think>` 块从正文拆到思考链。
fn split_tagged_thinking(content: &str) -> (String, String) {
    let lower = content.to_ascii_lowercase();
    let mut visible = String::new();
    let mut thinking_blocks = Vec::new();
    let mut cursor = 0usize;

    while let Some(start_rel) = lower[cursor..].find("<think>") {
        let start = cursor + start_rel;
        let body_start = start + "<think>".len();
        let Some(end_rel) = lower[body_start..].find("</think>") else {
            break;
        };
        let end = body_start + end_rel;
        let tag_end = end + "</think>".len();

        visible.push_str(&content[cursor..start]);
        let thinking = content[body_start..end].trim();
        if !thinking.is_empty() {
            thinking_blocks.push(thinking.to_string());
        }
        cursor = tag_end;
    }

    visible.push_str(&content[cursor..]);
    (visible.trim().to_string(), thinking_blocks.join("\n\n"))
}
