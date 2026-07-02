use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use parking_lot::Mutex;
use tauri::{ipc::Channel, AppHandle};
use tokio::sync::watch;

use super::common::{
    self, build_args_map, build_tool_calls_payload, cancellation_requested,
    persist_assistant_message, persist_tool_calls_message, persist_tool_result_with_compression,
    select_provider_for_messages, stream_llm_response, with_usage_paused, LlmStreamOutcome,
    UsageTracker,
};
use super::config::DispatcherAgentConfig;
use super::db::{
    AgentContext, AhaSettingsV2, ChatCategoryAgentConfig, DispatcherDb, DispatcherMessageRecord,
    DispatcherSessionTokenUsageSource,
};
use super::llm::{
    ChatMessage, LlmResponse, OpenAiCompatProvider, RequestedToolCall, ToolDefinition,
};
use super::runtime::{agent_loop::AgentLoop, AgentEvent, AgentTurn};
use super::sub_agent::{tool::sub_agent_failure_message, SubAgentManager};
use super::tools::{ToolAction, ToolRegistry, ToolResult, ToolRunFinishUpdate, ToolRuntime};
use crate::project::mcp::{ensure_project_mcp_file, ProjectMcpRegistry};
use crate::shared::truncate_for_display;
use crate::ssh_tool::SshSessionManager;

const SUB_AGENT_TOOL_NAMES: [&str; 2] = ["list_sub_agents", "call_sub_agent"];

pub struct PlainChatAgent {
    config: DispatcherAgentConfig,
    provider: Mutex<OpenAiCompatProvider>,
    system_prompt: Mutex<String>,
    vision_model: Mutex<String>,
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
            registry.add_tool(Box::new(super::sub_agent::SubAgentTool::new(Arc::clone(
                manager,
            ))));
            registry.add_tool(Box::new(super::sub_agent::ListSubAgentsTool::new(
                Arc::clone(manager),
            )));
        }

        Self {
            config,
            provider: Mutex::new(provider),
            system_prompt: Mutex::new(super::config::DEFAULT_PLAIN_CHAT_SYSTEM_PROMPT.to_string()),
            vision_model: Mutex::new(String::new()),
            summary_model: Mutex::new(super::config::DEFAULT_SUMMARY_MODEL.to_string()),
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
        if let Some(v) = active_vision {
            if !v.model.trim().is_empty() {
                *self.vision_model.lock() = v.model.trim().to_string();
            }
        }
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

    pub async fn run(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        user_segments_json: String,
        on_event: Channel<AgentEvent>,
        cancel_rx: watch::Receiver<bool>,
    ) -> Result<AgentTurn> {
        common::emit(
            &on_event,
            AgentEvent::Started {
                workspace_id: workspace_id.to_string(),
            },
        );
        let result: Result<AgentTurn> = async {

        let user = db
            .add_visible_message_from_segments_async(workspace_id, "user", user_segments_json)
            .await?;
        common::emit(&on_event, AgentEvent::UserMessage { message: user });

        let provider = self.provider.lock().clone();
        if !provider.is_configured() {
            anyhow::bail!(
                "聊天 LLM API Key 未配置。请在设置中配置，或设置 DASHSCOPE_API_KEY / OPENAI_API_KEY 环境变量。"
            );
        }

        let mut usage_tracker = UsageTracker::new();
        let workspace = self.browser_workspace().await?;
        self.project_mcp_registry
            .ensure_recent(&workspace)
            .await
            .map_err(anyhow::Error::msg)
            .context("刷新聊天 MCP 状态失败")?;
        let reply = self
            .run_loop(
                db,
                workspace_id,
                &workspace,
                &on_event,
                &provider,
                cancel_rx,
                &mut usage_tracker,
            )
            .await?;

        let messages = db.list_visible_messages_async(workspace_id).await?;
        common::emit(
            &on_event,
            AgentEvent::Finished {
                messages: messages.clone(),
            },
        );
        Ok(AgentTurn { reply, messages })
        }
        .await;

        if let Err(error) = &result {
            common::emit(
                &on_event,
                AgentEvent::Failed {
                    workspace_id: workspace_id.to_string(),
                    message: error.to_string(),
                },
            );
        }

        result
    }

    /// Core agent loop: stream → execute tools → loop until no tools or cancelled.
    #[allow(clippy::too_many_arguments)]
    async fn run_loop(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        workspace: &Path,
        on_event: &Channel<AgentEvent>,
        provider: &OpenAiCompatProvider,
        cancel_rx: watch::Receiver<bool>,
        usage_tracker: &mut UsageTracker,
    ) -> Result<DispatcherMessageRecord> {
        let tool_context = self
            .build_tool_context(db, workspace_id, workspace, provider)
            .await;
        let tool_definitions = self.build_tool_definitions(workspace_id, workspace);
        let allowed_tool_names = tool_definitions
            .iter()
            .map(|t| t.function.name.clone())
            .collect::<std::collections::HashSet<_>>();
        let mut agent_loop = AgentLoop::new(
            db,
            workspace_id,
            self.build_effective_system_prompt(workspace_id),
        )
        .await?;
        let vision_model = self.vision_model.lock().clone();

        for iteration in 0..self.config.max_tool_iterations {
            if cancellation_requested(&cancel_rx) {
                return emit_stop_and_finish(db, workspace_id, on_event, "", usage_tracker).await;
            }

            let messages = agent_loop.request_messages();
            let request_provider = select_provider_for_messages(
                provider,
                &messages,
                &vision_model,
                on_event,
                iteration == 0,
            )?;

            // Stream LLM response
            let response = match stream_llm_response(
                db,
                workspace_id,
                request_provider.model(),
                DispatcherSessionTokenUsageSource::Primary,
                usage_tracker,
                on_event,
                &request_provider,
                &messages,
                &tool_definitions,
                cancel_rx.clone(),
            )
            .await?
            {
                LlmStreamOutcome::Cancelled(partial) => {
                    return emit_stop_and_finish(
                        db,
                        workspace_id,
                        on_event,
                        &partial,
                        usage_tracker,
                    )
                    .await;
                }
                LlmStreamOutcome::Response(response) => response,
            };

            // If no tool calls, persist final message and return
            if response.tool_calls.is_empty() {
                let content = response.content.trim().to_string();
                if content.is_empty() {
                    anyhow::bail!(
                        "{}",
                        empty_plain_chat_response_error(
                            &response,
                            &request_provider,
                            tool_definitions.len(),
                        )
                    );
                }
                let usage_stats = usage_tracker.snapshot();
                let reply =
                    persist_assistant_message(db, workspace_id, &content, &usage_stats).await?;
                common::emit(
                    on_event,
                    AgentEvent::AssistantMessage {
                        message: reply.clone(),
                    },
                );
                return Ok(reply);
            }

            // Persist tool_call message, emit ToolPlanned events
            let tool_calls_payload = build_tool_calls_payload(&response.tool_calls, &self.tools);
            let args_map = build_args_map(&response.tool_calls, &self.tools);
            agent_loop.append(ChatMessage {
                role: "assistant".to_string(),
                content: response.content.clone(),
                content_parts: Vec::new(),
                reasoning_content: if response.thinking_content.is_empty() {
                    None
                } else {
                    Some(response.thinking_content.clone())
                },
                tool_calls: Some(tool_calls_payload.clone()),
                tool_call_id: None,
                name: None,
            });

            for tc in &tool_calls_payload {
                common::emit(
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

            // Execute tool calls sequentially (with parallel for readonly)
            let executed = self
                .execute_all_tools(
                    db,
                    &response.tool_calls,
                    &args_map,
                    &tool_context,
                    &allowed_tool_names,
                    on_event,
                    &cancel_rx,
                    usage_tracker,
                    workspace_id,
                )
                .await?;

            let summary_provider = self.summary_provider(&request_provider);
            let summary_model = self.summary_model();
            for executed_tool in &executed {
                if cancellation_requested(&cancel_rx) {
                    break;
                }
                let result_text = executed_tool.result.output_for_llm();
                let result_metadata_json = executed_tool.result.run_metadata_json();
                let tool_message = persist_tool_result_with_compression(
                    db,
                    workspace_id,
                    on_event,
                    &executed_tool.tool_call,
                    &result_text,
                    &summary_provider,
                    &summary_model,
                    |usage| {
                        usage_tracker.record(usage);
                    },
                )
                .await?;
                if let Some(message) = tool_message.to_llm_message() {
                    agent_loop.append(message);
                }
                self.finish_tool_run(
                    db,
                    on_event,
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
        }

        anyhow::bail!(
            "已达到最大工具迭代次数（{}），本轮聊天被终止。请检查模型是否陷入工具调用循环。",
            self.config.max_tool_iterations
        )
    }

    async fn execute_all_tools(
        &self,
        db: &DispatcherDb,
        tool_calls: &[RequestedToolCall],
        args_map: &std::collections::HashMap<String, String>,
        tool_context: &super::tools::ToolContext,
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
        tool_context: &super::tools::ToolContext,
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
        tool_context: &super::tools::ToolContext,
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
    ) -> super::tools::ToolContext {
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
        super::tools::ToolContext {
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
            vision_model: self.vision_model.lock().clone(),
            image_model_url: String::new(),
            image_model_api_key: String::new(),
            image_model: String::new(),
            image_edit_model: String::new(),
            sub_agent_tool_registry: Some(Arc::clone(&self.tools)),
            current_sub_agent_id: None,
            current_sub_agent_name: None,
        }
    }

    fn build_effective_system_prompt(&self, workspace_id: &str) -> String {
        let mut prompt = {
            let configured = self.system_prompt.lock().trim().to_string();
            if configured.is_empty() {
                super::config::DEFAULT_PLAIN_CHAT_SYSTEM_PROMPT.to_string()
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
            super::prompt::current_local_time()
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
