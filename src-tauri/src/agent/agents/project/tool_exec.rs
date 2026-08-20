use std::path::Path;

use anyhow::Result;
use tauri::ipc::Channel;
use tokio::sync::watch;

use crate::agent::common::{
    self, build_args_map, build_tool_calls_payload, cancellation_requested,
    persist_tool_calls_message,
};
use crate::agent::db::{DispatcherDb, DispatcherMessageRecord, DispatcherSessionTokenUsageSource};
use crate::agent::llm::{ChatMessage, LlmResponse, OpenAiCompatProvider, RequestedToolCall};
use crate::agent::run_loop::core::{LoopProtocolAction, RunLoopToolOutcome};
use crate::agent::run_loop::AgentEvent;
use crate::agent::tools::{
    CapabilitySet, ToolAction, ToolContext, ToolResult, ToolRunFinishUpdate, ToolRuntime,
    ToolStatus,
};

use super::graph_submit::SubmitGraphInterception;
use super::helpers::{
    self, build_tool_retry_context, emit, extract_message_content, is_retryable_tool_error,
    record_run_token_usage,
};
use super::OrchestratorAgent;

#[derive(Default)]
struct PersistedProjectToolOutcome {
    retryable: bool,
    final_message: Option<String>,
    fatal_message: Option<String>,
}

// ─── Tool execution impl ──────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
impl OrchestratorAgent {
    /// 执行一批 tool_calls：先持久化 assistant 工具调用消息（协议正确性要求），
    /// 再按模型顺序逐个执行——相邻只读工具合并为并行组，其余严格串行。
    /// `submit_graph` 不在本地执行，而是拦截为「校验 → 落库 → 广播」的协议动作。
    ///
    /// workspace 单一来源（审查项 G8-23）：统一使用 `tool_context.workspace`
    /// （执行入口已 canonicalize），不再接受平行的 workspace 参数，
    /// 避免只读分组判定与实际执行/落库基于不同路径而漂移。
    pub(super) async fn execute_tool_calls(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        on_event: &Channel<AgentEvent>,
        response: LlmResponse,
        direct_capabilities: &CapabilitySet,
        runtime_capabilities: &CapabilitySet,
        tool_context: &ToolContext,
        cancel_rx: &watch::Receiver<bool>,
        request_provider: &OpenAiCompatProvider,
        usage_tracker: &mut crate::agent::common::UsageTracker,
    ) -> Result<RunLoopToolOutcome> {
        if response.tool_calls.len() > crate::agent::tools::MAX_TOOL_CALLS_PER_BATCH {
            anyhow::bail!(
                "模型单轮返回 {} 个工具调用，超过运行时上限 {}；已在持久化或执行前拒绝。",
                response.tool_calls.len(),
                crate::agent::tools::MAX_TOOL_CALLS_PER_BATCH
            );
        }
        let workspace = tool_context.workspace.as_path();
        // Persist the assistant tool-call message before executing tools. The LLM protocol expects
        // later tool results to answer a concrete assistant tool_call_id, so this write is part of
        // protocol correctness rather than UI bookkeeping.
        let tool_calls = response.tool_calls.clone();
        let tool_calls_payload = build_tool_calls_payload(&tool_calls, &self.tools)?;
        let args_map = build_args_map(&tool_calls, &self.tools)?;

        for tc in &tool_calls_payload {
            emit(
                on_event,
                AgentEvent::ToolPlanned {
                    tool_call_id: tc.id.clone(),
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
        // 用量持久化任务句柄收集：收口处统一 await（审查项 G8-01/G8-03），
        // 用量不再 fire-and-forget 静默丢失。
        let mut usage_persist_handles: Vec<tokio::task::JoinHandle<()>> = Vec::new();

        // 项目编排器的模型可见面只有 ToolProgram 与控制面工具，顶层严格串行；
        // 数据面并行只能在经过静态验证和有界调度的 ToolProgram 内发生。
        let mut tool_call_index = 0usize;
        while tool_call_index < tool_calls.len() {
            if cancellation_requested(cancel_rx) {
                break;
            }

            let mut tool_call = tool_calls[tool_call_index].clone();
            tool_call_index += 1;
            let tool_args_json = args_map
                .get(&tool_call.id)
                .cloned()
                .unwrap_or_else(|| "{}".to_string());
            emit(
                on_event,
                AgentEvent::ToolStarted {
                    tool_call_id: tool_call.id.clone(),
                    name: tool_call.name.clone(),
                    arguments: tool_args_json,
                },
            );
            let run_id = self
                .create_and_start_tool_run(db, workspace_id, workspace, on_event, &tool_call)
                .await?;

            // 控制面协议不能借“宿主拦截”绕过模型可见能力和权威 JSON Schema。
            // 先按本轮 direct grant 授权，再走与 Broker 同源的 default+校验路径；
            // 只有合法的 effective arguments 才能触发落图/报告等副作用。
            let prepared_arguments = if !direct_capabilities.contains(&tool_call.name) {
                Err(ToolResult::recoverable_error(format!(
                    "错误：禁止调用工具 '{}'；该控制面能力未授予本轮模型。",
                    tool_call.name
                )))
            } else {
                self.tools.prepare_control_arguments(
                    workspace,
                    &tool_call.name,
                    &tool_call.arguments,
                )
            };

            // 控制面/运行时协议由编排器拦截；壳工具自身永不直接执行。
            let (result, graph_action) = match prepared_arguments {
                Err(result) => (result, None),
                Ok(arguments) => {
                    tool_call.arguments = arguments;
                    if tool_call.name == "run_tool_program" {
                        (
                            self.intercept_tool_program(
                                db,
                                workspace_id,
                                on_event,
                                &tool_call,
                                &run_id,
                                runtime_capabilities,
                                tool_context,
                                cancel_rx,
                            )
                            .await,
                            None,
                        )
                    } else if tool_call.name == "submit_graph" {
                        match self
                            .intercept_submit_graph(
                                db,
                                workspace_id,
                                &tool_call,
                                graph_submitted_this_batch,
                            )
                            .await
                        {
                            Ok(SubmitGraphInterception::Submitted {
                                display_text,
                                action,
                            }) => {
                                graph_submitted_this_batch = true;
                                (ToolResult::success_text(display_text), Some(action))
                            }
                            Ok(SubmitGraphInterception::Rejected { error }) => {
                                (ToolResult::recoverable_error(error), None)
                            }
                            Err(error) => (
                                ToolResult::fatal_error(format!(
                                    "提交执行图协议处理失败：{error:#}"
                                )),
                                None,
                            ),
                        }
                    } else if tool_call.name == "graph_plan_report" {
                        // graph_plan_report 协议拦截：返回最近运行的紧凑报告，不收口本轮，
                        // 供模型决定答复用户或提交 inheritsFrom 修复图（反思闭环）。
                        match self
                            .intercept_graph_plan_report(db, workspace_id, &tool_call)
                            .await
                        {
                            Ok(report) => (ToolResult::from_text(report), None),
                            Err(error) => (
                                ToolResult::recoverable_error(format!(
                                    "读取执行图运行报告失败：{error:#}"
                                )),
                                None,
                            ),
                        }
                    } else {
                        (
                            ToolRuntime::execute_tool_with_cancellation(
                                &self.tools,
                                workspace,
                                direct_capabilities,
                                &tool_call,
                                tool_context,
                                cancel_rx.clone(),
                            )
                            .await,
                            None,
                        )
                    }
                }
            };

            // 取消只阻止下一次尚未开始的调用。当前结果已经执行完成（控制面甚至
            // 可能已提交图计划），必须先落 tool message、收尾 run 并保留协议动作。
            let persisted = self
                .persist_project_tool_result(
                    db,
                    workspace_id,
                    workspace,
                    on_event,
                    &tool_call,
                    result,
                    graph_action,
                    &run_id,
                    request_provider,
                    usage_tracker,
                    &mut usage_persist_handles,
                    &mut llm_messages,
                    &mut protocol_actions,
                )
                .await;
            let persisted = match persisted {
                Ok(persisted) => persisted,
                Err(error) => {
                    self.finish_started_run_after_error(
                        db,
                        workspace_id,
                        on_event,
                        &run_id,
                        &error,
                    )
                    .await;
                    return Err(error);
                }
            };
            saw_retryable_tool_error |= persisted.retryable;
            if persisted.final_message.is_some() {
                final_message = persisted.final_message;
            }
            if let Some(message) = persisted.fatal_message {
                anyhow::bail!(message);
            }
        }

        // 用量持久化收口（审查项 G8-01/G8-03）：逐一 await 持久化任务，
        // 应用退出/DB 瞬时故障时用量不再静默丢失；任务内失败已记录持久化警告，
        // 任务本身异常退出（panic）在此补充告警。
        for handle in usage_persist_handles {
            if let Err(error) = handle.await {
                helpers::log_warning(&format!("用量持久化任务异常退出：{error}"));
            }
        }

        Ok(RunLoopToolOutcome {
            saw_retryable_tool_error,
            final_message,
            protocol_actions,
            llm_messages,
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn persist_project_tool_result(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        workspace: &std::path::Path,
        on_event: &Channel<AgentEvent>,
        tool_call: &RequestedToolCall,
        result: ToolResult,
        graph_action: Option<LoopProtocolAction>,
        run_id: &str,
        request_provider: &OpenAiCompatProvider,
        usage_tracker: &mut crate::agent::common::UsageTracker,
        usage_persist_handles: &mut Vec<tokio::task::JoinHandle<()>>,
        llm_messages: &mut Vec<ChatMessage>,
        protocol_actions: &mut Vec<LoopProtocolAction>,
    ) -> Result<PersistedProjectToolOutcome> {
        let result_text = result.output_for_llm();
        let result_metadata_json = result.run_metadata_json();

        if let Some(action) = graph_action {
            let tool_message = self
                .emit_tool_result_message(db, workspace_id, on_event, tool_call, &result_text)
                .await?;
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
            if let Some(message) = tool_message.to_llm_message() {
                llm_messages.push(message);
            }
            protocol_actions.push(action);
            return Ok(PersistedProjectToolOutcome::default());
        }

        if matches!(result.status, ToolStatus::RecoverableError)
            || is_retryable_tool_error(&tool_call.name, &result_text)
        {
            let retry_message = self
                .emit_tool_retry_feedback(db, workspace_id, on_event, tool_call, &result_text)
                .await?;
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
            if let Some(message) = retry_message.to_llm_message() {
                llm_messages.push(message);
            }
            return Ok(PersistedProjectToolOutcome {
                retryable: true,
                ..Default::default()
            });
        }

        // 普通工具可能返回超大 payload。喂回模型前先压缩，原始结果保留为产物。
        let summary_model = self.summary_model();
        let summary_provider = self.summary_provider(request_provider);
        let tool_message = common::persist_tool_result_with_compression(
            db,
            workspace_id,
            on_event,
            tool_call,
            &self.tools,
            workspace,
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
                    usage_persist_handles,
                );
            },
        )
        .await?;
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
        if let Some(message) = tool_message.to_llm_message() {
            llm_messages.push(message);
        }

        let final_message = if let Some(ToolAction::FinalMessage { content }) = &result.action {
            Some(content.clone())
        } else if tool_call.name == "message" {
            extract_message_content(&tool_call.arguments)
        } else {
            None
        };
        let fatal_message =
            (result.status == ToolStatus::FatalError).then_some(result_text.clone());
        Ok(PersistedProjectToolOutcome {
            final_message,
            fatal_message,
            ..Default::default()
        })
    }

    async fn finish_started_run_after_error(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        on_event: &Channel<AgentEvent>,
        run_id: &str,
        error: &anyhow::Error,
    ) {
        let message = format!("错误：项目工具结果处理失败：{error:#}");
        if let Err(finish_error) = self
            .finish_tool_run(
                db,
                on_event,
                run_id,
                "internal_error",
                None,
                None,
                Some("internal"),
                Some(&message),
                None,
                None,
            )
            .await
        {
            helpers::log_warning(&format!(
                "项目工具 run 收尾失败（run_id={run_id}）：{finish_error:#}"
            ));
        }
        // 若 tool message 已落库但树绑定失败，优先补挂；若消息根本没生成，则
        // 删除未绑定树，避免 message_id=NULL 的 child runs/artifacts 绕过清理。
        match db.load_tool_run_async(run_id).await {
            Ok(run) => {
                if let Some(message_id) = run.message_id {
                    if let Err(attach_error) = db
                        .attach_tool_run_tree_message_async(run_id, &message_id)
                        .await
                    {
                        helpers::log_warning(&format!(
                            "项目工具运行树补挂失败（run_id={run_id}）：{attach_error:#}"
                        ));
                    }
                } else if let Err(delete_error) = db
                    .delete_unattached_tool_run_tree_async(workspace_id, run_id)
                    .await
                {
                    helpers::log_warning(&format!(
                        "项目工具未绑定运行树清理失败（run_id={run_id}）：{delete_error:#}"
                    ));
                }
            }
            Err(load_error) => helpers::log_warning(&format!(
                "项目工具运行树补偿读取失败（run_id={run_id}）：{load_error:#}"
            )),
        }
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
        let arguments_json = common::serialize_tool_arguments(
            &tool_call.name,
            &self
                .tools
                .effective_args(&tool_call.name, &tool_call.arguments),
        )?;
        emit(
            on_event,
            AgentEvent::ToolFinished {
                tool_call_id: tool_call.id.clone(),
                name: tool_call.name.clone(),
                arguments: arguments_json,
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
        let arguments_json = common::serialize_tool_arguments(
            &tool_call.name,
            &self
                .tools
                .effective_args(&tool_call.name, &tool_call.arguments),
        )?;
        emit(
            on_event,
            AgentEvent::ToolFinished {
                tool_call_id: tool_call.id.clone(),
                name: tool_call.name.clone(),
                arguments: arguments_json,
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
