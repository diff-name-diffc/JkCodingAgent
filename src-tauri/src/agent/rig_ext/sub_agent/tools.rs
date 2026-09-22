//! 子智能体工具（rig `PortableDynamicTool` 形态）。
//!
//! 迁移自旧 `agent/sub_agent/tool.rs`：
//! - `call_sub_agent`：委派子任务；构建失败/执行失败一律按**致命失败**返回
//!   （`with_code("fatal")`）——父循环不得基于不完整的委派结果继续推理，
//!   与旧 `ToolResult::fatal_error` 语义一致；轨迹无论成败都持久化。
//! - `list_sub_agents`：枚举本会话可用子智能体（同步 SQLite 读取走
//!   `spawn_blocking`）。
//! - `notify_user_progress`：子智能体专用进度通知（写轨迹 + 发
//!   `sub-agent-event`）；由子智能体运行时构造时绑定其身份与父调用关联，
//!   非子智能体上下文不注册该工具。

use std::sync::Arc;

use parking_lot::Mutex;
use rig::tool::{PortableDynamicTool, ToolExecutionError, ToolOutput};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

use super::events::{record_trace_event, SubAgentEvent, SubAgentEventPayload};
use super::runner::{RigSubAgentRequest, RigSubAgentRuntime};
use crate::agent::rig_ext::model::PurposeModelSpec;
use crate::agent::rig_ext::tools::deps::{RigToolDeps, ToolCallSlot};
use crate::agent::sub_agent::manager::SubAgentManager;

/// `call_sub_agent`：委派子任务给指定子智能体。
pub fn call_sub_agent_tool(
    manager: Arc<SubAgentManager>,
    deps: RigToolDeps,
    parent_spec: PurposeModelSpec,
    app_handle: Option<AppHandle>,
    workspace_id: String,
    tool_call_id: ToolCallSlot,
) -> PortableDynamicTool {
    PortableDynamicTool::new(
        "call_sub_agent",
        "调用一个子智能体执行特定领域的复杂任务。子智能体拥有独立的执行上下文，内部的工具调用过程对你透明，你只会收到最终结果。可用子智能体列表通过 list_sub_agents 获取。",
        json!({
            "type": "object",
            "properties": {
                "agent_id": {
                    "type": "string",
                    "description": "子智能体的 ID。通过 list_sub_agents 查看可用列表。"
                },
                "task": {
                    "type": "string",
                    "description": "要交给子智能体的任务描述，应清晰说明期望的行为和输出格式。"
                }
            },
            "required": ["agent_id", "task"]
        }),
        move |args| {
            let manager = Arc::clone(&manager);
            let deps = deps.clone();
            let parent_spec = parent_spec.clone();
            let app_handle = app_handle.clone();
            let workspace_id = workspace_id.clone();
            let tool_call_id = tool_call_id.clone();
            Box::pin(async move {
                run_sub_agent_call(
                    &args,
                    manager,
                    deps,
                    parent_spec,
                    app_handle,
                    workspace_id,
                    tool_call_id,
                )
                .await
            })
        },
    )
}

#[allow(clippy::too_many_arguments)]
async fn run_sub_agent_call(
    args: &Value,
    manager: Arc<SubAgentManager>,
    deps: RigToolDeps,
    parent_spec: PurposeModelSpec,
    app_handle: Option<AppHandle>,
    workspace_id: String,
    tool_call_id: ToolCallSlot,
) -> Result<ToolOutput, ToolExecutionError> {
    // LLM 传参常带前后空白/换行：先 trim 再校验（避免纯空白参数白跑一次）。
    let agent_id = args
        .get("agent_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    let task = args
        .get("task")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if agent_id.is_empty() {
        return Err(fatal("错误：agent_id 参数不能为空"));
    }
    if task.is_empty() {
        return Err(fatal("错误：task 参数不能为空"));
    }

    let config = match manager.get(agent_id) {
        Some(config) if config.enabled => config,
        Some(_) => {
            return Err(fatal(format!("错误：子智能体 '{agent_id}' 已被禁用")));
        }
        None => return Err(fatal(format!("错误：未找到子智能体 '{agent_id}'"))),
    };

    let Some(parent_tool_call_id) = tool_call_id.get() else {
        return Err(fatal("错误：调用子智能体缺少 tool_call_id"));
    };

    let request = RigSubAgentRequest {
        config: &config,
        parent_spec: &parent_spec,
        deps: &deps,
        task,
        parent_tool_call_id: &parent_tool_call_id,
        app_handle: app_handle.clone(),
        session_id: &workspace_id,
        cancel_rx: deps.cancel_rx.clone(),
    };
    let runtime = match RigSubAgentRuntime::build(&request) {
        Ok(runtime) => runtime,
        Err(error) => return Err(fatal(format!("错误：子智能体初始化失败：{error}"))),
    };

    let outcome = runtime.execute(task).await;
    let status = if outcome.is_ok() { "completed" } else { "failed" };
    let trace_json = match runtime.trace_events_json() {
        Ok(trace) => trace,
        Err(error) => return Err(fatal(error)),
    };
    // 运行实际模型随轨迹持久化（UI-14 遗留）：回放权威源，Started 事件被
    // 容量裁剪逐出后仍可显示真实模型。
    let persist_model = runtime.model().to_string();
    let persist_workspace_id = workspace_id.clone();
    let persist_tool_call_id = parent_tool_call_id.clone();
    let persist_agent_id = config.agent_id.clone();
    let persist_status = status.to_string();
    let persist = tokio::task::spawn_blocking(move || {
        manager.save_run_trace(
            &persist_workspace_id,
            &persist_tool_call_id,
            &persist_agent_id,
            &persist_status,
            &trace_json,
            Some(persist_model.as_str()),
        )
    })
    .await;
    match persist {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => {
            return Err(fatal(format!("错误：子智能体轨迹持久化失败：{error}")));
        }
        Err(error) => {
            return Err(fatal(format!("错误：子智能体轨迹任务失败：{error}")));
        }
    }

    match outcome {
        Ok(result) => Ok(ToolOutput::text(result)),
        Err(error) => Err(fatal(format!("错误：子智能体执行失败：{error}"))),
    }
}

/// `list_sub_agents`：列出本会话可用子智能体。
pub fn list_sub_agents_tool(
    manager: Arc<SubAgentManager>,
    workspace_id: String,
) -> PortableDynamicTool {
    PortableDynamicTool::new(
        "list_sub_agents",
        "列出当前可用的全部子智能体及其描述，帮助你决定调用哪个子智能体。",
        json!({ "type": "object", "properties": {} }),
        move |_args| {
            let manager = Arc::clone(&manager);
            let workspace_id = workspace_id.clone();
            Box::pin(async move {
                // get_enabled_for_session 内部是同步 SQLite 读取：阻塞 I/O
                // 必须走 spawn_blocking（项目规范）。
                let joined =
                    tokio::task::spawn_blocking(move || manager.get_enabled_for_session(&workspace_id))
                        .await;
                let configs = match joined {
                    Ok(Ok(configs)) => configs,
                    Ok(Err(error)) => {
                        return Ok(ToolOutput::text(format!(
                            "错误：无法获取子智能体列表：{error}"
                        )))
                    }
                    Err(error) => {
                        return Ok(ToolOutput::text(format!(
                            "错误：子智能体列表查询任务失败：{error}"
                        )))
                    }
                };

                if configs.is_empty() {
                    return Ok(ToolOutput::text(
                        "当前会话没有可用的子智能体。请在设置中配置并启用子智能体。",
                    ));
                }

                let mut output = String::from("可用的子智能体列表：\n\n");
                for config in &configs {
                    output.push_str(&format!(
                        "### {} ({})\n{}\n\n",
                        config.agent_name, config.agent_id, config.description
                    ));
                }
                output.push_str("使用 call_sub_agent(agent_id=\"...\", task=\"...\") 来调用子智能体。");
                Ok(ToolOutput::text(output))
            })
        },
    )
}

/// `notify_user_progress`：子智能体专用进度通知。
pub fn notify_user_progress_tool(
    agent_id: String,
    agent_name: String,
    parent_tool_call_id: String,
    app_handle: Option<AppHandle>,
    trace_events: Arc<Mutex<Vec<Value>>>,
    workspace_id: String,
) -> PortableDynamicTool {
    PortableDynamicTool::new(
        "notify_user_progress",
        "子智能体专用：向用户主动发送阶段性进度、当前发现、阻塞点或下一步计划。适合长时间任务中每完成一个有意义阶段调用一次，消息会直接展示在正文中。",
        json!({
            "type": "object",
            "properties": {
                "message": {
                    "type": "string",
                    "description": "给用户看的简洁进度说明，应说明当前状态、已完成内容、正在做什么或遇到的阻塞。"
                }
            },
            "required": ["message"]
        }),
        move |args| {
            let agent_id = agent_id.clone();
            let agent_name = agent_name.clone();
            let parent_tool_call_id = parent_tool_call_id.clone();
            let app_handle = app_handle.clone();
            let trace_events = Arc::clone(&trace_events);
            let workspace_id = workspace_id.clone();
            Box::pin(async move {
                let message = args
                    .get("message")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(trim_progress_message);
                let Some(message) = message else {
                    return Err(fatal("错误：message 参数不能为空"));
                };
                let event = SubAgentEvent::Progress {
                    agent_id,
                    agent_name,
                    message: message.clone(),
                };
                let timestamp_ms = chrono::Utc::now().timestamp_millis();
                if let Ok(value) = serde_json::to_value(&event) {
                    record_trace_event(&trace_events, value, timestamp_ms);
                }
                if let Some(handle) = &app_handle {
                    if let Err(error) = handle.emit(
                        "sub-agent-event",
                        SubAgentEventPayload {
                            session_id: workspace_id,
                            tool_call_id: parent_tool_call_id,
                            timestamp_ms,
                            event,
                        },
                    ) {
                        return Err(fatal(format!("错误：发送进度通知失败：{error}")));
                    }
                }
                Ok(ToolOutput::text(format!("已通知用户：{message}")))
            })
        },
    )
}

/// 委派失败的致命错误（父循环据此中止 run）。
fn fatal(message: impl Into<String>) -> ToolExecutionError {
    ToolExecutionError::other(message).with_code("fatal")
}

fn trim_progress_message(message: &str) -> String {
    const MAX_CHARS: usize = 2_000;
    let trimmed = message.trim();
    if trimmed.chars().count() <= MAX_CHARS {
        return trimmed.to_string();
    }
    format!("{}...", trimmed.chars().take(MAX_CHARS).collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::trim_progress_message;

    #[test]
    fn progress_message_is_bounded() {
        assert_eq!(trim_progress_message("  进行中  "), "进行中");
        let long = "字".repeat(2_100);
        let trimmed = trim_progress_message(&long);
        assert_eq!(trimmed.chars().count(), 2_003);
        assert!(trimmed.ends_with("..."));
    }
}
