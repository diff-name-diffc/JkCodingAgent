use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use async_trait::async_trait;
use tauri::ipc::Channel;
use tokio::sync::watch;

use super::super::common::{
    cancellation_requested, stream_llm_response, wait_for_cancellation, LlmStreamOutcome,
    UsageTracker,
};
use super::super::db::{
    DispatcherDb, DispatcherMessageRecord, DispatcherSessionTokenUsageSource,
    DEFAULT_CONTEXT_WINDOW_CAPACITY_TOKENS,
};
use super::super::debug::{render_json, ContextDebugLogger, DebugSection};
use super::super::llm::{ChatMessage, LlmResponse, OpenAiCompatProvider, ToolDefinition};
use super::super::summary::summarize_dispatch_result;
use super::super::tools::ToolContext;

use super::agent_loop::AgentLoop;
use super::helpers::{
    build_stopped_dispatch_reply, emit, empty_llm_response_error, record_run_token_usage,
};
use super::run_loop_core::{
    self, AgentRunAdapter, AgentRunRequest, RunLoopAgent, RunLoopContext, RunLoopIteration,
    RunLoopToolOutcome, RunPromptState, RuntimeAgentKind,
};
use super::types::{AgentEvent, AgentTurn, DispatchFeedbackState};
use super::DispatcherAgent;

// ─── 内部类型 ───────────────────────────────────────────────────────────

/// 子任务完成/让出控制权后，继续主调度循环的输入。
pub(crate) struct DispatcherContinueAfterDispatchRequest<'a> {
    pub db: &'a DispatcherDb,
    pub workspace_id: &'a str,
    pub workspace_path: &'a str,
    pub dispatch_result: &'a str,
    pub dispatch_state: DispatchFeedbackState,
    pub dispatch_id: Option<&'a str>,
    pub on_event: Channel<AgentEvent>,
    pub cancel_rx: watch::Receiver<bool>,
}

// ─── 子任务回流入口 ─────────────────────────────────────────────────────────

impl DispatcherAgent {
    /// 子任务回流入口：子进程执行完毕后，将其结果摘要作为隐藏用户消息注入，
    /// 然后重新进入主循环，让主 Agent 据此继续推理或收口本轮调度。
    ///
    /// 这是"调度闭环"的后半段（前半段是 protocol.rs 触发 DispatchProposed）。
    pub async fn continue_after_dispatch(
        &self,
        request: DispatcherContinueAfterDispatchRequest<'_>,
    ) -> Result<AgentTurn> {
        let DispatcherContinueAfterDispatchRequest {
            db,
            workspace_id,
            workspace_path,
            dispatch_result,
            dispatch_state,
            dispatch_id,
            on_event,
            cancel_rx,
        } = request;

        let result: Result<AgentTurn> = async {
            let debug_logger =
                ContextDebugLogger::new(self.context_debug_enabled(), PathBuf::from(workspace_path));
            let _ = dispatch_id;
            let result_msg = db
                .add_visible_message_async(
                    workspace_id,
                    "assistant",
                    dispatch_state.visible_message(),
                    None,
                )
                .await?;
            emit(
                &on_event,
                AgentEvent::AssistantMessage {
                    message: result_msg.clone(),
                },
            );

            if cancellation_requested(&cancel_rx) {
                let reply = self
                    .emit_stop_and_finish(db, workspace_id, &on_event, "", None)
                    .await?;
                let messages = db.list_visible_messages_async(workspace_id).await?;
                emit(
                    &on_event,
                    AgentEvent::Finished {
                        messages: messages.clone(),
                    },
                );
                return Ok(AgentTurn { reply, messages });
            }

            let provider = self.provider.lock().clone();
            if !provider.is_configured() {
                anyhow::bail!(
                    "主 Agent LLM API Key 未配置，无法继续处理子任务结果。请在 Dispatcher 设置中配置，或设置 DASHSCOPE_API_KEY / OPENAI_API_KEY 环境变量。"
                );
            }
            let summary_model = self.summary_model();
            let summary_provider = self.summary_provider(&provider);
            let mut usage_tracker = UsageTracker::new();
            let mut summary_cancel_rx = cancel_rx.clone();
            let summarized_dispatch_result = match tokio::select! {
                _ = wait_for_cancellation(&mut summary_cancel_rx) => {
                    let reply = self.emit_stop_and_finish(
                        db,
                        workspace_id,
                        &on_event,
                        "",
                        None,
                    ).await?;
                    let messages = db.list_visible_messages_async(workspace_id).await?;
                    emit(
                        &on_event,
                        AgentEvent::Finished {
                            messages: messages.clone(),
                        },
                    );
                    return Ok(AgentTurn { reply, messages });
                }
                result = summarize_dispatch_result(&summary_provider, &summary_model, dispatch_result, |usage| {
                    record_run_token_usage(
                        db,
                        workspace_id,
                        &summary_model,
                        DispatcherSessionTokenUsageSource::Summary,
                        usage,
                        &mut usage_tracker,
                        &on_event,
                    );
                }) => result
            } {
                Ok(summary) => summary,
                Err(error) => {
                    debug_logger.log(
                        "子任务结果摘要失败",
                        vec![
                            ("工作区".to_string(), workspace_id.to_string()),
                            (
                                "子任务状态".to_string(),
                                format!("{dispatch_state:?}").to_lowercase(),
                            ),
                        ],
                        vec![
                            DebugSection::new("摘要调用", error.debug_context().to_string()),
                            DebugSection::new("失败原因", error.message().to_string()),
                        ],
                    );
                    anyhow::bail!(
                        "子任务结果摘要失败，summary_model={}：{}",
                        summary_model,
                        error.message()
                    );
                }
            };

            let hidden_message = format!(
                "{}\n\n{}",
                dispatch_state.hidden_prefix(),
                summarized_dispatch_result
            );

            // 子进程输出在上方以简短可见状态展示，但作为隐藏的 user 上下文重新进入主 LLM。
            // 这样既保持聊天界面可读，又保留下一步推理所需的因果关系。
            db.add_hidden_message_async(
                workspace_id,
                "user",
                &hidden_message,
                None,
                None,
                None,
                None,
            )
            .await?;

            let workspace = PathBuf::from(workspace_path);
            self.project_mcp_registry
                .ensure_recent(&workspace)
                .await
                .map_err(anyhow::Error::msg)
                .context("刷新项目 MCP 状态失败")?;

            // 与 run() 相同的本轮快照策略：静态提示词只加载一次，动态运行态在循环内刷新。
            let static_prompt = self.build_system_prompt().await?;
            let initial_system_prompt = static_prompt.static_content.clone();
            let reply = run_loop_core::run_loop(
                self,
                RunLoopContext {
                    kind: RuntimeAgentKind::Project,
                    db,
                    workspace_id,
                    workspace: &workspace,
                    on_event: &on_event,
                    provider,
                    cancel_rx,
                    usage_tracker,
                    initial_system_prompt,
                    project_prompt: Some(static_prompt),
                },
            )
                .await?;

            let messages = db.list_visible_messages_async(workspace_id).await?;
            emit(
                &on_event,
                AgentEvent::Finished {
                    messages: messages.clone(),
                },
            );
            Ok(AgentTurn { reply, messages })
        }
        .await;

        if let Err(error) = &result {
            emit(
                &on_event,
                AgentEvent::Failed {
                    workspace_id: workspace_id.to_string(),
                    message: error.to_string(),
                },
            );
        }

        result
    }

    // ─── 辅助方法 ───────────────────────────────────────────────────────────

    pub(super) async fn build_tool_context(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        workspace: &Path,
        provider: &OpenAiCompatProvider,
    ) -> ToolContext {
        let session_title = db
            .get_session_title_async(workspace_id)
            .await
            .unwrap_or_else(|_| "untitled".to_string());
        let user_task = db
            .get_latest_user_message_content_async(workspace_id)
            .await
            .ok()
            .flatten();
        let ssh_review = db
            .get_settings_v2()
            .ok()
            .and_then(|settings| settings.review.is_configured().then_some(settings.review));
        let ms = self.models.lock().snapshot();
        ToolContext {
            workspace_id: workspace_id.to_string(),
            workspace: workspace.to_path_buf(),
            session_title,
            user_task,
            ssh_review,
            exec_timeout_secs: self.config.exec_timeout_secs,
            restrict_to_workspace: self.config.restrict_to_workspace,
            extra_allowed_dirs: dirs::home_dir()
                .map(|h| vec![h.join(".jkcodingagent")])
                .unwrap_or_default(),
            app_handle: self.app_handle.clone(),
            llm_provider: Some(provider.clone()),
            vision_model: ms.vision_model,
            image_model_url: ms.image_model_url,
            image_model_api_key: ms.image_model_api_key,
            image_model: ms.image_model,
            image_edit_model: ms.image_edit_model,
            sub_agent_tool_registry: Some(std::sync::Arc::clone(&self.tools)),
            current_sub_agent_id: None,
            current_sub_agent_name: None,
        }
    }

    async fn stream_llm_response_inner(
        &self,
        ctx: &mut RunLoopContext<'_>,
        iteration_ctx: &RunLoopIteration,
        iteration: usize,
    ) -> Result<LlmStreamOutcome> {
        let debug_logger =
            ContextDebugLogger::new(self.context_debug_enabled(), PathBuf::from(ctx.workspace));
        if debug_logger.enabled() {
            let request_snapshot = iteration_ctx
                .request_provider
                .build_request_snapshot(&iteration_ctx.messages, &iteration_ctx.tool_definitions);
            debug_logger.log(
                "发送大模型请求",
                vec![
                    ("工作区".to_string(), ctx.workspace_id.to_string()),
                    ("轮次".to_string(), (iteration + 1).to_string()),
                    (
                        "模型".to_string(),
                        iteration_ctx.request_provider.model().to_string(),
                    ),
                    (
                        "消息数".to_string(),
                        iteration_ctx.messages.len().to_string(),
                    ),
                    (
                        "工具数".to_string(),
                        iteration_ctx.tool_definitions.len().to_string(),
                    ),
                ],
                vec![DebugSection::new(
                    "实际请求",
                    render_json(&request_snapshot),
                )],
            );
        }

        let outcome = stream_llm_response(
            ctx.db,
            ctx.workspace_id,
            iteration_ctx.request_provider.model(),
            DispatcherSessionTokenUsageSource::Primary,
            &mut ctx.usage_tracker,
            ctx.on_event,
            &iteration_ctx.request_provider,
            &iteration_ctx.messages,
            &iteration_ctx.tool_definitions,
            ctx.cancel_rx.clone(),
        )
        .await?;

        if debug_logger.enabled() {
            if let LlmStreamOutcome::Response(ref response) = outcome {
                let response_snapshot = iteration_ctx
                    .request_provider
                    .build_response_snapshot(response);
                debug_logger.log(
                    "收到大模型响应",
                    vec![
                        ("工作区".to_string(), ctx.workspace_id.to_string()),
                        ("轮次".to_string(), (iteration + 1).to_string()),
                        (
                            "模型".to_string(),
                            iteration_ctx.request_provider.model().to_string(),
                        ),
                        ("状态码".to_string(), response.status_code.to_string()),
                        (
                            "工具调用数".to_string(),
                            response.tool_calls.len().to_string(),
                        ),
                    ],
                    vec![DebugSection::new(
                        "实际响应",
                        render_json(&response_snapshot),
                    )],
                );
            }
        }

        Ok(outcome)
    }

    pub(super) async fn handle_no_tool_response(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        on_event: &Channel<AgentEvent>,
        response: &LlmResponse,
        usage_tracker: &UsageTracker,
    ) -> Result<DispatcherMessageRecord> {
        let content = response.content.trim().to_string();
        if content.is_empty() {
            anyhow::bail!("{}", empty_llm_response_error(response));
        }
        let usage_stats = usage_tracker.snapshot();
        let reply = db
            .add_visible_message_with_usage_and_thinking_async(
                workspace_id,
                "assistant",
                &content,
                &usage_stats,
                Some(&response.thinking_content),
                response.thinking_elapsed_ms,
            )
            .await?;
        emit(
            on_event,
            AgentEvent::AssistantMessage {
                message: reply.clone(),
            },
        );
        Ok(reply)
    }

    pub(super) async fn emit_stop_and_finish(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        on_event: &Channel<AgentEvent>,
        partial: &str,
        usage_tracker: Option<&UsageTracker>,
    ) -> Result<DispatcherMessageRecord> {
        let content = build_stopped_dispatch_reply(partial);
        let usage_stats = usage_tracker.map(UsageTracker::snapshot);
        let reply = if let Some(usage_stats) = usage_stats.as_ref() {
            db.add_visible_message_with_usage_async(
                workspace_id,
                "assistant",
                &content,
                usage_stats,
            )
            .await?
        } else {
            db.add_visible_message_async(workspace_id, "assistant", &content, None)
                .await?
        };
        emit(
            on_event,
            AgentEvent::AssistantMessage {
                message: reply.clone(),
            },
        );
        Ok(reply)
    }
}

#[async_trait]
impl AgentRunAdapter for DispatcherAgent {
    async fn prepare_run_workspace(&self, request: &AgentRunRequest<'_>) -> Result<PathBuf> {
        let workspace_path = request
            .workspace_path
            .ok_or_else(|| anyhow::anyhow!("项目 Agent 启动缺少 workspace_path"))?;
        let workspace = PathBuf::from(workspace_path);
        let workspace_for_create = workspace.clone();
        tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(&workspace_for_create)
                .with_context(|| format!("create workspace {}", workspace_for_create.display()))
        })
        .await
        .map_err(|error| anyhow::anyhow!("create workspace task failed: {error}"))??;

        self.project_mcp_registry
            .ensure_recent(&workspace)
            .await
            .map_err(anyhow::Error::msg)
            .context("刷新项目 MCP 状态失败")?;
        Ok(workspace)
    }

    fn provider_snapshot(&self) -> OpenAiCompatProvider {
        self.provider.lock().clone()
    }

    fn provider_missing_message(&self) -> &'static str {
        "主 Agent LLM API Key 未配置。请在 Dispatcher 设置中配置，或设置 DASHSCOPE_API_KEY / OPENAI_API_KEY 环境变量。"
    }

    async fn build_run_prompt(&self, _workspace_id: &str) -> Result<RunPromptState> {
        let static_prompt = self.build_system_prompt().await?;
        Ok(RunPromptState {
            initial_system_prompt: static_prompt.static_content.clone(),
            project_prompt: Some(static_prompt),
        })
    }
}

#[async_trait]
impl RunLoopAgent for DispatcherAgent {
    async fn build_loop_tool_context(&self, ctx: &RunLoopContext<'_>) -> ToolContext {
        self.build_tool_context(ctx.db, ctx.workspace_id, ctx.workspace, &ctx.provider)
            .await
    }

    fn max_tool_iterations(&self) -> usize {
        self.config.max_tool_iterations
    }

    fn tool_definitions_for_loop(
        &self,
        workspace_id: &str,
        workspace: &Path,
    ) -> Vec<ToolDefinition> {
        self.tool_definitions_for_workspace(workspace_id, workspace)
    }

    fn build_iteration_messages(
        &self,
        ctx: &RunLoopContext<'_>,
        agent_loop: &AgentLoop,
        tool_definitions: &[ToolDefinition],
    ) -> Result<Vec<ChatMessage>> {
        let Some(static_prompt) = ctx.project_prompt.as_ref() else {
            anyhow::bail!("Project run_loop 缺少静态提示词状态");
        };
        let debug_logger =
            ContextDebugLogger::new(self.context_debug_enabled(), PathBuf::from(ctx.workspace));
        let estimated_tokens = agent_loop.estimated_tokens();
        if estimated_tokens > DEFAULT_CONTEXT_WINDOW_CAPACITY_TOKENS * 8 / 10 {
            debug_logger.log(
                "上下文窗口接近上限",
                vec![
                    ("工作区".to_string(), ctx.workspace_id.to_string()),
                    ("估算tokens".to_string(), estimated_tokens.to_string()),
                    (
                        "容量".to_string(),
                        DEFAULT_CONTEXT_WINDOW_CAPACITY_TOKENS.to_string(),
                    ),
                ],
                vec![],
            );
        }

        let prompt_snapshot = self.build_system_prompt_from_static(
            static_prompt,
            ctx.workspace_id,
            ctx.workspace,
            tool_definitions,
        )?;
        let mut messages = vec![ChatMessage::system(prompt_snapshot.rendered)];
        messages.extend(agent_loop.request_messages().into_iter().skip(1));
        Ok(messages)
    }

    fn provider_for_iteration(
        &self,
        ctx: &RunLoopContext<'_>,
        messages: &[ChatMessage],
        iteration: usize,
    ) -> Result<OpenAiCompatProvider> {
        self.provider_for_messages(&ctx.provider, messages, ctx.on_event, iteration == 0)
    }

    async fn stream_iteration_response(
        &self,
        ctx: &mut RunLoopContext<'_>,
        iteration: &RunLoopIteration,
        iteration_index: usize,
    ) -> Result<LlmStreamOutcome> {
        self.stream_llm_response_inner(ctx, iteration, iteration_index)
            .await
    }

    async fn handle_cancelled_loop(
        &self,
        ctx: &RunLoopContext<'_>,
        partial: &str,
    ) -> Result<DispatcherMessageRecord> {
        self.emit_stop_and_finish(
            ctx.db,
            ctx.workspace_id,
            ctx.on_event,
            partial,
            Some(&ctx.usage_tracker),
        )
        .await
    }

    async fn handle_no_tool_response(
        &self,
        ctx: &RunLoopContext<'_>,
        response: &LlmResponse,
    ) -> Result<DispatcherMessageRecord> {
        DispatcherAgent::handle_no_tool_response(
            self,
            ctx.db,
            ctx.workspace_id,
            ctx.on_event,
            response,
            &ctx.usage_tracker,
        )
        .await
    }

    async fn execute_loop_tool_calls(
        &self,
        ctx: &mut RunLoopContext<'_>,
        iteration: &RunLoopIteration,
        tool_context: &ToolContext,
        response: LlmResponse,
    ) -> Result<RunLoopToolOutcome> {
        self.execute_tool_calls(
            ctx.db,
            ctx.workspace_id,
            ctx.workspace,
            ctx.on_event,
            response,
            &iteration.allowed_tool_names,
            tool_context,
            &ctx.cancel_rx,
            &iteration.request_provider,
            &mut ctx.usage_tracker,
        )
        .await
    }

    async fn resolve_loop_outcome(
        &self,
        ctx: &RunLoopContext<'_>,
        outcome: RunLoopToolOutcome,
    ) -> Result<Option<DispatcherMessageRecord>> {
        DispatcherAgent::resolve_loop_outcome(
            self,
            ctx.db,
            ctx.workspace_id,
            ctx.on_event,
            outcome,
            &ctx.usage_tracker,
        )
        .await
    }

    fn max_iterations_error(&self, kind: RuntimeAgentKind) -> String {
        match kind {
            RuntimeAgentKind::Project => format!(
                "已达到最大工具迭代次数（{}），本轮执行被终止。请检查模型是否陷入工具调用循环或计划协议未收口。",
                self.config.max_tool_iterations
            ),
            RuntimeAgentKind::PlainChat => format!(
                "已达到最大工具迭代次数（{}），本轮聊天被终止。请检查模型是否陷入工具调用循环。",
                self.config.max_tool_iterations
            ),
        }
    }
}
