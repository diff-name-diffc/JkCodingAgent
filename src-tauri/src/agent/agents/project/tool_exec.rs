use std::collections::HashSet;
use std::path::Path;

use anyhow::Result;
use futures::future::join_all;
use tauri::ipc::Channel;
use tokio::sync::watch;

use crate::agent::common::{
    self, build_args_map, build_tool_calls_payload, cancellation_requested,
    persist_tool_calls_message,
};
use crate::agent::db::{DispatcherDb, DispatcherMessageRecord, DispatcherSessionTokenUsageSource};
use crate::agent::llm::{LlmResponse, OpenAiCompatProvider, RequestedToolCall};
use crate::agent::run_loop::core::{LoopProtocolAction, RunLoopToolOutcome};
use crate::agent::run_loop::AgentEvent;
use crate::agent::tools::{
    ToolAction, ToolContext, ToolResult, ToolRunFinishUpdate, ToolRuntime, ToolStatus,
};

use super::graph_submit::SubmitGraphInterception;
use super::helpers::{
    build_tool_retry_context, emit, extract_message_content, is_retryable_tool_error,
    record_run_token_usage,
};
use super::OrchestratorAgent;

pub(super) struct ExecutedToolCall {
    pub tool_call: RequestedToolCall,
    pub result: ToolResult,
    pub run_id: Option<String>,
    pub graph_action: Option<LoopProtocolAction>,
}

// ─── Tool execution impl ──────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
impl OrchestratorAgent {
    /// 执行一批 tool_calls：先持久化 assistant 工具调用消息（协议正确性要求），
    /// 再按模型顺序逐个执行——相邻只读工具合并为并行组，其余严格串行。
    /// `submit_graph` 不在本地执行，而是拦截为「校验 → 落库 → 广播」的协议动作。
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

        let mut protocol_actions = Vec::new();
        let mut final_message: Option<String> = None;
        let mut saw_retryable_tool_error = false;
        let mut graph_submitted_this_batch = false;

        // 按模型给出的顺序执行工具，但把相邻的只读工具合并为一个并行组。
        // message / submit_graph 非只读，保持严格串行。
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
                        graph_action: None,
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

                // submit_graph 协议拦截：真正的动作（校验/落库/广播）在这里完成，
                // 壳工具的 execute 只负责回显。
                let (result, graph_action) = if tool_call.name == "submit_graph" {
                    match self
                        .intercept_submit_graph(
                            db,
                            workspace_id,
                            &tool_call,
                            graph_submitted_this_batch,
                        )
                        .await?
                    {
                        SubmitGraphInterception::Submitted {
                            display_text,
                            action,
                        } => {
                            graph_submitted_this_batch = true;
                            (ToolResult::success_text(display_text), Some(action))
                        }
                        SubmitGraphInterception::Rejected { error } => {
                            (ToolResult::recoverable_error(error), None)
                        }
                    }
                } else if tool_call.name == "graph_plan_report" {
                    // graph_plan_report 协议拦截：返回最近运行的紧凑报告，不收口本轮，
                    // 供模型决定答复用户或提交 inheritsFrom 修复图（反思闭环）。
                    // 拦截硬错误（DB 读取失败等）转为可重试工具错误交回模型，
                    // 与 submit_graph 分支保持一致：避免以 Err 中止本轮编排，
                    // 留下永久 started 的悬挂运行记录，且让模型拿到失败反馈。
                    match self
                        .intercept_graph_plan_report(db, workspace_id, &tool_call)
                        .await
                    {
                        Ok(report) => (ToolResult::success_text(report), None),
                        Err(error) => (
                            ToolResult::recoverable_error(format!(
                                "读取执行图运行报告失败：{error:#}"
                            )),
                            None,
                        ),
                    }
                } else {
                    (
                        ToolRuntime::execute_tool(
                            &self.tools,
                            workspace,
                            allowed_tool_names,
                            &tool_call,
                            tool_context,
                        )
                        .await,
                        None,
                    )
                };
                vec![ExecutedToolCall {
                    tool_call,
                    result,
                    run_id: Some(run_id),
                    graph_action,
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

                // submit_graph 成功：按协议动作处理——持久化工具消息保证下一轮
                // LLM 请求的因果链完整，动作本身交给 resolve_loop_outcome 收口。
                if let Some(action) = executed.graph_action {
                    let tool_message = self
                        .emit_tool_result_message(
                            db,
                            workspace_id,
                            on_event,
                            &tool_call,
                            &result_text,
                        )
                        .await?;
                    if let Some(message) = tool_message.to_llm_message() {
                        llm_messages.push(message);
                    }
                    if let Some(run_id) = &run_id {
                        self.finish_tool_run(
                            db,
                            on_event,
                            run_id,
                            "succeeded",
                            Some("raw"),
                            Some(&tool_message.id),
                            None,
                            None,
                            Some("submit_graph"),
                            result_metadata_json.as_deref(),
                        )
                        .await?;
                    }
                    protocol_actions.push(action);
                    continue;
                }

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

                // 普通工具可能返回超大 payload。喂回模型前先压缩，
                // 但原始结果仍可通过 DB/UI 查到。
                let summary_model = self.summary_model();
                let summary_provider = self.summary_provider(request_provider);
                let tool_message = common::persist_tool_result_with_compression(
                    db,
                    workspace_id,
                    on_event,
                    &tool_call,
                    &self.tools,
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

        Ok(RunLoopToolOutcome {
            saw_retryable_tool_error,
            final_message,
            protocol_actions,
            llm_messages,
        })
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

    /// 持久化工具结果消息（原始文本、raw 模式），并补发 ToolFinished 事件。
    async fn emit_tool_result_message(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        on_event: &Channel<AgentEvent>,
        tool_call: &RequestedToolCall,
        display_text: &str,
    ) -> Result<DispatcherMessageRecord> {
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
            .add_visible_message_with_tools_async(
                workspace_id,
                "tool",
                display_text,
                Some(&tool_call.id),
                Some(&tool_call.name),
                Some("raw"),
                None,
            )
            .await?;
        Ok(message)
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
}
