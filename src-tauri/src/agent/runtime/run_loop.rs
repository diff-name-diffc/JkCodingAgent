use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tauri::ipc::Channel;
use tokio::sync::watch;

use super::super::common::{
    build_tool_calls_payload, cancellation_requested, stream_llm_response, wait_for_cancellation,
    LlmStreamOutcome, UsageTracker,
};
use super::super::db::{
    DispatcherDb, DispatcherMessageRecord, DispatcherSessionRuntimeState,
    DispatcherSessionTokenUsageSource, DEFAULT_CONTEXT_WINDOW_CAPACITY_TOKENS,
};
use super::super::debug::{render_json, ContextDebugLogger, DebugSection};
use super::super::llm::{ChatMessage, OpenAiCompatProvider};
use super::super::prompt::PromptBundle;
use super::super::summary::summarize_dispatch_result;
use super::super::tools::ToolContext;

use super::helpers::{
    build_stopped_dispatch_reply, emit, empty_llm_response_error, record_run_token_usage,
};
use super::planning::{
    complete_checklist_dispatch, empty_checklist_state, start_checklist_dispatch,
};

use super::types::{AgentEvent, AgentTurn, DispatchFeedbackState};
use super::DispatcherAgent;

// ─── Internal types ───────────────────────────────────────────────────────────

pub(super) struct IterationContext {
    pub runtime_state: DispatcherSessionRuntimeState,
    pub tool_definitions: Vec<crate::agent::llm::ToolDefinition>,
    pub allowed_tool_names: HashSet<String>,
    pub messages: Vec<ChatMessage>,
    pub request_provider: OpenAiCompatProvider,
    pub debug_logger: ContextDebugLogger,
}

// ─── Main entry points ────────────────────────────────────────────────────────

impl DispatcherAgent {
    #[allow(clippy::too_many_arguments)]
    pub async fn run(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        workspace_path: &str,
        user_message: &str,
        user_segments_json: Option<String>,
        enable_thinking: bool,
        on_event: Channel<AgentEvent>,
        cancel_rx: watch::Receiver<bool>,
    ) -> Result<AgentTurn> {
        emit(
            &on_event,
            AgentEvent::Started {
                workspace_id: workspace_id.to_string(),
            },
        );
        db.clear_checklist_async(workspace_id)
            .await
            .context("clear stale checklist before new dispatcher turn")?;
        emit(
            &on_event,
            AgentEvent::ChecklistPlanUpdated {
                state: empty_checklist_state(),
            },
        );

        let workspace = PathBuf::from(workspace_path);
        if !workspace.exists() {
            std::fs::create_dir_all(&workspace)
                .with_context(|| format!("create workspace {}", workspace.display()))?;
        }
        self.project_mcp_registry
            .ensure_recent(&workspace)
            .await
            .map_err(anyhow::Error::msg)
            .context("刷新项目 MCP 状态失败")?;
        let user = db
            .add_visible_message_async(workspace_id, "user", user_message, user_segments_json)
            .await?;
        emit(&on_event, AgentEvent::UserMessage { message: user });

        let provider = self.provider.lock().clone();
        if !provider.is_configured() {
            anyhow::bail!(
                "主 Agent LLM API Key 未配置。请在 Dispatcher 设置中配置，或设置 DASHSCOPE_API_KEY / OPENAI_API_KEY 环境变量。"
            );
        }
        let mut usage_tracker = UsageTracker::new();

        // Build static prompt once for the entire turn (avoids re-reading disk files)
        let static_prompt = self.build_system_prompt().await?;
        // Pre-compute initial tool definitions and cache for the loop
        let initial_runtime_state = db.get_session_runtime_state_async(workspace_id).await?;
        let initial_tool_defs =
            self.tool_definitions_for_workspace(workspace_id, &workspace, &initial_runtime_state);
        let allowed_tool_names: HashSet<String> = initial_tool_defs
            .iter()
            .map(|t| t.function.name.clone())
            .collect();

        let reply = self
            .run_llm_loop(
                db,
                workspace_id,
                &workspace,
                &on_event,
                &provider,
                enable_thinking,
                cancel_rx,
                &mut usage_tracker,
                &static_prompt,
                initial_tool_defs,
                allowed_tool_names,
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

    #[allow(clippy::too_many_arguments)]
    pub async fn continue_after_dispatch(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        workspace_path: &str,
        dispatch_result: &str,
        dispatch_state: DispatchFeedbackState,
        dispatch_id: Option<&str>,
        enable_thinking: bool,
        on_event: Channel<AgentEvent>,
        mut cancel_rx: watch::Receiver<bool>,
    ) -> Result<AgentTurn> {
        let debug_logger =
            ContextDebugLogger::new(self.context_debug_enabled(), PathBuf::from(workspace_path));
        if let Some(dispatch_id) = dispatch_id {
            let checklist = match dispatch_state {
                DispatchFeedbackState::RoundCompleted => {
                    start_checklist_dispatch(db, workspace_id, dispatch_id).await
                }
                DispatchFeedbackState::ProcessDone
                | DispatchFeedbackState::ProcessFailed
                | DispatchFeedbackState::ProcessCancelled => {
                    complete_checklist_dispatch(db, workspace_id, dispatch_id).await
                }
            }
            .map_err(anyhow::Error::msg)?;
            if let Some(checklist) = checklist {
                emit(
                    &on_event,
                    AgentEvent::ChecklistPlanUpdated { state: checklist },
                );
            }
        }
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
        let summarized_dispatch_result = match tokio::select! {
            _ = wait_for_cancellation(&mut cancel_rx) => {
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

        // Build static prompt once for the entire turn (avoids re-reading disk files)
        let static_prompt = self.build_system_prompt().await?;
        // Pre-compute initial tool definitions and cache for the loop
        let initial_runtime_state = db.get_session_runtime_state_async(workspace_id).await?;
        let initial_tool_defs =
            self.tool_definitions_for_workspace(workspace_id, &workspace, &initial_runtime_state);
        let allowed_tool_names: HashSet<String> = initial_tool_defs
            .iter()
            .map(|t| t.function.name.clone())
            .collect();

        let reply = self
            .run_llm_loop(
                db,
                workspace_id,
                &workspace,
                &on_event,
                &provider,
                enable_thinking,
                cancel_rx,
                &mut usage_tracker,
                &static_prompt,
                initial_tool_defs,
                allowed_tool_names,
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

    // ─── LLM loop ─────────────────────────────────────────────────────────────

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn run_llm_loop(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        workspace: &Path,
        on_event: &Channel<AgentEvent>,
        provider: &OpenAiCompatProvider,
        enable_thinking: bool,
        cancel_rx: watch::Receiver<bool>,
        usage_tracker: &mut UsageTracker,
        static_prompt: &PromptBundle,
        initial_tool_definitions: Vec<crate::agent::llm::ToolDefinition>,
        initial_allowed_tools: HashSet<String>,
    ) -> Result<DispatcherMessageRecord> {
        let tool_context = self
            .build_tool_context(db, workspace_id, workspace, provider)
            .await;
        let debug_logger =
            ContextDebugLogger::new(self.context_debug_enabled(), PathBuf::from(workspace));

        // ── Loop-invariant setup (executed once) ─────────────────────────────

        let initial_runtime_state = db.get_session_runtime_state_async(workspace_id).await?;
        let tool_definitions = initial_tool_definitions;
        let allowed_tool_names = initial_allowed_tools;

        // Load history messages once from DB
        let history_messages = db.load_llm_history_async(workspace_id).await?;
        let mut incremental_messages: Vec<ChatMessage> = history_messages;

        // Cache for tool definitions across iterations
        let mut tool_defs_cache = tool_definitions;
        let mut allowed_names_cache = allowed_tool_names;
        let mut runtime_state_cache = initial_runtime_state;

        // Pre-check context window budget
        let estimated_tokens = DispatcherDb::estimate_context_tokens(&incremental_messages);
        if estimated_tokens > DEFAULT_CONTEXT_WINDOW_CAPACITY_TOKENS * 8 / 10 {
            debug_logger.log(
                "上下文窗口接近上限",
                vec![
                    ("工作区".to_string(), workspace_id.to_string()),
                    ("估算tokens".to_string(), estimated_tokens.to_string()),
                    (
                        "容量".to_string(),
                        DEFAULT_CONTEXT_WINDOW_CAPACITY_TOKENS.to_string(),
                    ),
                ],
                vec![],
            );
        }

        // ── Main iteration loop ─────────────────────────────────────────────

        for iteration in 0..self.config.max_tool_iterations {
            if cancellation_requested(&cancel_rx) {
                return self
                    .emit_stop_and_finish(db, workspace_id, on_event, "", Some(usage_tracker))
                    .await;
            }

            // For iteration > 0: rebuild dynamic context with current state
            if iteration > 0 {
                runtime_state_cache = db.get_session_runtime_state_async(workspace_id).await?;
                let new_tool_defs = self.tool_definitions_for_workspace(
                    workspace_id,
                    workspace,
                    &runtime_state_cache,
                );
                allowed_names_cache = new_tool_defs
                    .iter()
                    .map(|t| t.function.name.clone())
                    .collect();
                tool_defs_cache = new_tool_defs;
            }

            // Build system prompt from cached static + fresh dynamic sections (no disk I/O)
            let prompt_snapshot = self.build_system_prompt_from_static(
                static_prompt,
                workspace_id,
                workspace,
                &tool_defs_cache,
                &runtime_state_cache,
            )?;

            // Prepend system prompt to incremental messages
            let mut messages = vec![ChatMessage::system(prompt_snapshot.rendered.clone())];
            messages.extend(incremental_messages.iter().cloned());

            // Provider selection: cache across iterations (images don't appear mid-loop)
            let request_provider = if iteration == 0 {
                self.provider_for_messages(provider, &messages, on_event, true)?
            } else {
                self.provider_for_messages(provider, &messages, on_event, false)?
            };

            let ctx = IterationContext {
                runtime_state: runtime_state_cache.clone(),
                tool_definitions: tool_defs_cache.clone(),
                allowed_tool_names: allowed_names_cache.clone(),
                messages,
                request_provider,
                debug_logger: debug_logger.clone(),
            };

            let response = match self
                .stream_llm_response_inner(
                    db,
                    workspace_id,
                    on_event,
                    &ctx.request_provider,
                    &ctx.messages,
                    &ctx.tool_definitions,
                    enable_thinking,
                    cancel_rx.clone(),
                    usage_tracker,
                    &ctx.debug_logger,
                    iteration,
                )
                .await?
            {
                LlmStreamOutcome::Cancelled(partial) => {
                    return self
                        .emit_stop_and_finish(
                            db,
                            workspace_id,
                            on_event,
                            &partial,
                            Some(usage_tracker),
                        )
                        .await;
                }
                LlmStreamOutcome::Response(r) => r,
            };

            // No tool calls → end of loop (pure text reply)
            if response.tool_calls.is_empty() {
                incremental_messages.push(ChatMessage {
                    role: "assistant".to_string(),
                    content: response.content.clone(),
                    reasoning_content: if response.thinking_content.is_empty() {
                        None
                    } else {
                        Some(response.thinking_content.clone())
                    },
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                });
                return self
                    .handle_no_tool_response(db, workspace_id, on_event, &response, usage_tracker)
                    .await;
            }

            // Build the assistant message with tool calls for in-memory tracking.
            let tool_calls_payload = build_tool_calls_payload(&response.tool_calls, &self.tools);
            incremental_messages.push(ChatMessage {
                role: "assistant".to_string(),
                content: response.content.clone(),
                reasoning_content: if response.thinking_content.is_empty() {
                    None
                } else {
                    Some(response.thinking_content.clone())
                },
                tool_calls: Some(tool_calls_payload),
                tool_call_id: None,
                name: None,
            });

            let outcome = self
                .execute_tool_calls(
                    db,
                    workspace_id,
                    workspace,
                    on_event,
                    response,
                    &ctx.runtime_state,
                    &ctx.allowed_tool_names,
                    &tool_context,
                    &cancel_rx,
                    &ctx.request_provider,
                    usage_tracker,
                )
                .await?;

            // Append new tool results from DB to incremental messages (only new ones)
            let updated_history = db.load_llm_history_async(workspace_id).await?;
            let current_len = incremental_messages.len();
            if updated_history.len() > current_len {
                incremental_messages.extend(updated_history.into_iter().skip(current_len));
            }

            // Token budget check: log warning if approaching context window limit
            let estimated_tokens = DispatcherDb::estimate_context_tokens(&incremental_messages);
            if estimated_tokens > DEFAULT_CONTEXT_WINDOW_CAPACITY_TOKENS * 8 / 10 {
                debug_logger.log(
                    "上下文窗口接近上限，考虑折叠早期消息",
                    vec![
                        ("工作区".to_string(), workspace_id.to_string()),
                        ("轮次".to_string(), (iteration + 1).to_string()),
                        ("估算tokens".to_string(), estimated_tokens.to_string()),
                    ],
                    vec![],
                );
            }

            if let Some(reply) = self
                .resolve_loop_outcome(db, workspace_id, on_event, outcome, usage_tracker)
                .await?
            {
                return Ok(reply);
            }
        }

        anyhow::bail!(
            "已达到最大工具迭代次数（{}），本轮执行被终止。请检查模型是否陷入工具调用循环或计划协议未收口。",
            self.config.max_tool_iterations
        )
    }

    // ─── Supporting methods ───────────────────────────────────────────────────

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

    #[allow(clippy::too_many_arguments)]
    async fn stream_llm_response_inner(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        on_event: &Channel<AgentEvent>,
        request_provider: &OpenAiCompatProvider,
        messages: &[ChatMessage],
        tool_definitions: &[crate::agent::llm::ToolDefinition],
        enable_thinking: bool,
        cancel_rx: watch::Receiver<bool>,
        usage_tracker: &mut UsageTracker,
        debug_logger: &ContextDebugLogger,
        iteration: usize,
    ) -> Result<LlmStreamOutcome> {
        if debug_logger.enabled() {
            let request_snapshot = request_provider.build_request_snapshot(
                messages,
                tool_definitions,
                enable_thinking,
            );
            debug_logger.log(
                "发送大模型请求",
                vec![
                    ("工作区".to_string(), workspace_id.to_string()),
                    ("轮次".to_string(), (iteration + 1).to_string()),
                    ("模型".to_string(), request_provider.model().to_string()),
                    ("消息数".to_string(), messages.len().to_string()),
                    ("工具数".to_string(), tool_definitions.len().to_string()),
                ],
                vec![DebugSection::new(
                    "实际请求",
                    render_json(&request_snapshot),
                )],
            );
        }

        let outcome = stream_llm_response(
            db,
            workspace_id,
            request_provider.model(),
            DispatcherSessionTokenUsageSource::Primary,
            usage_tracker,
            on_event,
            request_provider,
            messages,
            tool_definitions,
            enable_thinking,
            cancel_rx,
        )
        .await?;

        if debug_logger.enabled() {
            if let LlmStreamOutcome::Response(ref response) = outcome {
                let response_snapshot = request_provider.build_response_snapshot(response);
                debug_logger.log(
                    "收到大模型响应",
                    vec![
                        ("工作区".to_string(), workspace_id.to_string()),
                        ("轮次".to_string(), (iteration + 1).to_string()),
                        ("模型".to_string(), request_provider.model().to_string()),
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
        response: &super::super::llm::LlmResponse,
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
