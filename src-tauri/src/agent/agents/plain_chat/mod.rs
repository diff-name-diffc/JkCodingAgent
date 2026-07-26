use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use parking_lot::Mutex;
use tauri::{ipc::Channel, AppHandle};
use tokio::sync::watch;

use crate::agent::common::{
    self, build_args_map, build_tool_calls_payload, cancellation_requested,
    persist_assistant_message, persist_tool_calls_message, persist_tool_result_with_compression,
    select_provider_for_messages, stream_llm_response, with_usage_paused, LlmStreamOutcome,
    UsageTracker,
};
use crate::agent::config::DispatcherAgentConfig;
use crate::agent::db::{
    AgentContext, AhaSettingsV2, ChatCategoryAgentConfig, DispatcherDb, DispatcherMessageRecord,
    DispatcherSessionTokenUsageSource,
};
use crate::agent::llm::{
    ChatMessage, LlmResponse, OpenAiCompatProvider, RequestedToolCall, ToolDefinition,
};
use crate::agent::run_loop::agent_loop::AgentLoop;
use crate::agent::run_loop::core::{
    AgentRunAdapter, AgentRunRequest, RunLoopAgent, RunLoopContext, RunLoopIteration,
    RunLoopToolOutcome, RunPromptState, RuntimeAgentKind,
};
use crate::agent::run_loop::AgentEvent;
use crate::agent::sub_agent::{tool::sub_agent_failure_message, SubAgentManager};
use crate::agent::tools::{
    ToolAction, ToolContext, ToolRegistry, ToolResult, ToolRunFinishUpdate, ToolRuntime,
};
use crate::project::mcp::{ensure_project_mcp_file, ProjectMcpRegistry};
use crate::shared::truncate_for_display;
use crate::ssh_tool::SshSessionManager;

const SUB_AGENT_TOOL_NAMES: [&str; 2] = ["list_sub_agents", "call_sub_agent"];

pub struct PlainChatAgent {
    config: DispatcherAgentConfig,
    provider: Mutex<OpenAiCompatProvider>,
    system_prompt: Mutex<String>,
    vision_provider: Mutex<Option<OpenAiCompatProvider>>,
    summary_model: Mutex<String>,
    summary_api_key: Mutex<String>,
    summary_api_base: Mutex<String>,
    app_handle: Option<AppHandle>,
    tools: Arc<ToolRegistry>,
    allowed_tools: Mutex<Vec<String>>,
    category_context: Mutex<Option<(String, String)>>,
    project_mcp_registry: ProjectMcpRegistry,
    sub_agent_manager: Option<Arc<SubAgentManager>>,
}

struct ExecutedPlainTool {
    tool_call: RequestedToolCall,
    result: ToolResult,
    run_id: String,
}

impl PlainChatAgent {
    pub fn new(
        config: DispatcherAgentConfig,
        project_mcp_registry: ProjectMcpRegistry,
        ssh_manager: SshSessionManager,
        sub_agent_manager: Option<Arc<SubAgentManager>>,
    ) -> Self {
        let provider = OpenAiCompatProvider::new(
            config.api_key.clone(),
            config.api_base.clone(),
            config.model.clone(),
            config.max_tokens,
            config.temperature,
        );

        let mut registry =
            ToolRegistry::plain_chat_tools(project_mcp_registry.clone(), ssh_manager.clone());
        if let Some(manager) = &sub_agent_manager {
            registry.add_tool(Box::new(crate::agent::sub_agent::SubAgentTool::new(
                Arc::clone(manager),
            )));
            registry.add_tool(Box::new(crate::agent::sub_agent::ListSubAgentsTool::new(
                Arc::clone(manager),
            )));
        }

        Self {
            config,
            provider: Mutex::new(provider),
            system_prompt: Mutex::new(
                crate::agent::config::DEFAULT_PLAIN_CHAT_SYSTEM_PROMPT.to_string(),
            ),
            vision_provider: Mutex::new(None),
            summary_model: Mutex::new(crate::agent::config::DEFAULT_SUMMARY_MODEL.to_string()),
            summary_api_key: Mutex::new(String::new()),
            summary_api_base: Mutex::new(String::new()),
            app_handle: None,
            tools: Arc::new(registry),
            allowed_tools: Mutex::new(Vec::new()),
            category_context: Mutex::new(None),
            project_mcp_registry,
            sub_agent_manager,
        }
    }

    pub fn with_app_handle(mut self, app_handle: AppHandle) -> Self {
        self.app_handle = Some(app_handle);
        self
    }

    pub fn apply_settings_v2(&self, settings: &AhaSettingsV2, context: AgentContext) {
        let ctx_config = match context {
            AgentContext::Project => &settings.project,
            AgentContext::Chat => &settings.chat,
        };
        let shared = &settings.shared;

        let active_chat = ctx_config
            .chat_model_configs
            .iter()
            .find(|c| c.active)
            .or_else(|| ctx_config.chat_model_configs.first());
        let active_summary = ctx_config
            .summary_model_configs
            .iter()
            .find(|c| c.active)
            .or_else(|| ctx_config.summary_model_configs.first());
        let active_vision = shared
            .vision_model_configs
            .iter()
            .find(|c| c.active)
            .or_else(|| shared.vision_model_configs.first());

        if let Some(chat) = active_chat {
            if !chat.system_prompt.trim().is_empty() {
                *self.system_prompt.lock() = chat.system_prompt.trim().to_string();
            }
            let mut provider = self.provider.lock();
            *provider = OpenAiCompatProvider::new(
                if chat.api_key.is_empty() {
                    self.config.api_key.clone()
                } else {
                    chat.api_key.clone()
                },
                if chat.url.is_empty() {
                    self.config.api_base.clone()
                } else {
                    chat.url.clone()
                },
                if chat.model.is_empty() {
                    self.config.model.clone()
                } else {
                    chat.model.clone()
                },
                self.config.max_tokens,
                self.config.temperature,
            );
        }
        // 视觉模型切换必须使用设置中视觉用途的完整配置（url/apiKey/model，
        // 默认取第一个 active，否则第一个条目），url/apiKey 为空时回退聊天
        // 主模型的凭据。只换模型名会把视觉模型名打到聊天网关，报 unknown provider。
        *self.vision_provider.lock() = active_vision
            .filter(|v| !v.model.trim().is_empty())
            .map(|v| {
                let fallback = self.provider.lock();
                OpenAiCompatProvider::new(
                    if v.api_key.trim().is_empty() {
                        fallback.api_key().to_string()
                    } else {
                        v.api_key.trim().to_string()
                    },
                    if v.url.trim().is_empty() {
                        fallback.api_base().to_string()
                    } else {
                        v.url.trim().to_string()
                    },
                    v.model.trim().to_string(),
                    self.config.max_tokens,
                    self.config.temperature,
                )
            });
        if let Some(smc) = active_summary {
            if !smc.model.trim().is_empty() {
                *self.summary_model.lock() = smc.model.trim().to_string();
            }
            if !smc.api_key.trim().is_empty() {
                *self.summary_api_key.lock() = smc.api_key.trim().to_string();
            }
            if !smc.url.trim().is_empty() {
                *self.summary_api_base.lock() = smc.url.trim().to_string();
            }
        }
        *self.allowed_tools.lock() = ctx_config.allowed_tools.clone();
    }

    pub fn apply_category_config(&self, config: &ChatCategoryAgentConfig) {
        *self.allowed_tools.lock() = config.allowed_tools.clone();
        *self.system_prompt.lock() = config.system_prompt.clone();
        *self.category_context.lock() =
            Some((config.category_id.clone(), config.category_name.clone()));
    }

    fn summary_model(&self) -> String {
        self.summary_model.lock().clone()
    }

    fn summary_provider(&self, fallback: &OpenAiCompatProvider) -> OpenAiCompatProvider {
        let api_key = {
            let key = self.summary_api_key.lock().clone();
            if key.is_empty() {
                fallback.api_key().to_string()
            } else {
                key
            }
        };
        let api_base = {
            let base = self.summary_api_base.lock().clone();
            if base.is_empty() {
                fallback.api_base().to_string()
            } else {
                base
            }
        };
        OpenAiCompatProvider::new(
            api_key,
            api_base,
            self.summary_model.lock().clone(),
            self.config.max_tokens,
            self.config.temperature,
        )
    }

    async fn execute_all_tools(
        &self,
        db: &DispatcherDb,
        tool_calls: &[RequestedToolCall],
        args_map: &std::collections::HashMap<String, String>,
        tool_context: &ToolContext,
        allowed_tool_names: &std::collections::HashSet<String>,
        on_event: &Channel<AgentEvent>,
        cancel_rx: &watch::Receiver<bool>,
        usage_tracker: &mut UsageTracker,
        workspace_id: &str,
    ) -> Result<Vec<ExecutedPlainTool>> {
        let readonly_end =
            common::readonly_tool_run_end(&self.tools, &tool_context.workspace, tool_calls, 0);

        if readonly_end >= 2 {
            let readonly_run = &tool_calls[..readonly_end];
            for tool_call in readonly_run {
                common::emit(
                    on_event,
                    AgentEvent::ToolStarted {
                        tool_call_id: Some(tool_call.id.clone()),
                        name: tool_call.name.clone(),
                        arguments: args_map
                            .get(&tool_call.id)
                            .cloned()
                            .unwrap_or_else(|| "{}".to_string()),
                    },
                );
            }

            let mut results = Vec::with_capacity(tool_calls.len());

            let mut run_ids = Vec::with_capacity(readonly_run.len());
            for tool_call in readonly_run {
                let run_id = self
                    .create_and_start_tool_run(db, workspace_id, tool_context, on_event, tool_call)
                    .await?;
                run_ids.push(run_id);
            }

            let readonly_results: Vec<ToolResult> =
                futures::future::join_all(readonly_run.iter().map(|tool_call| async move {
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

            for ((tool_call, result), run_id) in
                readonly_run.iter().zip(readonly_results).zip(run_ids)
            {
                if cancellation_requested(cancel_rx) {
                    return Ok(results);
                }
                let result_text = result.output_for_llm();
                let result_metadata_json = result.run_metadata_json();
                if let Some(message) = sub_agent_failure_message(&result_text) {
                    self.finish_tool_run(
                        db,
                        on_event,
                        &run_id,
                        "fatal_error",
                        None,
                        None,
                        Some("sub_agent_failure"),
                        Some(message),
                        None,
                        result_metadata_json.as_deref(),
                    )
                    .await?;
                    anyhow::bail!("{}", message);
                }
                results.push(ExecutedPlainTool {
                    tool_call: tool_call.clone(),
                    result,
                    run_id,
                });
            }

            let remaining = &tool_calls[readonly_end..];
            for tool_call in remaining {
                if cancellation_requested(cancel_rx) {
                    break;
                }
                common::emit(
                    on_event,
                    AgentEvent::ToolStarted {
                        tool_call_id: Some(tool_call.id.clone()),
                        name: tool_call.name.clone(),
                        arguments: args_map
                            .get(&tool_call.id)
                            .cloned()
                            .unwrap_or_else(|| "{}".to_string()),
                    },
                );
                let run_id = self
                    .create_and_start_tool_run(db, workspace_id, tool_context, on_event, tool_call)
                    .await?;
                let result = self
                    .execute_single_tool_with_usage(
                        tool_call,
                        tool_context,
                        allowed_tool_names,
                        on_event,
                        usage_tracker,
                        workspace_id,
                    )
                    .await;
                let result_text = result.output_for_llm();
                let result_metadata_json = result.run_metadata_json();
                if let Some(message) = sub_agent_failure_message(&result_text) {
                    self.finish_tool_run(
                        db,
                        on_event,
                        &run_id,
                        "fatal_error",
                        None,
                        None,
                        Some("sub_agent_failure"),
                        Some(message),
                        None,
                        result_metadata_json.as_deref(),
                    )
                    .await?;
                    anyhow::bail!("{}", message);
                }
                results.push(ExecutedPlainTool {
                    tool_call: tool_call.clone(),
                    result,
                    run_id,
                });
            }

            Ok(results)
        } else {
            let mut results = Vec::with_capacity(tool_calls.len());
            for tool_call in tool_calls {
                if cancellation_requested(cancel_rx) {
                    break;
                }
                common::emit(
                    on_event,
                    AgentEvent::ToolStarted {
                        tool_call_id: Some(tool_call.id.clone()),
                        name: tool_call.name.clone(),
                        arguments: args_map
                            .get(&tool_call.id)
                            .cloned()
                            .unwrap_or_else(|| "{}".to_string()),
                    },
                );
                let run_id = self
                    .create_and_start_tool_run(db, workspace_id, tool_context, on_event, tool_call)
                    .await?;
                let result = self
                    .execute_single_tool_with_usage(
                        tool_call,
                        tool_context,
                        allowed_tool_names,
                        on_event,
                        usage_tracker,
                        workspace_id,
                    )
                    .await;
                let result_text = result.output_for_llm();
                let result_metadata_json = result.run_metadata_json();
                if let Some(message) = sub_agent_failure_message(&result_text) {
                    self.finish_tool_run(
                        db,
                        on_event,
                        &run_id,
                        "fatal_error",
                        None,
                        None,
                        Some("sub_agent_failure"),
                        Some(message),
                        None,
                        result_metadata_json.as_deref(),
                    )
                    .await?;
                    anyhow::bail!("{}", message);
                }
                results.push(ExecutedPlainTool {
                    tool_call: tool_call.clone(),
                    result,
                    run_id,
                });
            }
            Ok(results)
        }
    }

    async fn create_and_start_tool_run(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        tool_context: &ToolContext,
        on_event: &Channel<AgentEvent>,
        tool_call: &RequestedToolCall,
    ) -> Result<String> {
        ToolRuntime::create_and_start_tool_run(
            db,
            &self.tools,
            workspace_id,
            &tool_context.workspace,
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

    /// 执行单个工具。`call_sub_agent` 期间暂停主 Agent 的用量计时，
    /// 避免子 Agent 耗时稀释主 Agent 的 token 生成速度。
    async fn execute_single_tool_with_usage(
        &self,
        tool_call: &RequestedToolCall,
        tool_context: &ToolContext,
        allowed_tool_names: &std::collections::HashSet<String>,
        on_event: &Channel<AgentEvent>,
        usage_tracker: &mut UsageTracker,
        workspace_id: &str,
    ) -> ToolResult {
        let is_sub_agent_call = tool_call.name == "call_sub_agent";

        if is_sub_agent_call {
            with_usage_paused(usage_tracker, workspace_id, on_event, || async {
                ToolRuntime::execute_tool(
                    &self.tools,
                    &tool_context.workspace,
                    allowed_tool_names,
                    tool_call,
                    tool_context,
                )
                .await
            })
            .await
        } else {
            ToolRuntime::execute_tool(
                &self.tools,
                &tool_context.workspace,
                allowed_tool_names,
                tool_call,
                tool_context,
            )
            .await
        }
    }

    async fn browser_workspace(&self) -> Result<PathBuf> {
        let workspace = self.config.root_dir.join("plain-chat-browser");
        let config_dir = workspace.join(".jkcodingagent");
        let workspace_for_init = workspace.clone();
        tokio::task::spawn_blocking(move || {
            fs::create_dir_all(&config_dir)
                .with_context(|| format!("create {}", config_dir.display()))?;
            ensure_project_mcp_file(&workspace_for_init.to_string_lossy())
                .map_err(anyhow::Error::msg)
        })
        .await
        .map_err(|error| {
            anyhow::anyhow!("create plain chat browser workspace panicked: {error}")
        })??;
        Ok(workspace)
    }

    async fn build_tool_context(
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
        ToolContext {
            workspace_id: workspace_id.to_string(),
            workspace: workspace.to_path_buf(),
            session_title,
            user_task,
            ssh_review,
            exec_timeout_secs: self.config.exec_timeout_secs,
            restrict_to_workspace: true,
            extra_allowed_dirs: dirs::home_dir()
                .map(|home| vec![home.join(".jkcodingagent")])
                .unwrap_or_default(),
            app_handle: self.app_handle.clone(),
            llm_provider: Some(provider.clone()),
            vision_model: self
                .vision_provider
                .lock()
                .as_ref()
                .map(|p| p.model().to_string())
                .unwrap_or_default(),
            image_model_url: String::new(),
            image_model_api_key: String::new(),
            image_model: String::new(),
            image_edit_model: String::new(),
            sub_agent_tool_registry: Some(Arc::clone(&self.tools)),
            current_sub_agent_id: None,
            current_sub_agent_name: None,
            current_tool_call_id: None,
            sub_agent_parent_tool_call_id: None,
            sub_agent_trace_events: None,
        }
    }

    fn build_effective_system_prompt(&self, workspace_id: &str) -> String {
        let mut prompt = {
            let configured = self.system_prompt.lock().trim().to_string();
            if configured.is_empty() {
                crate::agent::config::DEFAULT_PLAIN_CHAT_SYSTEM_PROMPT.to_string()
            } else {
                configured
            }
        };
        if let Some((category_id, category_name)) = self.category_context.lock().clone() {
            prompt.push_str(&format!(
                "\n\n## 当前会话分类\n\n- 分类：{}\n- 分类 ID：{}",
                category_name, category_id
            ));
        }
        prompt.push_str(&format!(
            "\n\n## 系统时间\n\n当前本地时间：{}",
            crate::agent::prompt::current_local_time()
        ));
        if self.should_expose_sub_agent_tools(workspace_id) {
            if let Some(manager) = &self.sub_agent_manager {
                if let Ok(agents) = manager.get_enabled_for_session(workspace_id) {
                    if !agents.is_empty() {
                        prompt.push_str("\n\n## 当前可用子智能体\n\n");
                        prompt.push_str("以下是当前会话已启用的子智能体，你可以直接调用：\n\n");
                        for agent in &agents {
                            prompt.push_str(&format!(
                                "- **{}** (`{}`): {}\n",
                                agent.agent_name, agent.agent_id, agent.description
                            ));
                        }
                        prompt.push_str("\n使用方式：调用 call_sub_agent(agent_id, task) 来让子智能体处理特定任务。\n");
                    }
                }
            }
        }
        prompt
    }

    fn should_expose_sub_agent_tools(&self, workspace_id: &str) -> bool {
        if self.is_tool_allowed_by_config("call_sub_agent")
            || self.is_tool_allowed_by_config("list_sub_agents")
        {
            return true;
        }

        self.session_has_enabled_sub_agents(workspace_id)
    }

    fn is_tool_allowed_by_config(&self, tool_name: &str) -> bool {
        let configured = self.allowed_tools.lock();
        is_tool_allowed_by_config(&configured, tool_name)
    }

    fn session_has_enabled_sub_agents(&self, workspace_id: &str) -> bool {
        self.sub_agent_manager
            .as_ref()
            .and_then(|manager| manager.get_enabled_for_session(workspace_id).ok())
            .is_some_and(|agents| !agents.is_empty())
    }

    fn build_tool_definitions(&self, workspace_id: &str, workspace: &Path) -> Vec<ToolDefinition> {
        let configured = self.allowed_tools.lock().clone();
        let mut defs = self.tools.definitions_for_workspace(
            workspace,
            Option::<std::iter::Empty<&str>>::None,
            true,
        );

        if !configured.is_empty() {
            let allowed = effective_allowed_tools_for_chat_category(
                configured,
                self.session_has_enabled_sub_agents(workspace_id),
            );
            defs.retain(|def| allowed.contains(&def.function.name));
        }
        defs
    }
}

#[async_trait]
impl RunLoopAgent for PlainChatAgent {
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
        self.build_tool_definitions(workspace_id, workspace)
    }

    fn build_iteration_messages(
        &self,
        _ctx: &RunLoopContext<'_>,
        agent_loop: &AgentLoop,
        _tool_definitions: &[ToolDefinition],
    ) -> Result<Vec<ChatMessage>> {
        Ok(agent_loop.request_messages())
    }

    fn provider_for_iteration(
        &self,
        ctx: &RunLoopContext<'_>,
        messages: &[ChatMessage],
        iteration: usize,
    ) -> Result<OpenAiCompatProvider> {
        let vision_provider = self.vision_provider.lock().clone();
        select_provider_for_messages(
            &ctx.provider,
            messages,
            vision_provider.as_ref(),
            ctx.on_event,
            iteration == 0,
        )
    }

    async fn stream_iteration_response(
        &self,
        ctx: &mut RunLoopContext<'_>,
        iteration: &RunLoopIteration,
        _iteration_index: usize,
    ) -> Result<LlmStreamOutcome> {
        stream_llm_response(
            ctx.db,
            ctx.workspace_id,
            iteration.request_provider.model(),
            DispatcherSessionTokenUsageSource::Primary,
            &mut ctx.usage_tracker,
            ctx.on_event,
            &iteration.request_provider,
            &iteration.messages,
            &iteration.tool_definitions,
            ctx.cancel_rx.clone(),
        )
        .await
    }

    async fn handle_cancelled_loop(
        &self,
        ctx: &RunLoopContext<'_>,
        partial: &str,
    ) -> Result<DispatcherMessageRecord> {
        emit_stop_and_finish(
            ctx.db,
            ctx.workspace_id,
            ctx.on_event,
            partial,
            &ctx.usage_tracker,
        )
        .await
    }

    async fn handle_no_tool_response(
        &self,
        ctx: &RunLoopContext<'_>,
        response: &LlmResponse,
    ) -> Result<DispatcherMessageRecord> {
        let content = response.content.trim().to_string();
        if content.is_empty() {
            anyhow::bail!(
                "{}",
                empty_plain_chat_response_error(
                    response,
                    &ctx.provider,
                    self.build_tool_definitions(ctx.workspace_id, ctx.workspace)
                        .len(),
                )
            );
        }
        let usage_stats = ctx.usage_tracker.snapshot();
        let reply =
            persist_assistant_message(ctx.db, ctx.workspace_id, &content, &usage_stats).await?;
        common::emit(
            ctx.on_event,
            AgentEvent::AssistantMessage {
                message: reply.clone(),
            },
        );
        Ok(reply)
    }

    async fn execute_loop_tool_calls(
        &self,
        ctx: &mut RunLoopContext<'_>,
        iteration: &RunLoopIteration,
        tool_context: &ToolContext,
        response: LlmResponse,
    ) -> Result<RunLoopToolOutcome> {
        let tool_calls_payload = build_tool_calls_payload(&response.tool_calls, &self.tools);
        let args_map = build_args_map(&response.tool_calls, &self.tools);
        let mut llm_messages = Vec::new();

        for tc in &tool_calls_payload {
            common::emit(
                ctx.on_event,
                AgentEvent::ToolPlanned {
                    tool_call_id: Some(tc.id.clone()),
                    name: tc.function.name.clone(),
                    arguments: tc.function.arguments.clone(),
                },
            );
        }

        let assistant_message = persist_tool_calls_message(
            ctx.db,
            ctx.workspace_id,
            &response.content,
            &tool_calls_payload,
            &response.thinking_content,
            Some(response.thinking_elapsed_ms),
        )
        .await?;
        if let Some(message) = assistant_message.to_llm_message() {
            llm_messages.push(message);
        }

        let executed = self
            .execute_all_tools(
                ctx.db,
                &response.tool_calls,
                &args_map,
                tool_context,
                &iteration.allowed_tool_names,
                ctx.on_event,
                &ctx.cancel_rx,
                &mut ctx.usage_tracker,
                ctx.workspace_id,
            )
            .await?;

        let summary_provider = self.summary_provider(&iteration.request_provider);
        let summary_model = self.summary_model();
        for executed_tool in &executed {
            if cancellation_requested(&ctx.cancel_rx) {
                break;
            }
            let result_text = executed_tool.result.output_for_llm();
            let result_metadata_json = executed_tool.result.run_metadata_json();
            let tool_message = persist_tool_result_with_compression(
                ctx.db,
                ctx.workspace_id,
                ctx.on_event,
                &executed_tool.tool_call,
                &result_text,
                &summary_provider,
                &summary_model,
                |usage| {
                    ctx.usage_tracker.record(usage);
                },
            )
            .await?;
            if let Some(message) = tool_message.to_llm_message() {
                llm_messages.push(message);
            }
            self.finish_tool_run(
                ctx.db,
                ctx.on_event,
                &executed_tool.run_id,
                executed_tool.result.status.as_run_status(),
                tool_message.tool_result_mode.as_deref(),
                Some(&tool_message.id),
                executed_tool.result.status.error_kind(),
                executed_tool
                    .result
                    .status
                    .error_kind()
                    .map(|_| result_text.as_str()),
                executed_tool.result.action.as_ref().map(ToolAction::kind),
                result_metadata_json.as_deref(),
            )
            .await?;
        }

        Ok(RunLoopToolOutcome {
            saw_retryable_tool_error: false,
            final_message: None,
            protocol_actions: Vec::new(),
            llm_messages,
        })
    }

    async fn resolve_loop_outcome(
        &self,
        _ctx: &RunLoopContext<'_>,
        _outcome: RunLoopToolOutcome,
    ) -> Result<Option<DispatcherMessageRecord>> {
        Ok(None)
    }

    fn max_iterations_error(&self, kind: RuntimeAgentKind) -> String {
        match kind {
            RuntimeAgentKind::PlainChat => format!(
                "已达到最大工具迭代次数（{}），本轮聊天被终止。请检查模型是否陷入工具调用循环。",
                self.config.max_tool_iterations
            ),
            RuntimeAgentKind::Project => format!(
                "已达到最大工具迭代次数（{}），本轮执行被终止。",
                self.config.max_tool_iterations
            ),
        }
    }
}

fn is_tool_allowed_by_config(configured: &[String], tool_name: &str) -> bool {
    configured.is_empty() || configured.iter().any(|name| name == tool_name)
}

fn effective_allowed_tools_for_chat_category(
    configured: Vec<String>,
    has_enabled_sub_agents: bool,
) -> HashSet<String> {
    let mut allowed = configured.into_iter().collect::<HashSet<_>>();
    if has_enabled_sub_agents {
        allowed.extend(SUB_AGENT_TOOL_NAMES.iter().map(|name| name.to_string()));
    }
    allowed
}

#[async_trait]
impl AgentRunAdapter for PlainChatAgent {
    async fn prepare_run_workspace(&self, _request: &AgentRunRequest<'_>) -> Result<PathBuf> {
        let workspace = self.browser_workspace().await?;
        self.project_mcp_registry
            .ensure_recent(&workspace)
            .await
            .map_err(anyhow::Error::msg)
            .context("刷新聊天 MCP 状态失败")?;
        Ok(workspace)
    }

    fn provider_snapshot(&self) -> OpenAiCompatProvider {
        self.provider.lock().clone()
    }

    fn provider_missing_message(&self) -> &'static str {
        "聊天 LLM API Key 未配置。请在设置中配置，或设置 DASHSCOPE_API_KEY / OPENAI_API_KEY 环境变量。"
    }

    async fn build_run_prompt(&self, workspace_id: &str) -> Result<RunPromptState> {
        Ok(RunPromptState {
            initial_system_prompt: self.build_effective_system_prompt(workspace_id),
            project_prompt: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_sub_agents_expose_sub_agent_tools_even_when_tool_allowlist_omits_them() {
        let allowed =
            effective_allowed_tools_for_chat_category(vec!["browser_read_text".to_string()], true);

        assert!(allowed.contains("browser_read_text"));
        assert!(allowed.contains("list_sub_agents"));
        assert!(allowed.contains("call_sub_agent"));
    }

    #[test]
    fn category_without_sub_agents_keeps_tool_allowlist_exact() {
        let allowed =
            effective_allowed_tools_for_chat_category(vec!["browser_read_text".to_string()], false);

        assert!(allowed.contains("browser_read_text"));
        assert!(!allowed.contains("list_sub_agents"));
        assert!(!allowed.contains("call_sub_agent"));
    }
}

fn empty_plain_chat_response_error(
    response: &LlmResponse,
    provider: &OpenAiCompatProvider,
    tool_count: usize,
) -> String {
    let raw_response = response.raw_response.trim();
    let response_detail = if raw_response.is_empty() {
        "<空>".to_string()
    } else {
        truncate_for_display(raw_response, 4_000, "\n...[LLM 接口响应内容已截断]")
    };

    format!(
        "LLM 返回了空响应且没有工具调用，无法继续执行。\n请求摘要：model={}, tools={}\nLLM 接口响应内容：\n{}",
        provider.model(),
        tool_count,
        response_detail
    )
}

async fn emit_stop_and_finish(
    db: &DispatcherDb,
    workspace_id: &str,
    on_event: &Channel<AgentEvent>,
    partial: &str,
    usage_tracker: &UsageTracker,
) -> Result<DispatcherMessageRecord> {
    let content = build_stopped_plain_chat_reply(partial);
    let usage_stats = usage_tracker.snapshot();
    let reply = persist_assistant_message(db, workspace_id, &content, &usage_stats).await?;
    common::emit(
        on_event,
        AgentEvent::AssistantMessage {
            message: reply.clone(),
        },
    );
    Ok(reply)
}

fn build_stopped_plain_chat_reply(partial: &str) -> String {
    let trimmed = partial.trim();
    if trimmed.is_empty() {
        "⏹️ 本轮聊天已停止。当前会话上下文已保留，可稍后继续。".to_string()
    } else {
        format!(
            "{}\n\n[本轮聊天已手动停止。当前会话上下文与以上输出均已保留，可稍后继续。]",
            trimmed
        )
    }
}
