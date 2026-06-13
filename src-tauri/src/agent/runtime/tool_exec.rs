use std::collections::HashSet;
use std::path::Path;

use anyhow::Result;
use futures::future::join_all;
use tauri::ipc::Channel;
use tokio::sync::watch;

use super::super::common::{
    self, build_args_map, build_tool_calls_payload, cancellation_requested,
    persist_tool_calls_message, persist_tool_result_raw,
};
use super::super::db::{
    DispatcherDb, DispatcherSessionRuntimeState, DispatcherSessionTokenUsageSource,
};
use super::super::llm::{LlmResponse, RequestedToolCall};
use super::super::sub_agent::tool::sub_agent_failure_message;
use super::super::tools::ToolContext;

use super::helpers::{
    build_tool_retry_context, disallowed_tool_result, emit, extract_message_content,
    is_retryable_tool_error, record_run_token_usage,
};
use super::planning::PlanningToolOutcome;
use super::subprocess::{ProtocolBatchState, ProtocolToolAction};
use super::types::AgentEvent;
use super::DispatcherAgent;

// ─── Internal types ───────────────────────────────────────────────────────────

pub(super) struct ToolCallsOutcome {
    pub saw_retryable_tool_error: bool,
    pub planning_waiting_message: Option<String>,
    pub final_message: Option<String>,
    pub protocol_actions: Vec<ProtocolToolAction>,
}

pub(super) enum SingleToolDisposition {
    Handled,
    HandledWithRetry,
    WaitForUser(String),
    ProtocolAction(ProtocolToolAction),
    NeedsSummary,
}

// ─── Tool execution impl ──────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
impl DispatcherAgent {
    pub(super) async fn execute_tool_calls(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        workspace: &Path,
        on_event: &Channel<AgentEvent>,
        response: LlmResponse,
        runtime_state: &DispatcherSessionRuntimeState,
        allowed_tool_names: &HashSet<String>,
        tool_context: &ToolContext,
        cancel_rx: &watch::Receiver<bool>,
        request_provider: &super::super::llm::OpenAiCompatProvider,
        usage_tracker: &mut super::super::common::UsageTracker,
    ) -> Result<ToolCallsOutcome> {
        // Persist tool calls and emit ToolPlanned events
        let tool_calls = response.tool_calls.clone();
        let tool_calls_payload = build_tool_calls_payload(&tool_calls, &self.tools);
        let args_map = build_args_map(&tool_calls, &self.tools);

        for tc in &tool_calls_payload {
            emit(
                on_event,
                AgentEvent::ToolPlanned {
                    tool_call_id: Some(tc.id.clone()),
                    name: tc.function.name.clone(),
                    arguments: tc.function.arguments.clone(),
                },
            );
        }

        persist_tool_calls_message(
            db,
            workspace_id,
            &response.content,
            &tool_calls_payload,
            &response.thinking_content,
            Some(response.thinking_elapsed_ms),
        )
        .await?;

        let mut protocol_state =
            ProtocolBatchState::new(self.active_subprocesses_for_workspace(workspace_id));
        let mut protocol_actions = Vec::new();
        let mut planning_waiting_message: Option<String> = None;
        let mut final_message: Option<String> = None;
        let mut saw_retryable_tool_error = false;

        // Execute tool calls in order, parallelizing adjacent readonly ones
        let mut tool_call_index = 0usize;
        'outer: while tool_call_index < tool_calls.len() {
            if cancellation_requested(cancel_rx) {
                break;
            }

            let readonly_end = common::readonly_tool_run_end(&tool_calls, tool_call_index);
            let ready_tool_results = if readonly_end.saturating_sub(tool_call_index) >= 2 {
                let run = &tool_calls[tool_call_index..readonly_end];
                let results = self
                    .execute_parallel_readonly_tools(
                        run,
                        tool_context,
                        on_event,
                        allowed_tool_names,
                    )
                    .await;
                let items = run
                    .iter()
                    .cloned()
                    .zip(results)
                    .collect::<Vec<(RequestedToolCall, String)>>();
                tool_call_index = readonly_end;
                items
            } else {
                let tool_call = tool_calls[tool_call_index].clone();
                tool_call_index += 1;
                let tool_args_json = args_map
                    .get(&tool_call.id)
                    .cloned()
                    .unwrap_or_else(|| "{}".to_string());
                emit(
                    on_event,
                    AgentEvent::ToolStarted {
                        tool_call_id: Some(tool_call.id.clone()),
                        name: tool_call.name.clone(),
                        arguments: tool_args_json,
                    },
                );
                let result = if allowed_tool_names.contains(&tool_call.name) {
                    self.tools
                        .execute(&tool_call.name, &tool_call.arguments, tool_context)
                        .await
                } else {
                    disallowed_tool_result(&tool_call.name)
                };
                vec![(tool_call, result)]
            };

            for (tool_call, result) in ready_tool_results {
                if cancellation_requested(cancel_rx) {
                    break 'outer;
                }
                if let Some(message) = sub_agent_failure_message(&result) {
                    anyhow::bail!("{}", message);
                }

                match self
                    .process_single_tool_call(
                        db,
                        workspace_id,
                        workspace,
                        on_event,
                        &tool_call,
                        runtime_state,
                        &mut protocol_state,
                    )
                    .await?
                {
                    SingleToolDisposition::Handled => {}
                    SingleToolDisposition::HandledWithRetry => {
                        saw_retryable_tool_error = true;
                    }
                    SingleToolDisposition::WaitForUser(msg) => {
                        planning_waiting_message = Some(msg);
                    }
                    SingleToolDisposition::ProtocolAction(action) => {
                        protocol_actions.push(action);
                    }
                    SingleToolDisposition::NeedsSummary => {
                        if is_retryable_tool_error(&tool_call.name, &result) {
                            self.emit_tool_retry_feedback(
                                db,
                                workspace_id,
                                on_event,
                                &tool_call,
                                &result,
                            )
                            .await?;
                            saw_retryable_tool_error = true;
                            continue;
                        }

                        let summary_model = self.summary_model();
                        let summary_provider = self.summary_provider(request_provider);
                        common::persist_tool_result_with_compression(
                            db,
                            workspace_id,
                            on_event,
                            &tool_call,
                            &result,
                            &summary_provider,
                            &summary_model,
                            |usage| {
                                record_run_token_usage(
                                    db,
                                    workspace_id,
                                    &summary_model,
                                    DispatcherSessionTokenUsageSource::Summary,
                                    usage,
                                    usage_tracker,
                                    on_event,
                                );
                            },
                        )
                        .await?;

                        if let Err(error) = db
                            .compact_successful_tool_retry_async(
                                workspace_id,
                                &tool_call.name,
                                &tool_call.id,
                            )
                            .await
                        {
                            eprintln!(
                                "failed to compact dispatcher tool retry messages for workspace {} and tool {}: {}",
                                workspace_id, tool_call.name, error
                            );
                        }

                        if tool_call.name == "message" {
                            if let Some(content) = extract_message_content(&tool_call.arguments) {
                                final_message = Some(content);
                            }
                        }
                    }
                }
            }
        }

        Ok(ToolCallsOutcome {
            saw_retryable_tool_error,
            planning_waiting_message,
            final_message,
            protocol_actions,
        })
    }

    /// Classify a single tool call through the planning/protocol priority waterfall.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn process_single_tool_call(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        workspace: &Path,
        on_event: &Channel<AgentEvent>,
        tool_call: &RequestedToolCall,
        runtime_state: &DispatcherSessionRuntimeState,
        protocol_state: &mut ProtocolBatchState,
    ) -> Result<SingleToolDisposition> {
        // Priority 1: planning tools (update_plan, present_plan, etc.)
        match self
            .execute_planning_tool(
                db,
                workspace_id,
                workspace,
                on_event,
                tool_call,
                runtime_state,
            )
            .await
        {
            Ok(Some(PlanningToolOutcome::ToolResult(res))) => {
                persist_tool_result_raw(db, workspace_id, on_event, tool_call, &res).await?;
                return Ok(SingleToolDisposition::Handled);
            }
            Ok(Some(PlanningToolOutcome::WaitForUser(res))) => {
                persist_tool_result_raw(db, workspace_id, on_event, tool_call, &res).await?;
                return Ok(SingleToolDisposition::WaitForUser(res));
            }
            Ok(None) => {} // not a planning tool — fall through to protocol check
            Err(error) => {
                let is_retryable = is_retryable_tool_error(&tool_call.name, &error);
                self.handle_tool_call_error(db, workspace_id, on_event, tool_call, &error)
                    .await?;
                return Ok(if is_retryable {
                    SingleToolDisposition::HandledWithRetry
                } else {
                    SingleToolDisposition::Handled
                });
            }
        }

        // Priority 2: protocol actions (dispatch, continue, exit subprocess)
        match self
            .plan_protocol_action(db, workspace_id, tool_call, protocol_state)
            .await
        {
            Ok(Some(action)) => {
                if let ProtocolToolAction::Exit { agent, .. } = &action {
                    self.mark_agent_exit_requested(workspace_id, agent.slug());
                }
                self.emit_protocol_action(db, workspace_id, on_event, tool_call, &action)
                    .await?;
                return Ok(SingleToolDisposition::ProtocolAction(action));
            }
            Ok(None) => {} // not a protocol action — fall through
            Err(error) => {
                let is_retryable = is_retryable_tool_error(&tool_call.name, &error);
                self.handle_tool_call_error(db, workspace_id, on_event, tool_call, &error)
                    .await?;
                return Ok(if is_retryable {
                    SingleToolDisposition::HandledWithRetry
                } else {
                    SingleToolDisposition::Handled
                });
            }
        }

        // Priority 3: neither planning nor protocol — needs standard summary processing
        Ok(SingleToolDisposition::NeedsSummary)
    }

    pub(super) async fn handle_tool_call_error(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        on_event: &Channel<AgentEvent>,
        tool_call: &RequestedToolCall,
        error: &str,
    ) -> Result<()> {
        if is_retryable_tool_error(&tool_call.name, error) {
            self.emit_tool_retry_feedback(db, workspace_id, on_event, tool_call, error)
                .await?;
        } else {
            self.emit_tool_error(db, workspace_id, on_event, tool_call, error)
                .await?;
        }
        Ok(())
    }

    pub(super) async fn resolve_loop_outcome(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        on_event: &Channel<AgentEvent>,
        outcome: ToolCallsOutcome,
        usage_tracker: &super::super::common::UsageTracker,
    ) -> Result<Option<super::super::db::DispatcherMessageRecord>> {
        if outcome.saw_retryable_tool_error {
            return Ok(None);
        }

        if let Some(waiting_content) = outcome.planning_waiting_message {
            let usage_stats = usage_tracker.snapshot();
            let waiting_msg = db
                .add_visible_message_with_usage_async(
                    workspace_id,
                    "assistant",
                    &waiting_content,
                    &usage_stats,
                )
                .await?;
            emit(
                on_event,
                AgentEvent::AssistantMessage {
                    message: waiting_msg.clone(),
                },
            );
            return Ok(Some(waiting_msg));
        }

        if !outcome.protocol_actions.is_empty() {
            let waiting_content = super::protocol::build_protocol_waiting_message(
                &outcome.protocol_actions,
                self.auto_approve_dispatch(),
                outcome.final_message.as_deref(),
            );
            let usage_stats = usage_tracker.snapshot();
            let waiting_msg = db
                .add_visible_message_with_usage_async(
                    workspace_id,
                    "assistant",
                    &waiting_content,
                    &usage_stats,
                )
                .await?;
            emit(
                on_event,
                AgentEvent::AssistantMessage {
                    message: waiting_msg.clone(),
                },
            );
            return Ok(Some(waiting_msg));
        }

        if let Some(final_message) = outcome.final_message {
            let usage_stats = usage_tracker.snapshot();
            let reply = db
                .add_visible_message_with_usage_async(
                    workspace_id,
                    "assistant",
                    &final_message,
                    &usage_stats,
                )
                .await?;
            emit(
                on_event,
                AgentEvent::AssistantMessage {
                    message: reply.clone(),
                },
            );
            return Ok(Some(reply));
        }

        Ok(None)
    }

    pub(super) async fn execute_parallel_readonly_tools(
        &self,
        tool_calls: &[RequestedToolCall],
        tool_context: &ToolContext,
        on_event: &Channel<AgentEvent>,
        allowed_tool_names: &HashSet<String>,
    ) -> Vec<String> {
        for tool_call in tool_calls {
            let enriched = self
                .tools
                .effective_args(&tool_call.name, &tool_call.arguments);
            emit(
                on_event,
                AgentEvent::ToolStarted {
                    tool_call_id: Some(tool_call.id.clone()),
                    name: tool_call.name.clone(),
                    arguments: serde_json::to_string(&enriched)
                        .unwrap_or_else(|_| "{}".to_string()),
                },
            );
        }

        let results = join_all(tool_calls.iter().map(|tool_call| async move {
            if allowed_tool_names.contains(&tool_call.name) {
                self.tools
                    .execute(&tool_call.name, &tool_call.arguments, tool_context)
                    .await
            } else {
                disallowed_tool_result(&tool_call.name)
            }
        }))
        .await;

        results
    }

    pub(super) async fn emit_tool_retry_feedback(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        on_event: &Channel<AgentEvent>,
        tool_call: &RequestedToolCall,
        error: &str,
    ) -> Result<()> {
        let context_payload = build_tool_retry_context(tool_call, error);
        let display_text = "工具调用参数需要修正，已交回模型重试。";
        emit(
            on_event,
            AgentEvent::ToolFinished {
                tool_call_id: Some(tool_call.id.clone()),
                name: tool_call.name.clone(),
                display_text: display_text.to_string(),
                result_mode: "raw".to_string(),
                detail_refs: Vec::new(),
            },
        );
        db.add_visible_tool_result_async(
            workspace_id,
            display_text,
            &context_payload,
            Some(&tool_call.id),
            Some(&tool_call.name),
            Some("raw"),
            &[],
        )
        .await?;
        Ok(())
    }

    pub(super) async fn emit_tool_error(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        on_event: &Channel<AgentEvent>,
        tool_call: &RequestedToolCall,
        error: &str,
    ) -> Result<()> {
        emit(
            on_event,
            AgentEvent::ToolFinished {
                tool_call_id: Some(tool_call.id.clone()),
                name: tool_call.name.clone(),
                display_text: error.to_string(),
                result_mode: "raw".to_string(),
                detail_refs: Vec::new(),
            },
        );
        db.add_visible_message_with_tools_async(
            workspace_id,
            "tool",
            error,
            Some(&tool_call.id),
            Some(&tool_call.name),
            Some("raw"),
            None,
        )
        .await?;
        Ok(())
    }
}
