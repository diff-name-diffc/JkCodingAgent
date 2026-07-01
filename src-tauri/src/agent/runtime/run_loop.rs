use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tauri::ipc::Channel;
use tokio::sync::watch;

use super::super::common::{
    cancellation_requested, stream_llm_response, wait_for_cancellation, LlmStreamOutcome,
    UsageTracker,
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

use super::agent_loop::AgentLoop;
use super::helpers::{
    build_stopped_dispatch_reply, emit, empty_llm_response_error, record_run_token_usage,
};
use super::planning::{
    complete_checklist_dispatch, empty_checklist_state, start_checklist_dispatch,
};

use super::types::{AgentEvent, AgentTurn, DispatchFeedbackState};
use super::DispatcherAgent;

// ─── 内部类型 ───────────────────────────────────────────────────────────

/// 单次 LLM 请求的不可变快照。
///
/// 从请求发出到流式响应完成，必须使用同一份消息、provider 和工具 schema 快照。
/// 流式传输期间的状态变更（如子进程状态）会在下一轮迭代处理，不在当前请求中途生效。
pub(super) struct IterationContext {
    pub runtime_state: DispatcherSessionRuntimeState,
    pub tool_definitions: Vec<crate::agent::llm::ToolDefinition>,
    pub allowed_tool_names: HashSet<String>,
    pub messages: Vec<ChatMessage>,
    pub request_provider: OpenAiCompatProvider,
    pub debug_logger: ContextDebugLogger,
}

// ─── 主入口 ────────────────────────────────────────────────────────────────

impl DispatcherAgent {
    /// 项目模式主入口：接收用户消息并启动一轮调度。
    /// 流程概览：发 Started → 清旧 checklist → 刷新 MCP → 存用户消息 →
    /// 快照 provider/prompt → 进入 run_llm_loop（核心迭代循环）→ 发 Finished。
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
        // Started 在 fallible 代码块之外发出，保证 UI 总能离开 idle 状态；
        // 之后的任何失败都在此边界统一归一化为 AgentEvent::Failed。
        emit(
            &on_event,
            AgentEvent::Started {
                workspace_id: workspace_id.to_string(),
            },
        );
        let result: Result<AgentTurn> = async {
        // 新一轮用户对话需要一份全新的 checklist。若沿用旧 checklist，会让子进程
        // dispatch 状态看起来仍然有效，但模型实际上在解决一个全新目标。
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
        // 工具可用性是提示词契约的一部分。在用户消息进入循环前刷新 MCP 元数据，
        // 确保第一次模型请求就能看到当前的工具 schema。
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

        // provider 和静态提示词是本轮快照：设置变更只作用于下一轮 run，
        // 而本轮内部保持一致。
        let static_prompt = self.build_system_prompt().await?;
        // 初始化一次动态工具视图；run_llm_loop 会在工具调用可能改变运行态
        // （如激活的计划、子进程可用性）后刷新它。
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

    /// 子任务回流入口：子进程执行完毕后，将其结果摘要作为隐藏用户消息注入，
    /// 然后重新进入主循环，让主 Agent 据此继续推理或收口本轮调度。
    ///
    /// 这是"调度闭环"的后半段（前半段是 protocol.rs 触发 DispatchProposed）。
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
        let result: Result<AgentTurn> = async {
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

    // ─── LLM 循环 ─────────────────────────────────────────────────────────

    /// 核心迭代循环：重复"请求 LLM → 执行工具 → 判断是否收口"，直到：
    /// - 模型返回不含工具调用的最终答复（handle_no_tool_response），或
    /// - 协议动作/计划等待用户（resolve_loop_outcome 返回 Some），或
    /// - 被取消，或
    /// - 达到 max_tool_iterations 上限（视为模型陷入循环）。
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

        // ── 循环不变量初始化（仅执行一次） ────────────────────────────

        let initial_runtime_state = db.get_session_runtime_state_async(workspace_id).await?;
        let tool_definitions = initial_tool_definitions;
        let allowed_tool_names = initial_allowed_tools;

        // AgentLoop 拥有本轮内存中的 LLM 历史。它从持久化的 DB 历史起步，
        // 之后只追加本循环产生的消息，而非每次工具调用后都重新加载整段对话。
        let mut agent_loop =
            AgentLoop::new(db, workspace_id, static_prompt.static_content.clone()).await?;

        // 这些缓存只在运行态可能影响工具可见性时才替换。保持其局部性，
        // 防止半更新的设置泄漏进正在进行的 LLM 请求。
        let mut tool_defs_cache = tool_definitions;
        let mut allowed_names_cache = allowed_tool_names;
        let mut runtime_state_cache = initial_runtime_state;

        // 仅用于诊断：当前循环只记录压力告警，不会压缩历史。
        let estimated_tokens = agent_loop.estimated_tokens();
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

        // ── 主迭代循环 ─────────────────────────────────────────

        for iteration in 0..self.config.max_tool_iterations {
            if cancellation_requested(&cancel_rx) {
                return self
                    .emit_stop_and_finish(db, workspace_id, on_event, "", Some(usage_tracker))
                    .await;
            }

            // 工具调用可能改变运行态（例如预留计划、派生子进程），进而影响工具可见性。
            // 首轮之后每轮重新计算工具白名单，保证模型只看到当前合法的工具集。
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

            // 每轮从静态文本 + 最新运行态分片重新渲染系统提示；既避免磁盘读取，
            // 又保证动态指令始终是当前的。
            let prompt_snapshot = self.build_system_prompt_from_static(
                static_prompt,
                workspace_id,
                workspace,
                &tool_defs_cache,
                &runtime_state_cache,
            )?;

            // 每轮用最新动态系统提示替换首条 system 消息。跳过 AgentLoop 内的旧 system，
            // 避免过期的动态分片（如已失效的子进程状态）混入本轮请求。
            let mut messages = vec![ChatMessage::system(prompt_snapshot.rendered.clone())];
            messages.extend(agent_loop.request_messages().into_iter().skip(1));

            // 每轮仍需按消息内容选择 provider：若本轮消息含图片，则切换到视觉模型。
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

            // 从此处直到流式响应完成，请求必须使用消息、provider 和工具 schema 的稳定快照。
            // 流式传输期间的状态变更留到下一轮迭代处理，不在 token 流式输出过程中介入。
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

            // 无工具调用 ⇒ 模型已给出面向用户的最终答复，本轮循环可以收口，
            // 不再进入下一轮推理。
            if response.tool_calls.is_empty() {
                return self
                    .handle_no_tool_response(db, workspace_id, on_event, &response, usage_tracker)
                    .await;
            }

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

            // 只把本轮新产生的消息追加进 AgentLoop 的内存历史，
            // 而非全量 reload——这是 AgentLoop 抽象的性能关键点。
            for message in &outcome.llm_messages {
                agent_loop.append(message.clone());
            }

            // token 预算检查仍只是告警而非拦截。若此处频繁触发，
            // 历史压缩应在此处实现，而非下放到各个工具。
            let estimated_tokens = agent_loop.estimated_tokens();
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
