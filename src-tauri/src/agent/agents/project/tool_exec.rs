use std::collections::HashSet;
use std::path::Path;

use anyhow::Result;
use futures::future::join_all;
use tauri::ipc::Channel;
use tokio::sync::watch;

use crate::agent::common::{
    self, build_args_map, build_tool_calls_payload, cancellation_requested,
    persist_tool_calls_message, with_usage_paused,
};
use crate::agent::db::{DispatcherDb, DispatcherMessageRecord, DispatcherSessionTokenUsageSource};
use crate::agent::llm::{ChatMessage, LlmResponse, OpenAiCompatProvider, RequestedToolCall};
use crate::agent::run_loop::{core::RunLoopToolOutcome, AgentEvent};
use crate::agent::sub_agent::tool::sub_agent_failure_message;
use crate::agent::tools::{
    ToolAction, ToolContext, ToolResult, ToolRunFinishUpdate, ToolRuntime, ToolStatus,
};

use super::helpers::{
    build_tool_retry_context, emit, extract_message_content, is_retryable_tool_error,
    record_run_token_usage,
};
use super::subprocess::{ProtocolBatchState, ProtocolToolAction};
use super::DispatcherAgent;

// ─── Internal types ───────────────────────────────────────────────────────────

/// 单个工具调用经三级优先瀑布处理后的处置分类（见 process_single_tool_call）。
pub(super) enum SingleToolDisposition {
    Handled,
    HandledWithRetry,
    ProtocolAction(ProtocolToolAction),
    NeedsSummary,
}

pub(super) struct ExecutedToolCall {
    pub tool_call: RequestedToolCall,
    pub result: ToolResult,
    pub run_id: Option<String>,
}

// ─── Tool execution impl ──────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
impl DispatcherAgent {
    /// 执行一批 tool_calls：先持久化 assistant 工具调用消息（协议正确性要求），
    /// 再按模型顺序逐个执行——相邻只读工具合并为并行组，突变工具保持严格串行。
    /// 每个结果经 process_single_tool_call 三级瀑布分类处理。
    pub(super) async fn execute_tool_calls(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        workspace: &Path,
        on_event: &Channel<AgentEvent>,
        response: LlmResponse,
        allowed_tool_names: &HashSet<String>,
        tool_context: &ToolContext,
        cancel_rx: &watch::Receiver<bool>,
        request_provider: &OpenAiCompatProvider,
        usage_tracker: &mut crate::agent::common::UsageTracker,
    ) -> Result<RunLoopToolOutcome> {
        // Persist the assistant tool-call message before executing tools. The LLM protocol expects
        // later tool results to answer a concrete assistant tool_call_id, so this write is part of
        // protocol correctness rather than UI bookkeeping.
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

        let assistant_message = persist_tool_calls_message(
            db,
            workspace_id,
            &response.content,
            &tool_calls_payload,
            &response.thinking_content,
            Some(response.thinking_elapsed_ms),
        )
        .await?;
        let mut llm_messages = Vec::new();
        if let Some(message) = assistant_message.to_llm_message() {
            llm_messages.push(message);
        }

        // ProtocolBatchState enforces batch-level rules such as one dispatch/continue/exit path per
        // active agent. Without it, a single model response could propose conflicting subprocess
        // state transitions.
        let mut protocol_state =
            ProtocolBatchState::new(self.active_subprocesses_for_workspace(workspace_id));
        let mut protocol_actions = Vec::new();
        let mut final_message: Option<String> = None;
        let mut saw_retryable_tool_error = false;

        // 按模型给出的顺序执行工具，但把相邻的只读工具合并为一个并行组。
        // 突变工具（写文件、执行命令等）保持严格串行，确保文件/进程状态可预测。
        let mut tool_call_index = 0usize;
        'outer: while tool_call_index < tool_calls.len() {
            if cancellation_requested(cancel_rx) {
                break;
            }

            let readonly_end =
                common::readonly_tool_run_end(&self.tools, workspace, &tool_calls, tool_call_index);
            let ready_tool_results = if readonly_end.saturating_sub(tool_call_index) >= 2 {
                let run = &tool_calls[tool_call_index..readonly_end];
                let results = self
                    .execute_parallel_readonly_tools(
                        db,
                        workspace_id,
                        run,
                        tool_context,
                        on_event,
                        allowed_tool_names,
                    )
                    .await?;
                let items = run
                    .iter()
                    .cloned()
                    .zip(results)
                    .map(|(tool_call, (result, run_id))| ExecutedToolCall {
                        tool_call,
                        result,
                        run_id,
                    })
                    .collect::<Vec<_>>();
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
                let run_id = self
                    .create_and_start_tool_run(db, workspace_id, workspace, on_event, &tool_call)
                    .await?;
                let is_sub_agent_call = tool_call.name == "call_sub_agent";
                // 子智能体调用有独立的 token 计量。暂停父级用量统计，
                // 避免嵌套用量被重复计入主 run 的总量。
                let result = if is_sub_agent_call {
                    with_usage_paused(usage_tracker, workspace_id, on_event, || async {
                        ToolRuntime::execute_tool(
                            &self.tools,
                            workspace,
                            allowed_tool_names,
                            &tool_call,
                            tool_context,
                        )
                        .await
                    })
                    .await
                } else {
                    ToolRuntime::execute_tool(
                        &self.tools,
                        workspace,
                        allowed_tool_names,
                        &tool_call,
                        tool_context,
                    )
                    .await
                };
                vec![ExecutedToolCall {
                    tool_call,
                    result,
                    run_id: Some(run_id),
                }]
            };

            for executed in ready_tool_results {
                if cancellation_requested(cancel_rx) {
                    break 'outer;
                }
                let tool_call = executed.tool_call;
                let result = executed.result;
                let run_id = executed.run_id;
                let result_text = result.output_for_llm();
                let result_metadata_json = result.run_metadata_json();

                // 子智能体失败会升级为父循环的致命错误。若继续执行，主 Agent 会基于
                // 一个不完整的委派任务结果进行推理，这是不可接受的。
                if let Some(message) = sub_agent_failure_message(&result_text) {
                    if let Some(run_id) = &run_id {
                        self.finish_tool_run(
                            db,
                            on_event,
                            run_id,
                            "fatal_error",
                            None,
                            None,
                            Some("sub_agent_failure"),
                            Some(message),
                            None,
                            result_metadata_json.as_deref(),
                        )
                        .await?;
                    }
                    anyhow::bail!("{}", message);
                }

                match self
                    .process_single_tool_call(
                        db,
                        workspace_id,
                        on_event,
                        &tool_call,
                        &mut protocol_state,
                        &mut llm_messages,
                    )
                    .await?
                {
                    SingleToolDisposition::Handled => {
                        if let Some(run_id) = &run_id {
                            self.finish_tool_run(
                                db,
                                on_event,
                                run_id,
                                "succeeded",
                                Some("raw"),
                                None,
                                None,
                                None,
                                None,
                                result_metadata_json.as_deref(),
                            )
                            .await?;
                        }
                    }
                    SingleToolDisposition::HandledWithRetry => {
                        if let Some(run_id) = &run_id {
                            self.finish_tool_run(
                                db,
                                on_event,
                                run_id,
                                "recoverable_error",
                                Some("raw"),
                                None,
                                Some("retryable_tool_error"),
                                Some(&result_text),
                                None,
                                result_metadata_json.as_deref(),
                            )
                            .await?;
                        }
                        saw_retryable_tool_error = true;
                    }
                    SingleToolDisposition::ProtocolAction(action) => {
                        if let Some(run_id) = &run_id {
                            self.finish_tool_run(
                                db,
                                on_event,
                                run_id,
                                "succeeded",
                                Some("raw"),
                                None,
                                None,
                                None,
                                Some(protocol_action_kind(&action)),
                                result_metadata_json.as_deref(),
                            )
                            .await?;
                        }
                        protocol_actions.push(action);
                    }
                    SingleToolDisposition::NeedsSummary => {
                        // 普通工具可能返回超大或带产物的 payload。喂回模型前先压缩，
                        // 但原始结果仍可通过 DB/UI 查到。
                        if matches!(result.status, ToolStatus::RecoverableError)
                            || is_retryable_tool_error(&tool_call.name, &result_text)
                        {
                            let retry_message = self
                                .emit_tool_retry_feedback(
                                    db,
                                    workspace_id,
                                    on_event,
                                    &tool_call,
                                    &result_text,
                                )
                                .await?;
                            if let Some(run_id) = &run_id {
                                self.finish_tool_run(
                                    db,
                                    on_event,
                                    run_id,
                                    "recoverable_error",
                                    retry_message.tool_result_mode.as_deref(),
                                    Some(&retry_message.id),
                                    Some("retryable_tool_error"),
                                    Some(&result_text),
                                    None,
                                    result_metadata_json.as_deref(),
                                )
                                .await?;
                            }
                            saw_retryable_tool_error = true;
                            if let Some(message) = retry_message.to_llm_message() {
                                llm_messages.push(message);
                            }
                            continue;
                        }

                        let summary_model = self.summary_model();
                        let summary_provider = self.summary_provider(request_provider);
                        let tool_message = common::persist_tool_result_with_compression(
                            db,
                            workspace_id,
                            on_event,
                            &tool_call,
                            &result_text,
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
                        if let Some(run_id) = &run_id {
                            self.finish_tool_run(
                                db,
                                on_event,
                                run_id,
                                result.status.as_run_status(),
                                tool_message.tool_result_mode.as_deref(),
                                Some(&tool_message.id),
                                result.status.error_kind(),
                                result.status.error_kind().map(|_| result_text.as_str()),
                                result.action.as_ref().map(ToolAction::kind),
                                result_metadata_json.as_deref(),
                            )
                            .await?;
                        }

                        if let Some(message) = tool_message.to_llm_message() {
                            llm_messages.push(message);
                        }

                        if let Some(ToolAction::FinalMessage { content }) = &result.action {
                            final_message = Some(content.clone());
                        } else if tool_call.name == "message" {
                            final_message = extract_message_content(&tool_call.arguments);
                        }
                    }
                }
            }
        }

        Ok(RunLoopToolOutcome {
            saw_retryable_tool_error,
            final_message,
            protocol_actions,
            llm_messages,
        })
    }

    /// 两级优先瀑布分类单个工具调用：
    ///   优先级 1 — 协议动作（dispatch / continue / exit 子进程），不在本地执行，
    ///               只发出 UI/子进程命令并通常以等待消息结束本轮。
    ///   优先级 2 — 普通工具，由调用方负责持久化/压缩并决定是否需要下一轮迭代。
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn process_single_tool_call(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        on_event: &Channel<AgentEvent>,
        tool_call: &RequestedToolCall,
        protocol_state: &mut ProtocolBatchState,
        llm_messages: &mut Vec<ChatMessage>,
    ) -> Result<SingleToolDisposition> {
        // Priority 1: protocol actions do not execute locally. They emit UI/subprocess commands and
        // then usually end this turn with a waiting message.
        match self
            .plan_protocol_action(db, workspace_id, tool_call, protocol_state)
            .await
        {
            Ok(Some(action)) => {
                if let ProtocolToolAction::Exit { agent, .. } = &action {
                    self.mark_agent_exit_requested(workspace_id, agent.slug());
                }
                let message = self
                    .emit_protocol_action(db, workspace_id, on_event, tool_call, &action)
                    .await?;
                if let Some(message) = message.to_llm_message() {
                    llm_messages.push(message);
                }
                return Ok(SingleToolDisposition::ProtocolAction(action));
            }
            Ok(None) => {} // not a protocol action; treat it as a normal executable tool
            Err(error) => {
                let is_retryable = is_retryable_tool_error(&tool_call.name, &error);
                let message = self
                    .handle_tool_call_error(db, workspace_id, on_event, tool_call, &error)
                    .await?;
                if let Some(message) = message.to_llm_message() {
                    llm_messages.push(message);
                }
                return Ok(if is_retryable {
                    SingleToolDisposition::HandledWithRetry
                } else {
                    SingleToolDisposition::Handled
                });
            }
        }

        // Priority 2: not protocol. The caller will persist/compress the actual
        // ToolResult and decide whether another LLM iteration is needed.
        Ok(SingleToolDisposition::NeedsSummary)
    }

    pub(super) async fn handle_tool_call_error(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        on_event: &Channel<AgentEvent>,
        tool_call: &RequestedToolCall,
        error: &str,
    ) -> Result<DispatcherMessageRecord> {
        if is_retryable_tool_error(&tool_call.name, error) {
            self.emit_tool_retry_feedback(db, workspace_id, on_event, tool_call, error)
                .await
        } else {
            self.emit_tool_error(db, workspace_id, on_event, tool_call, error)
                .await
        }
    }

    /// 根据 execute_tool_calls 的结果决定本轮循环是否结束：
    ///   - 出现可重试工具错误 ⇒ 不收口，让模型再修正一轮。
    ///   - 有协议动作 ⇒ 输出"等待子任务"消息并收口（需子进程回流）。
    ///   - 有 final_message ⇒ 输出最终答复并收口。
    ///   - 以上都不是 ⇒ 返回 None，循环继续。
    pub(super) async fn resolve_loop_outcome(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        on_event: &Channel<AgentEvent>,
        outcome: RunLoopToolOutcome,
        usage_tracker: &crate::agent::common::UsageTracker,
    ) -> Result<Option<DispatcherMessageRecord>> {
        if outcome.saw_retryable_tool_error {
            return Ok(None);
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
        db: &DispatcherDb,
        workspace_id: &str,
        tool_calls: &[RequestedToolCall],
        tool_context: &ToolContext,
        on_event: &Channel<AgentEvent>,
        allowed_tool_names: &HashSet<String>,
    ) -> Result<Vec<(ToolResult, Option<String>)>> {
        let mut run_ids = Vec::with_capacity(tool_calls.len());
        for tool_call in tool_calls {
            // Even readonly tools get run records before parallel execution so the UI can display
            // deterministic start events instead of a burst of unordered completions.
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
            let run_id = self
                .create_and_start_tool_run(
                    db,
                    workspace_id,
                    &tool_context.workspace,
                    on_event,
                    tool_call,
                )
                .await?;
            run_ids.push(Some(run_id));
        }

        let results = join_all(tool_calls.iter().map(|tool_call| async move {
            ToolRuntime::execute_tool(
                &self.tools,
                &tool_context.workspace,
                allowed_tool_names,
                tool_call,
                tool_context,
            )
            .await
        }))
        .await;

        Ok(results.into_iter().zip(run_ids).collect())
    }

    async fn create_and_start_tool_run(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        workspace: &Path,
        on_event: &Channel<AgentEvent>,
        tool_call: &RequestedToolCall,
    ) -> Result<String> {
        ToolRuntime::create_and_start_tool_run(
            db,
            &self.tools,
            workspace_id,
            workspace,
            on_event,
            tool_call,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn finish_tool_run(
        &self,
        db: &DispatcherDb,
        on_event: &Channel<AgentEvent>,
        run_id: &str,
        status: &str,
        result_mode: Option<&str>,
        message_id: Option<&str>,
        error_kind: Option<&str>,
        error_message: Option<&str>,
        action_kind: Option<&str>,
        metadata_json: Option<&str>,
    ) -> Result<()> {
        ToolRuntime::finish_tool_run(
            db,
            on_event,
            run_id,
            ToolRunFinishUpdate {
                status,
                result_mode,
                message_id,
                error_kind,
                error_message,
                action_kind,
                metadata_json,
            },
        )
        .await
    }

    pub(super) async fn emit_tool_retry_feedback(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        on_event: &Channel<AgentEvent>,
        tool_call: &RequestedToolCall,
        error: &str,
    ) -> Result<DispatcherMessageRecord> {
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
        let message = db
            .add_visible_tool_result_async(
                workspace_id,
                display_text,
                &context_payload,
                Some(&tool_call.id),
                Some(&tool_call.name),
                Some("raw"),
                &[],
            )
            .await?;
        Ok(message)
    }

    pub(super) async fn emit_tool_error(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        on_event: &Channel<AgentEvent>,
        tool_call: &RequestedToolCall,
        error: &str,
    ) -> Result<DispatcherMessageRecord> {
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
        let message = db
            .add_visible_message_with_tools_async(
                workspace_id,
                "tool",
                error,
                Some(&tool_call.id),
                Some(&tool_call.name),
                Some("raw"),
                None,
            )
            .await?;
        Ok(message)
    }
}

fn protocol_action_kind(action: &ProtocolToolAction) -> &'static str {
    match action {
        ProtocolToolAction::Dispatch { .. } => "dispatch_sub_agent",
        ProtocolToolAction::Continue { .. } => "continue_sub_agent",
        ProtocolToolAction::Exit { .. } => "exit_sub_agent",
    }
}
