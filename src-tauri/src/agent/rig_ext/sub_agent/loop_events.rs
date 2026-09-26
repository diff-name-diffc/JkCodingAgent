//! 共享循环事件转换为子 Agent 轨迹；外层调用与内部 task_id 分别关联。
use super::events::{record_trace_event, SubAgentEvent, SubAgentEventPayload};
use crate::agent::rig_ext::events::AgentEvent;
use parking_lot::Mutex;
use serde_json::Value;
use std::sync::Arc;
use tauri::{
    ipc::{Channel, InvokeResponseBody},
    AppHandle, Emitter,
};

pub(super) fn channel(
    agent_id: String,
    agent_name: String,
    workspace: String,
    parent: String,
    app: Option<AppHandle>,
    trace: Arc<Mutex<Vec<Value>>>,
) -> Channel<AgentEvent> {
    // toolStarted 与 toolAccepted 都会映射成 ToolStarted（accepted 无 arguments），
    // trace 按 task_id 去重只记第一条；广播行为保持不变（前端 store 自行去重）。
    let seen_tool_starts = Mutex::new(std::collections::HashSet::new());
    Channel::new(move |body| {
        let InvokeResponseBody::Json(json) = body else {
            return Ok(());
        };
        let Ok(value) = serde_json::from_str::<Value>(&json) else {
            eprintln!("子 Agent 事件序列化契约失效");
            return Ok(());
        };
        let data = &value["data"];
        let name = data["name"].as_str().unwrap_or_default().to_string();
        let task_id = data["taskId"].as_str().map(str::to_string);
        let event = match value["event"].as_str() {
            Some("toolStarted" | "toolAccepted") => SubAgentEvent::ToolStarted {
                task_id,
                agent_id: agent_id.clone(),
                agent_name: agent_name.clone(),
                tool_name: name,
                arguments: data["arguments"]
                    .as_str()
                    .and_then(|raw| serde_json::from_str(raw).ok())
                    .unwrap_or_else(|| serde_json::json!({})),
            },
            Some("toolFinished") => {
                let result_preview =
                    tool_result_preview(&name, data["displayText"].as_str().unwrap_or_default());
                SubAgentEvent::ToolFinished {
                    task_id,
                    agent_id: agent_id.clone(),
                    agent_name: agent_name.clone(),
                    tool_name: name,
                    result_preview,
                }
            }
            _ => return Ok(()),
        };
        let should_record = match &event {
            SubAgentEvent::ToolStarted {
                task_id: Some(task_id),
                ..
            } => seen_tool_starts.lock().insert(task_id.clone()),
            _ => true,
        };
        let timestamp = chrono::Utc::now().timestamp_millis();
        if should_record {
            match serde_json::to_value(&event) {
                Ok(value) => record_trace_event(&trace, value, timestamp),
                Err(error) => eprintln!("子 Agent 轨迹写入失败：{error}"),
            }
        }
        if let Some(app) = &app {
            if let Err(error) = app.emit(
                "sub-agent-event",
                SubAgentEventPayload {
                    session_id: workspace.clone(),
                    tool_call_id: parent.clone(),
                    timestamp_ms: timestamp,
                    event,
                },
            ) {
                eprintln!("子 Agent 事件广播失败：{error}");
            }
        }
        Ok(())
    })
}

/// 事件结果预览：命令审查结果类保留更长预览（便于排障），其余 200 字符。
fn tool_result_preview(tool_name: &str, result: &str) -> String {
    let preview_limit = if is_command_review_result(tool_name, result) {
        4_000
    } else {
        200
    };
    if result.chars().count() > preview_limit {
        format!(
            "{}...",
            result.chars().take(preview_limit).collect::<String>()
        )
    } else {
        result.to_string()
    }
}

fn is_command_review_result(tool_name: &str, result: &str) -> bool {
    matches!(tool_name, "ssh_exec" | "local_zsh")
        && (result.starts_with("## SSH 命令审查记录")
            || (result.starts_with("## local_zsh 执行结果") && result.contains("审查结论: `拦截`")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_review_results_keep_longer_previews() {
        let review = format!("## SSH 命令审查记录\n{}", "x".repeat(1_000));
        // 命令审查结果在 4000 字符内全量保留（便于排障），普通工具 200 字符后截断。
        assert_eq!(
            tool_result_preview("ssh_exec", &review).chars().count(),
            review.chars().count()
        );
        assert_eq!(
            tool_result_preview("read_file", &"y".repeat(500))
                .chars()
                .count(),
            203
        );
    }

    #[test]
    fn tool_started_trace_is_deduped_by_task_id_but_broadcast_is_not() {
        let trace = Arc::new(Mutex::new(Vec::new()));
        let channel = channel(
            "a1".into(),
            "测试".into(),
            "ws-1".into(),
            "parent-call".into(),
            None,
            trace.clone(),
        );
        let send = |event: AgentEvent| channel.send(event).expect("channel 应接收事件");
        let started = || AgentEvent::ToolStarted {
            task_id: Some("t1".into()),
            tool_call_id: "c1".into(),
            name: "read_file".into(),
            arguments: "{}".into(),
        };
        send(started());
        send(AgentEvent::ToolAccepted {
            task_id: "t1".into(),
            tool_call_id: "c1".into(),
            name: "read_file".into(),
        });
        send(started());
        let events = trace.lock().clone();
        assert_eq!(events.len(), 1, "同一 task_id 只记第一条 ToolStarted");
        assert_eq!(events[0]["event"], Value::from("ToolStarted"));
    }
}
