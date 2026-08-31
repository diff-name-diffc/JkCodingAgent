use super::*;

pub(in crate::agent::sub_agent) fn record_trace_event(
    trace: &Arc<Mutex<Vec<Value>>>,
    mut event: Value,
    timestamp_ms: i64,
) {
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
    // G1-20：插入时强制执行容量上限，防止长任务中缓冲无界增长。
    trim_trace_events_to_limit(&mut events, timestamp_ms);
}

/// 追踪事件容量治理（G1-20）：超过上限时丢弃最旧事件，并在缓冲头部维护一个
/// traceTruncated 标记事件记录累计丢弃数量，保证丢弃行为可见（fail-closed）。
/// 标记事件始终占据索引 0，裁剪只作用于其后的普通事件，避免丢失计数。
pub(super) fn trim_trace_events_to_limit(events: &mut Vec<Value>, timestamp_ms: i64) {
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SubAgentUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

impl SubAgentUsage {
    pub(super) fn record(&mut self, usage: &LlmUsage) {
        self.prompt_tokens += usage.prompt_tokens;
        self.completion_tokens += usage.completion_tokens;
        self.total_tokens += if usage.total_tokens > 0 {
            usage.total_tokens
        } else {
            usage.prompt_tokens + usage.completion_tokens
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
