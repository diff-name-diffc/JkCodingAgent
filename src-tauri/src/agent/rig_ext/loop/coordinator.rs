//! 请求边界的确定性交付和观察确认；此处是聊天消息的唯一写入方。
use super::{
    scheduler::TaskScheduler,
    surface::{RigToolSurface, ToolExecutionPolicy},
};
use crate::agent::{
    common::{emit, UsageTracker},
    db::DispatcherMessageRecord,
    rig_ext::events::AgentEvent,
};
use anyhow::Result;
use rig::{
    completion::Message,
    message::{ToolCall, ToolResult, ToolResultContent, UserContent},
};
use tauri::ipc::Channel;

pub(crate) struct Coordinator {
    pub tasks: TaskScheduler,
    snapshot: Vec<i64>,
    pub host: super::host::LoopHost,
}

/// 派发后等待结果就绪的窗口：窗口内就绪的调用随本批直接交付（发 ToolFinished），
/// 未就绪的发 ToolAccepted 占位，正文留给后续 `deliver` 轮次装配。
///
/// 取值权衡：200ms 足以容纳本轮内的快工具，又不至于让每批派发长时间阻塞；
/// 调大让更多调用直接交付（占位事件更少），代价是每批派发变慢。
const INLINE_DISPATCH_WINDOW: std::time::Duration = std::time::Duration::from_millis(200);

impl Coordinator {
    pub fn new(tasks: TaskScheduler) -> Self {
        Self {
            tasks,
            snapshot: Vec::new(),
            host: super::host::LoopHost::Conversation,
        }
    }
    pub fn unsettled(&self) -> bool {
        self.tasks.pending() || !self.tasks.ready.is_empty()
    }

    pub async fn deliver(
        &mut self,
        messages: &mut Vec<Message>,
        ids: &mut Vec<Option<String>>,
        events: &Channel<AgentEvent>,
        usage: &mut UsageTracker,
    ) -> Result<()> {
        self.tasks.collect_ready().await?;
        self.snapshot.clear();
        let mut chars = 0;
        let mut delivered = Vec::new();
        for completion in self.tasks.ready.values_mut() {
            let in_history = completion
                .delivery_message_id
                .as_ref()
                .is_some_and(|id| ids.iter().any(|entry| entry.as_ref() == Some(id)));
            if completion.delivery_message_id.is_none()
                || (self.tasks.recovered.contains(&completion.event_id) && !in_history)
            {
                let size = completion.context_payload.chars().count();
                if chars > 0 && chars + size > 32_000 {
                    break;
                }
                let record = self
                    .host
                    .deliver(
                        &self.tasks.db,
                        &self.tasks.workspace,
                        &completion.scope_id,
                        completion,
                    )
                    .await?;
                chars += size;
                let payload = if record.role == "tool" {
                    serde_json::json!({"kind":"tool_completion","task_id":completion.tool_run_id,
                        "tool_call_id":completion.tool_call_id, "status":completion.status,
                        "context_payload":completion.context_payload})
                    .to_string()
                } else {
                    record.context_payload.clone().unwrap_or_default()
                };
                messages.push(Message::user(payload));
                ids.push(Some(record.id.clone()));
                completion.delivery_message_id = Some(record.id.clone());
                if let Some(call) = self.tasks.calls.get(&completion.tool_run_id) {
                    finish(events, call, &record);
                    delivered.push(completion.tool_run_id.clone());
                }
                if let Some(json) = completion.usage_json.take() {
                    usage.record(&serde_json::from_str(&json)?);
                }
            }
            self.snapshot.push(completion.event_id);
        }
        for task in delivered {
            self.tasks.delivered(&task);
        }
        Ok(())
    }

    pub async fn observed(&mut self, step: u64) -> Result<()> {
        let ids = self.snapshot.clone();
        if ids.is_empty() {
            return Ok(());
        }
        let mut scopes = std::collections::BTreeMap::<(String, String), Vec<i64>>::new();
        for event in ids {
            let completion = self
                .tasks
                .ready
                .get(&event)
                .ok_or_else(|| anyhow::anyhow!("观察快照事件缺失：{event}"))?;
            scopes
                .entry((completion.agent_run_id.clone(), completion.scope_id.clone()))
                .or_default()
                .push(event);
        }
        for ((run, scope), ids) in scopes {
            self.host
                .observe(self.tasks.db.clone(), run, scope, i64::try_from(step)?, ids)
                .await?;
        }
        for id in self.snapshot.drain(..) {
            self.tasks.ready.remove(&id);
            self.tasks.recovered.remove(&id);
        }
        Ok(())
    }

    async fn reject_batch(
        &self,
        calls: &[ToolCall],
        text: &str,
        events: &Channel<AgentEvent>,
    ) -> Result<(Vec<UserContent>, Option<String>)> {
        let mut results = Vec::new();
        let mut last = None;
        for call in calls {
            let record = self
                .host
                .reject(&self.tasks.db, &self.tasks.workspace, call, text)
                .await?;
            finish(events, call, &record);
            last = Some(record.id);
            results.push(tool_reply(call, text));
        }
        Ok((results, last))
    }

    pub async fn dispatch<P: ToolExecutionPolicy + Clone + 'static>(
        &mut self,
        calls: &[ToolCall],
        surface: &RigToolSurface,
        policy: &P,
        round: u64,
        anchor: &str,
        events: &Channel<AgentEvent>,
        usage: &mut UsageTracker,
    ) -> Result<(Vec<UserContent>, Option<String>)> {
        if let Some(call) = calls
            .iter()
            .find(|call| surface.find(&call.function.name).is_none())
        {
            let text = format!("错误：未注册的工具：{}；本批未执行", call.function.name);
            return self.reject_batch(calls, &text, events).await;
        }
        let tools = calls
            .iter()
            .map(|call| {
                surface
                    .find(&call.function.name)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("未注册工具：{}", call.function.name))
            })
            .collect::<Result<Vec<_>>>()?;
        let policies = calls
            .iter()
            .map(|call| surface.policy_for(&call.function.name))
            .collect::<Vec<_>>();
        let deadline = tokio::time::Instant::now() + INLINE_DISPATCH_WINDOW;
        let ids = match self
            .tasks
            .enqueue(calls, &tools, policy, &policies, round, anchor)
            .await
        {
            Ok(ids) => ids,
            Err(error) if error.is::<super::budgets::AdmissionError>() => {
                return self
                    .reject_batch(calls, &format!("错误：{error}；本批未执行"), events)
                    .await;
            }
            Err(error) => {
                self.reject_batch(
                    calls,
                    &format!("错误：工具批次登记失败，本批未执行：{error}"),
                    events,
                )
                .await?;
                return Err(error);
            }
        };
        // 即使窗口内取消/基础设施失败，也先配齐已登记批次的唯一应答。
        let window_error = self.tasks.window(deadline).await.err();
        let mut contents = Vec::new();
        let mut last = None;
        for (id, call) in ids.iter().zip(calls) {
            let event = self
                .tasks
                .ready
                .values()
                .find(|r| &r.tool_run_id == id)
                .map(|r| r.event_id);
            let record = self
                .host
                .reply(
                    &self.tasks.db,
                    &self.tasks.workspace,
                    id,
                    call,
                    event.and_then(|id| self.tasks.ready.get(&id)),
                )
                .await?;
            contents.push(tool_reply(
                call,
                record.context_payload.as_deref().unwrap_or_default(),
            ));
            last = Some(record.id.clone());
            if let Some(event) = event {
                if let Some(row) = self.tasks.ready.get_mut(&event) {
                    row.delivery_message_id = Some(record.id.clone());
                    if let Some(json) = row.usage_json.take() {
                        usage.record(&serde_json::from_str(&json)?);
                    }
                }
                self.tasks.delivered(id);
                finish(events, call, &record);
            } else {
                emit(
                    events,
                    AgentEvent::ToolAccepted {
                        task_id: id.clone(),
                        tool_call_id: call.wire_call_id().into(),
                        name: call.function.name.clone(),
                    },
                );
            }
        }
        if let Some(error) = window_error {
            return Err(error);
        }
        Ok((contents, last))
    }
}

pub(crate) fn tool_reply(call: &ToolCall, content: &str) -> UserContent {
    UserContent::ToolResult(ToolResult {
        call: call.id.clone(),
        provider: call.provider.clone(),
        name: call.function.name.clone(),
        content: vec![ToolResultContent::text(content)],
    })
}

fn finish(events: &Channel<AgentEvent>, call: &ToolCall, record: &DispatcherMessageRecord) {
    emit(
        events,
        AgentEvent::ToolFinished {
            task_id: record.tool_task_id.clone(),
            tool_call_id: call.wire_call_id().into(),
            name: call.function.name.clone(),
            arguments: call.function.arguments.to_string(),
            display_text: record.plain_text(),
            context_payload: if record.role == "runtime" {
                serde_json::from_str::<serde_json::Value>(
                    record.context_payload.as_deref().unwrap_or_default(),
                )
                .ok()
                .and_then(|value| value["context_payload"].as_str().map(str::to_string))
                .unwrap_or_else(|| record.context_payload.clone().unwrap_or_default())
            } else {
                record.context_payload.clone().unwrap_or_default()
            },
            result_mode: record.tool_result_mode.clone().unwrap_or_default(),
            detail_refs: record.tool_artifacts.clone(),
        },
    );
}
