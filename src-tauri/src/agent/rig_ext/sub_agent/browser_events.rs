//! browser-use 的动作事件投影；协议校验与 UI/持久化共用一份事件。

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter};

use super::events::{record_trace_event, SubAgentEvent, SubAgentEventPayload, SubAgentUsage};

#[derive(Clone)]
pub(super) struct BrowserTrace {
    pub events: Arc<Mutex<Vec<Value>>>,
    pub agent_id: String,
    pub agent_name: String,
    pub session_id: String,
    pub tool_call_id: String,
    pub app: Option<AppHandle>,
    pub started: Instant,
    active_actions: Arc<Mutex<BTreeMap<String, String>>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct BrowserUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

impl From<BrowserUsage> for SubAgentUsage {
    fn from(value: BrowserUsage) -> Self {
        Self {
            prompt_tokens: value.prompt_tokens,
            completion_tokens: value.completion_tokens,
            total_tokens: value.total_tokens,
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
enum BrowserEvent {
    #[serde(rename_all = "camelCase")]
    ActionStarted {
        task_id: String,
        tool_name: String,
        arguments: Value,
    },
    #[serde(rename_all = "camelCase")]
    ActionFinished {
        task_id: String,
        tool_name: String,
        result_preview: String,
        error: Option<String>,
    },
    Progress {
        message: Option<String>,
        step: Option<u32>,
        url: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Usage { token_usage: BrowserUsage },
}

impl BrowserTrace {
    #[cfg(test)]
    pub(super) fn for_test() -> Self {
        Self {
            events: Default::default(),
            agent_id: "browser-agent".into(),
            agent_name: "浏览器".into(),
            session_id: "session".into(),
            tool_call_id: "parent".into(),
            app: None,
            started: Instant::now(),
            active_actions: Default::default(),
        }
    }

    pub fn new(request: &super::runner::RigSubAgentRequest<'_>) -> Self {
        Self {
            events: Default::default(),
            agent_id: request.config.agent_id.clone(),
            agent_name: request.config.agent_name.clone(),
            session_id: request.session_id.into(),
            tool_call_id: request.parent_tool_call_id.into(),
            app: request.app_handle.clone(),
            started: Instant::now(),
            active_actions: Default::default(),
        }
    }

    pub fn emit(&self, event: SubAgentEvent) {
        let timestamp_ms = chrono::Utc::now().timestamp_millis();
        // SubAgentEvent 只有 JSON 支持的字段，序列化失败属于契约错误。
        match serde_json::to_value(&event) {
            Ok(value) => record_trace_event(&self.events, value, timestamp_ms),
            Err(error) => eprintln!("浏览器轨迹事件编码失败：{error}"),
        }
        if let Some(app) = &self.app {
            if let Err(error) = app.emit(
                "sub-agent-event",
                SubAgentEventPayload {
                    session_id: self.session_id.clone(),
                    tool_call_id: self.tool_call_id.clone(),
                    timestamp_ms,
                    event,
                },
            ) {
                eprintln!("浏览器轨迹广播失败：{error}");
            }
        }
    }

    pub fn receive(&self, value: Value) {
        let event = match serde_json::from_value::<BrowserEvent>(value) {
            Ok(event) => event,
            Err(error) => {
                self.progress(format!("浏览器动作事件契约错误：{error}"));
                return;
            }
        };
        let agent_id = self.agent_id.clone();
        let agent_name = self.agent_name.clone();
        let event = match event {
            BrowserEvent::ActionStarted {
                task_id,
                tool_name,
                arguments,
            } => {
                if self
                    .active_actions
                    .lock()
                    .insert(task_id.clone(), tool_name.clone())
                    .is_some()
                {
                    return;
                }
                SubAgentEvent::ToolStarted {
                    task_id: Some(task_id),
                    agent_id,
                    agent_name,
                    tool_name,
                    arguments,
                }
            }
            BrowserEvent::ActionFinished {
                task_id,
                tool_name,
                result_preview,
                error,
            } => {
                if self.active_actions.lock().remove(&task_id).is_none() {
                    self.progress("收到未配对的浏览器动作完成事件".into());
                    return;
                }
                SubAgentEvent::ToolFinished {
                    task_id: Some(task_id),
                    agent_id,
                    agent_name,
                    tool_name,
                    result_preview: bounded(&result_preview, 2_000),
                    error: error.map(|error| bounded(&error, 2_000)),
                }
            }
            BrowserEvent::Progress { message, step, url } => SubAgentEvent::Progress {
                agent_id,
                agent_name,
                message: bounded(
                    &message.unwrap_or_else(|| {
                        format!(
                            "第 {} 步：{}",
                            step.unwrap_or_default(),
                            url.unwrap_or_default()
                        )
                    }),
                    2_000,
                ),
            },
            BrowserEvent::Usage { token_usage } => SubAgentEvent::UsageUpdated {
                agent_id,
                agent_name,
                token_usage: token_usage.into(),
                elapsed_ms: self.elapsed_ms(),
            },
        };
        self.emit(event);
    }

    pub fn progress(&self, message: String) {
        self.emit(SubAgentEvent::Progress {
            agent_id: self.agent_id.clone(),
            agent_name: self.agent_name.clone(),
            message,
        });
    }

    /// 进程被强杀时没有 Python 收尾事件；必须配齐已开始的动作。
    pub fn settle_actions(&self, error: &str) -> usize {
        let active = std::mem::take(&mut *self.active_actions.lock());
        let count = active.len();
        for (task_id, tool_name) in active {
            self.emit(SubAgentEvent::ToolFinished {
                task_id: Some(task_id),
                agent_id: self.agent_id.clone(),
                agent_name: self.agent_name.clone(),
                tool_name,
                result_preview: bounded(error, 2_000),
                error: Some(bounded(error, 2_000)),
            });
        }
        count
    }

    pub fn elapsed_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    pub fn json(&self) -> Result<String, String> {
        let events = self.events.lock().clone();
        serde_json::to_string(&events).map_err(|error| format!("错误：浏览器轨迹编码失败：{error}"))
    }
}

pub(super) fn bounded(text: &str, limit: usize) -> String {
    let mut chars = text.chars();
    let prefix: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        format!("{prefix}\n[内容已截断]")
    } else {
        prefix
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn failed_and_interrupted_actions_remain_paired() {
        let trace = BrowserTrace::for_test();
        trace.receive(json!({"kind":"action_started","data":{"taskId":"a","toolName":"click","arguments":{"index":1}}}));
        trace.receive(json!({"kind":"action_finished","data":{"taskId":"a","toolName":"click","resultPreview":"missing","error":"missing"}}));
        trace.receive(json!({"kind":"action_started","data":{"taskId":"b","toolName":"navigate","arguments":{}}}));
        trace.settle_actions("已取消");
        let events = trace.events.lock();
        assert_eq!(events.len(), 4);
        assert_eq!(events[1]["data"]["error"], "missing");
        assert_eq!(events[3]["data"]["taskId"], "b");
        assert_eq!(events[3]["data"]["error"], "已取消");
        assert!(trace.active_actions.lock().is_empty());
    }

    #[test]
    fn camel_case_usage_is_projected_to_existing_event_contract() {
        let trace = BrowserTrace::for_test();
        trace.receive(json!({"kind":"usage","data":{"tokenUsage":{"promptTokens":4,"completionTokens":2,"totalTokens":6}}}));
        assert_eq!(
            trace.events.lock()[0]["data"]["tokenUsage"]["totalTokens"],
            6
        );
    }
}
