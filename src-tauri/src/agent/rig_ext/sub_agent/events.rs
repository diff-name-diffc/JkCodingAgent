//! 子智能体事件与轨迹缓冲（rig 形态）。
//!
//! 迁移自旧 `agent/sub_agent/runtime/events.rs`：事件形状、轨迹容量治理
//! （G1-20：超限丢最旧 + `traceTruncated` 标记）逐条保留；用量聚合改为直接
//! 消费 rig `Usage`（不再经 `LlmUsage` 中转）。

use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 轨迹事件缓冲容量上限：长任务中事件可积累数千条，超限后丢弃最旧事件并以
/// `traceTruncated` 标记累计丢弃数，保证缓冲有界且丢弃行为对消费方可见。
pub(crate) const SUB_AGENT_TRACE_EVENT_LIMIT: usize = 500;
const TRACE_TRUNCATED_EVENT: &str = "traceTruncated";

pub(crate) fn record_trace_event(
    trace: &Arc<Mutex<Vec<Value>>>,
    event: Value,
    timestamp_ms: i64,
) {
    let mut event = event;
    if let Some(object) = event.as_object_mut() {
        object.insert("timestampMs".to_string(), Value::from(timestamp_ms));
    }
    let mut events = trace.lock();
    let is_delta = event.get("event").and_then(Value::as_str) == Some("llmDelta");
    if is_delta {
        let incoming = event
            .get("data")
            .and_then(|data| data.get("delta"))
            .and_then(Value::as_str);
        if let (Some(delta), Some(last)) = (incoming, events.last_mut()) {
            if last.get("event").and_then(Value::as_str) == Some("llmDelta") {
                let existing = last
                    .get_mut("data")
                    .and_then(|data| data.get_mut("delta"))
                    .and_then(|value| value.as_str())
                    .map(str::to_owned);
                if let Some(existing) = existing {
                    let merged = format!("{existing}{delta}");
                    if let Some(slot) = last.get_mut("data").and_then(|data| data.get_mut("delta"))
                    {
                        *slot = Value::String(merged);
                        return;
                    }
                }
            }
        }
    }
    events.push(event);
    // 插入时强制执行容量上限，防止长任务中缓冲无界增长。
    trim_trace_events_to_limit(&mut events, timestamp_ms);
}

/// 追踪事件容量治理（G1-20）：超过上限时丢弃最旧事件，并在缓冲头部维护一个
/// traceTruncated 标记事件记录累计丢弃数量，保证丢弃行为可见（fail-closed）。
/// 标记事件始终占据索引 0，裁剪只作用于其后的普通事件，避免丢失计数。
fn trim_trace_events_to_limit(events: &mut Vec<Value>, timestamp_ms: i64) {
    if events.len() <= SUB_AGENT_TRACE_EVENT_LIMIT {
        return;
    }
    let mut dropped = events.len() - SUB_AGENT_TRACE_EVENT_LIMIT;
    let has_marker = events
        .first()
        .and_then(|event| event.get("event"))
        .and_then(Value::as_str)
        == Some(TRACE_TRUNCATED_EVENT);
    if has_marker {
        // 保留索引 0 的标记事件，丢弃其后最旧的 dropped 条。
        events.drain(1..=dropped);
        if let Some(marker) = events.first_mut() {
            let previous = marker
                .get("data")
                .and_then(|data| data.get("dropped"))
                .and_then(Value::as_u64)
                .unwrap_or(0);
            if let Some(slot) = marker
                .get_mut("data")
                .and_then(|data| data.get_mut("dropped"))
            {
                *slot = Value::from(previous + dropped as u64);
            }
            if let Some(slot) = marker.get_mut("timestampMs") {
                *slot = Value::from(timestamp_ms);
            }
        }
    } else {
        // 首次裁剪：多腾出 1 个槽位放置 traceTruncated 标记事件。
        dropped += 1;
        events.drain(0..dropped);
        events.insert(
            0,
            serde_json::json!({
                "event": TRACE_TRUNCATED_EVENT,
                "timestampMs": timestamp_ms,
                "data": {
                    "dropped": dropped,
                    "note": "追踪事件超出容量上限，最早的若干事件已被丢弃",
                },
            }),
        );
    }
}

/// 子智能体累计用量（前端事件载荷形状，与旧一致）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SubAgentUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

impl SubAgentUsage {
    pub(crate) fn record(&mut self, usage: &rig::completion::Usage) {
        self.prompt_tokens += usage.input_tokens;
        self.completion_tokens += usage.output_tokens;
        self.total_tokens += if usage.total_tokens > 0 {
            usage.total_tokens
        } else {
            usage.input_tokens + usage.output_tokens
        };
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", content = "data")]
pub enum SubAgentEvent {
    Started {
        #[serde(rename = "agentId")]
        agent_id: String,
        #[serde(rename = "agentName")]
        agent_name: String,
        task: String,
        /// 运行实际解析出的模型名（UI-14 遗留）：实时事件流与轨迹回放
        /// 的模型来源；老轨迹缺该字段，前端按可选处理走「未记录」兜底。
        model: String,
    },
    ToolStarted {
        #[serde(rename = "agentId")]
        agent_id: String,
        #[serde(rename = "agentName")]
        agent_name: String,
        #[serde(rename = "toolName")]
        tool_name: String,
        arguments: Value,
    },
    ToolFinished {
        #[serde(rename = "agentId")]
        agent_id: String,
        #[serde(rename = "agentName")]
        agent_name: String,
        #[serde(rename = "toolName")]
        tool_name: String,
        #[serde(rename = "resultPreview")]
        result_preview: String,
    },
    Progress {
        #[serde(rename = "agentId")]
        agent_id: String,
        #[serde(rename = "agentName")]
        agent_name: String,
        message: String,
    },
    #[serde(rename = "llmDelta")]
    LlmDelta {
        #[serde(rename = "agentId")]
        agent_id: String,
        #[serde(rename = "agentName")]
        agent_name: String,
        delta: String,
    },
    #[serde(rename = "UsageUpdated")]
    UsageUpdated {
        #[serde(rename = "agentId")]
        agent_id: String,
        #[serde(rename = "agentName")]
        agent_name: String,
        #[serde(rename = "tokenUsage")]
        token_usage: SubAgentUsage,
        #[serde(rename = "elapsedMs")]
        elapsed_ms: u64,
    },
    Finished {
        #[serde(rename = "agentId")]
        agent_id: String,
        #[serde(rename = "agentName")]
        agent_name: String,
        result: String,
        iterations: u32,
        #[serde(rename = "elapsedMs")]
        elapsed_ms: u64,
        #[serde(rename = "tokenUsage")]
        token_usage: SubAgentUsage,
    },
    Failed {
        #[serde(rename = "agentId")]
        agent_id: String,
        #[serde(rename = "agentName")]
        agent_name: String,
        error: String,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct SubAgentEventPayload {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    #[serde(rename = "toolCallId")]
    pub tool_call_id: String,
    #[serde(rename = "timestampMs")]
    pub timestamp_ms: i64,
    #[serde(flatten)]
    pub event: SubAgentEvent,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delta(value: &str) -> Value {
        serde_json::json!({
            "event": "llmDelta",
            "data": { "agentId": "a", "agentName": "A", "delta": value },
        })
    }

    #[test]
    fn consecutive_llm_deltas_are_merged() {
        let trace = Arc::new(Mutex::new(Vec::new()));
        record_trace_event(&trace, delta("he"), 1);
        record_trace_event(&trace, delta("llo"), 2);
        let events = trace.lock().clone();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["data"]["delta"], Value::from("hello"));
    }

    #[test]
    fn trace_buffer_is_bounded_and_marks_dropped_events() {
        let trace = Arc::new(Mutex::new(Vec::new()));
        for index in 0..(SUB_AGENT_TRACE_EVENT_LIMIT + 10) {
            record_trace_event(
                &trace,
                serde_json::json!({ "event": "toolStarted", "data": { "n": index } }),
                index as i64,
            );
        }
        let events = trace.lock().clone();
        assert_eq!(events.len(), SUB_AGENT_TRACE_EVENT_LIMIT);
        assert_eq!(events[0]["event"], Value::from("traceTruncated"));
        assert_eq!(events[0]["data"]["dropped"], Value::from(11));
    }

    #[test]
    fn usage_accumulates_with_total_fallback() {
        let mut usage = SubAgentUsage::default();
        usage.record(&rig::completion::Usage {
            input_tokens: 10,
            output_tokens: 5,
            total_tokens: 0,
            ..Default::default()
        });
        usage.record(&rig::completion::Usage {
            input_tokens: 1,
            output_tokens: 2,
            total_tokens: 3,
            ..Default::default()
        });
        assert_eq!(usage.prompt_tokens, 11);
        assert_eq!(usage.completion_tokens, 7);
        assert_eq!(usage.total_tokens, 18);
    }
}
