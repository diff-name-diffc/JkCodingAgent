use std::path::Path;

use anyhow::Result;
use tauri::ipc::Channel;

use super::super::db::{DispatcherDb, DispatcherMessageRecord};
use super::super::tools::{
    parse_continue_instruction, parse_dispatch_instruction, parse_exit_instruction, DispatchAgent,
};
use crate::shared::truncate_for_display;

use super::helpers::{
    collect_recent_exploration_entries_from_db, compact_multiline, emit,
    should_include_latest_user_goal, summarize_dispatch_description,
};
use super::subprocess::{ProtocolBatchState, ProtocolToolAction};
use super::types::AgentEvent;
use super::DispatcherAgent;

// ─── Protocol action planning ─────────────────────────────────────────────────
// 子进程协议（dispatch / continue / exit）的规划与发射。
// 这些动作不在主循环本地执行，而是转为 UI 事件交由前端启动子进程，
// 子进程完成后通过 continue_after_dispatch 回流结果。

impl DispatcherAgent {
    /// 判定一个 tool_call 是否属于子进程协议动作，若是则规划出对应的
    /// ProtocolToolAction（Dispatch/Continue/Exit）。
    /// dispatch_id 在规划阶段即生成，供后续 continue/exit 反馈关联同一次派生。
    pub(super) async fn plan_protocol_action(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        tool_call: &super::super::llm::RequestedToolCall,
        protocol_state: &mut ProtocolBatchState,
    ) -> std::result::Result<Option<ProtocolToolAction>, String> {
        if let Some(agent) = DispatchAgent::from_dispatch_tool_name(&tool_call.name) {
            // The dispatch_id is minted during planning, before any UI event is emitted. Later
            // continue/exit feedback uses the same id to reconnect the external subprocess result
            // to this main-agent turn.
            protocol_state.ensure_dispatch_allowed(agent.slug(), agent.display_name())?;
            let (task_description, permission_mode) =
                parse_dispatch_instruction(&tool_call.arguments, agent)?;
            let dispatch_id = uuid::Uuid::new_v4().to_string();
            let description = summarize_dispatch_description(&task_description);
            let task_prompt = self
                .build_subprocess_task_prompt(db, workspace_id, agent, &task_description)
                .await?;
            protocol_state.record_dispatch(agent.slug(), &dispatch_id);
            return Ok(Some(ProtocolToolAction::Dispatch {
                dispatch_id,
                agent,
                description,
                task_prompt,
                permission_mode,
            }));
        }

        if let Some(agent) = DispatchAgent::from_continue_tool_name(&tool_call.name) {
            // Continue/exit target an already active subprocess for this agent. "active" is kept as
            // a compatibility fallback for older sessions that do not have a concrete dispatch_id.
            protocol_state.ensure_continue_allowed(agent.slug(), agent.display_name())?;
            let text = parse_continue_instruction(&tool_call.arguments, agent)?;
            let dispatch_id = protocol_state
                .dispatch_id_for_agent(agent.slug())
                .unwrap_or("active")
                .to_string();
            protocol_state.record_continue(agent.slug());
            return Ok(Some(ProtocolToolAction::Continue {
                dispatch_id,
                agent,
                text,
            }));
        }

        if let Some(agent) = DispatchAgent::from_exit_tool_name(&tool_call.name) {
            protocol_state.ensure_exit_allowed(agent.slug(), agent.display_name())?;
            let reason = parse_exit_instruction(&tool_call.arguments, agent);
            let dispatch_id = protocol_state
                .dispatch_id_for_agent(agent.slug())
                .unwrap_or("active")
                .to_string();
            protocol_state.record_exit(agent.slug());
            return Ok(Some(ProtocolToolAction::Exit {
                dispatch_id,
                agent,
                reason,
            }));
        }

        Ok(None)
    }

    /// 持久化协议动作并发射对应的 UI 事件（DispatchProposed/Continue/Exit）。
    /// 协议动作虽不本地执行，但仍记为 tool 消息，保证下一次 LLM 请求能看到
    /// 交给 UI/子进程层的确切指令——保持因果链完整。
    pub(super) async fn emit_protocol_action(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        on_event: &Channel<AgentEvent>,
        tool_call: &super::super::llm::RequestedToolCall,
        action: &ProtocolToolAction,
    ) -> Result<DispatcherMessageRecord> {
        // Protocol actions are persisted as tool messages even though their real work happens
        // outside this loop. That makes the next LLM request see exactly what command was handed
        // to the UI/subprocess layer.
        let result = match action {
            ProtocolToolAction::Dispatch {
                dispatch_id,
                agent,
                description,
                task_prompt,
                permission_mode,
            } => {
                if let Some(checklist) = super::planning::reserve_checklist_dispatch(
                    db,
                    workspace_id,
                    dispatch_id,
                    agent.slug(),
                    description,
                )
                .await
                .map_err(anyhow::Error::msg)?
                {
                    emit(
                        on_event,
                        AgentEvent::ChecklistPlanUpdated { state: checklist },
                    );
                }
                emit(
                    on_event,
                    AgentEvent::DispatchProposed {
                        dispatch_id: dispatch_id.clone(),
                        agent: agent.slug().to_string(),
                        description: description.clone(),
                        task_prompt: task_prompt.clone(),
                        permission_mode: permission_mode.clone(),
                    },
                );

                format!(
                    "[{} 子任务已提交审查] dispatch_id={}, 任务: {}",
                    agent.display_name(),
                    dispatch_id,
                    truncate_for_display(description, 200, "...")
                )
            }
            ProtocolToolAction::Continue {
                dispatch_id,
                agent,
                text,
            } => {
                emit(
                    on_event,
                    AgentEvent::DispatchContinue {
                        dispatch_id: dispatch_id.clone(),
                        agent: agent.slug().to_string(),
                        text: text.clone(),
                    },
                );

                format!(
                    "[已发送后续指令到 {} 会话] 指令: {}",
                    agent.display_name(),
                    truncate_for_display(text, 200, "...")
                )
            }
            ProtocolToolAction::Exit {
                dispatch_id,
                agent,
                reason,
            } => {
                emit(
                    on_event,
                    AgentEvent::DispatchExit {
                        dispatch_id: dispatch_id.clone(),
                        agent: agent.slug().to_string(),
                        reason: reason.clone(),
                    },
                );

                format!(
                    "[已发送退出命令到 {} 会话] 原因: {}",
                    agent.display_name(),
                    reason
                )
            }
        };

        emit(
            on_event,
            AgentEvent::ToolFinished {
                tool_call_id: Some(tool_call.id.clone()),
                name: tool_call.name.clone(),
                display_text: result.clone(),
                result_mode: "raw".to_string(),
                detail_refs: Vec::new(),
            },
        );
        let message = db
            .add_visible_message_with_tools_async(
                workspace_id,
                "tool",
                &result,
                Some(&tool_call.id),
                Some(&tool_call.name),
                Some("raw"),
                None,
            )
            .await?;
        Ok(message)
    }

    /// 为子进程拼装紧凑的任务提示：任务目标 + 计划书 + 用户诉求 + 探索上下文。
    /// 刻意精简——子进程拿到的是聚焦的任务包，而非完整聊天记录。
    pub(super) async fn build_subprocess_task_prompt(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        _agent: DispatchAgent,
        task_description: &str,
    ) -> std::result::Result<String, String> {
        // Subprocess prompts are intentionally compact: the child agent gets the task, the active
        // plan, and a small slice of confirmed exploration context rather than the full chat log.
        let latest_user_goal = db
            .get_latest_user_message_content_async(workspace_id)
            .await
            .map_err(|error| format!("读取最新用户消息失败：{error}"))?
            .as_deref()
            .map(|text| compact_multiline(text.trim(), 240))
            .filter(|text| !text.is_empty());
        let explored_index_info = collect_recent_exploration_entries_from_db(db, workspace_id)
            .await
            .map_err(|error| format!("读取探索上下文失败：{error}"))?;
        let active_plan_path = db
            .get_session_runtime_state_async(workspace_id)
            .await
            .map_err(|error| format!("读取调度运行态失败：{error}"))?
            .active_plan_path
            .filter(|path| !super::helpers::is_implemented_plan_path(Path::new(path)));

        let mut sections = vec![format!("【任务目标】\n{}", task_description.trim())];

        if let Some(plan_path) = active_plan_path {
            sections.push(format!(
                "【计划书】\n请先读取并严格按照该计划书执行编码任务：{plan_path}"
            ));
        }

        if let Some(goal) =
            latest_user_goal.filter(|goal| should_include_latest_user_goal(goal, task_description))
        {
            sections.push(format!("【用户诉求】\n{}", goal));
        }

        if !explored_index_info.is_empty() {
            sections.push(format!("【已确认上下文】\n{}", explored_index_info));
        }

        sections.push(
            "【执行要求】\n\
- 优先直接完成目标；只有在上下文不足或与代码现场冲突时，才补做最少量验证。\n\
- 输出聚焦：实际改动或结论、验证结果、剩余风险；默认使用简体中文。"
                .to_string(),
        );

        Ok(sections.join("\n\n"))
    }
}

// ─── Protocol waiting message ─────────────────────────────────────────────────

pub(crate) fn build_protocol_waiting_message(
    actions: &[ProtocolToolAction],
    auto_approve_dispatch: bool,
    final_message: Option<&str>,
) -> String {
    let mut sections = Vec::new();

    let dispatch_lines = actions
        .iter()
        .filter_map(|action| match action {
            ProtocolToolAction::Dispatch {
                agent,
                description,
                dispatch_id,
                ..
            } => Some(format!(
                "- [{}] dispatch_id={} {}",
                agent.display_name(),
                dispatch_id,
                truncate_for_display(description, 200, "...")
            )),
            _ => None,
        })
        .collect::<Vec<_>>();
    if !dispatch_lines.is_empty() {
        let header = if auto_approve_dispatch {
            format!(
                "📋 已自动批准 {} 个子任务，正在执行：",
                dispatch_lines.len()
            )
        } else {
            format!(
                "📋 已提交 {} 个子任务审查，等待执行：",
                dispatch_lines.len()
            )
        };
        sections.push(format!("{}\n{}", header, dispatch_lines.join("\n")));
    }

    let continue_lines = actions
        .iter()
        .filter_map(|action| match action {
            ProtocolToolAction::Continue { agent, text, .. } => Some(format!(
                "- [{}] {}",
                agent.display_name(),
                truncate_for_display(text, 200, "...")
            )),
            _ => None,
        })
        .collect::<Vec<_>>();
    if !continue_lines.is_empty() {
        sections.push(format!(
            "📨 已发送 {} 条后续指令，等待执行：\n{}",
            continue_lines.len(),
            continue_lines.join("\n")
        ));
    }

    let exit_lines = actions
        .iter()
        .filter_map(|action| match action {
            ProtocolToolAction::Exit { agent, reason, .. } => {
                Some(format!("- [{}] {}", agent.display_name(), reason))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    if !exit_lines.is_empty() {
        sections.push(format!(
            "⏹️ 已发送 {} 条退出命令，等待进程结束：\n{}",
            exit_lines.len(),
            exit_lines.join("\n")
        ));
    }

    if let Some(message) = final_message
        .map(str::trim)
        .filter(|message| !message.is_empty())
    {
        sections.push(format!("补充说明：\n{}", message));
    }

    sections.join("\n\n")
}
