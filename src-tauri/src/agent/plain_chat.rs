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
    AgentContext, AhaSettingsV2, DispatcherDb, DispatcherMessageRecord,
    DispatcherSessionTokenUsageSource, DispatcherSettingsRecord,
};
use super::llm::{
    ChatMessage, LlmResponse, OpenAiCompatProvider, RequestedToolCall, ToolDefinition,
};
use super::runtime::{AgentEvent, AgentTurn};
use super::sub_agent::{tool::sub_agent_failure_message, SubAgentManager};
use super::tools::ToolRegistry;
use crate::project::mcp::{ensure_project_mcp_file, ProjectMcpRegistry};
use crate::shared::truncate_for_display;
use crate::ssh_tool::SshSessionManager;

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
    project_mcp_registry: ProjectMcpRegistry,
    sub_agent_manager: Option<Arc<SubAgentManager>>,
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
            project_mcp_registry,
            sub_agent_manager,
        }
    }

    pub fn with_app_handle(mut self, app_handle: AppHandle) -> Self {
        self.app_handle = Some(app_handle);
        self
    }

    pub fn apply_settings(&self, settings: &DispatcherSettingsRecord) {
        {
            let mut provider = self.provider.lock();
            *provider = OpenAiCompatProvider::new(
                if settings.api_key.is_empty() {
                    self.config.api_key.clone()
                } else {
                    settings.api_key.clone()
                },
                if settings.api_base.is_empty() {
                    self.config.api_base.clone()
                } else {
                    settings.api_base.clone()
                },
                if settings.model.is_empty() {
                    self.config.model.clone()
                } else {
                    settings.model.clone()
                },
                self.config.max_tokens,
                self.config.temperature,
            );
        }
        if !settings.vision_model.trim().is_empty() {
            *self.vision_model.lock() = settings.vision_model.trim().to_string();
        }
        if !settings.summary_model.trim().is_empty() {
            *self.summary_model.lock() = settings.summary_model.trim().to_string();
        }
        {
            let cfg = &settings.summary_model_config;
            if !cfg.api_key.trim().is_empty() {
                *self.summary_api_key.lock() = cfg.api_key.trim().to_string();
            }
            if !cfg.url.trim().is_empty() {
                *self.summary_api_base.lock() = cfg.url.trim().to_string();
            }
        }
        *self.allowed_tools.lock() = settings.allowed_tools.clone();
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
        user_message: &str,
        user_segments_json: Option<String>,
        enable_thinking: bool,
        on_event: Channel<AgentEvent>,
        cancel_rx: watch::Receiver<bool>,
    ) -> Result<AgentTurn> {
        common::emit(
            &on_event,
            AgentEvent::Started {
                workspace_id: workspace_id.to_string(),
            },
        );

        let user = db
            .add_visible_message_async(workspace_id, "user", user_message, user_segments_json)
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
                enable_thinking,
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

    /// Core agent loop: stream → execute tools → loop until no tools or cancelled.
    #[allow(clippy::too_many_arguments)]
    async fn run_loop(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        workspace: &Path,
        on_event: &Channel<AgentEvent>,
        provider: &OpenAiCompatProvider,
        enable_thinking: bool,
        cancel_rx: watch::Receiver<bool>,
        usage_tracker: &mut UsageTracker,
    ) -> Result<DispatcherMessageRecord> {
        let tool_context = self
            .build_tool_context(db, workspace_id, workspace, provider)
            .await;
        let tool_definitions = self.build_tool_definitions(workspace);
        let allowed_tool_names = tool_definitions
            .iter()
            .map(|t| t.function.name.clone())
            .collect::<std::collections::HashSet<_>>();

        for iteration in 0..self.config.max_tool_iterations {
            if cancellation_requested(&cancel_rx) {
                return emit_stop_and_finish(db, workspace_id, on_event, "", usage_tracker).await;
            }

            let history_messages = db.load_llm_history_async(workspace_id).await?;
            let vision_model = self.vision_model.lock().clone();
            let request_provider = select_provider_for_messages(
                provider,
                &history_messages,
                &vision_model,
                on_event,
                iteration == 0,
            )?;

            let system_prompt = self.build_effective_system_prompt(workspace_id);
            let mut messages = vec![ChatMessage::system(system_prompt)];
            messages.extend(history_messages);

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
                enable_thinking,
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
                            enable_thinking,
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
            for (tool_call, result) in &executed {
                if cancellation_requested(&cancel_rx) {
                    break;
                }
                persist_tool_result_with_compression(
                    db,
                    workspace_id,
                    on_event,
                    tool_call,
                    result,
                    &summary_provider,
                    &summary_model,
                    |usage| {
                        usage_tracker.record(usage);
                    },
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
        tool_calls: &[RequestedToolCall],
        args_map: &std::collections::HashMap<String, String>,
        tool_context: &super::tools::ToolContext,
        allowed_tool_names: &std::collections::HashSet<String>,
        on_event: &Channel<AgentEvent>,
        cancel_rx: &watch::Receiver<bool>,
        usage_tracker: &mut UsageTracker,
        workspace_id: &str,
    ) -> Result<Vec<(RequestedToolCall, String)>> {
        let readonly_end = common::readonly_tool_run_end(tool_calls, 0);

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

            let readonly_results: Vec<String> =
                futures::future::join_all(readonly_run.iter().map(|tool_call| async move {
                    if allowed_tool_names.contains(&tool_call.name) {
                        self.tools
                            .execute(&tool_call.name, &tool_call.arguments, tool_context)
                            .await
                    } else {
                        format!(
                            "错误：禁止调用工具 '{}'；请检查可用工具列表。",
                            tool_call.name
                        )
                    }
                }))
                .await;

            for (tool_call, result) in readonly_run.iter().zip(readonly_results) {
                if cancellation_requested(cancel_rx) {
                    return Ok(results);
                }
                if let Some(message) = sub_agent_failure_message(&result) {
                    anyhow::bail!("{}", message);
                }
                results.push((tool_call.clone(), result));
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
                if let Some(message) = sub_agent_failure_message(&result) {
                    anyhow::bail!("{}", message);
                }
                results.push((tool_call.clone(), result));
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
                if let Some(message) = sub_agent_failure_message(&result) {
                    anyhow::bail!("{}", message);
                }
                results.push((tool_call.clone(), result));
            }
            Ok(results)
        }
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
    ) -> String {
        let allowed = allowed_tool_names.contains(&tool_call.name);
        let is_sub_agent_call = tool_call.name == "call_sub_agent";
        let name = tool_call.name.clone();
        let arguments = tool_call.arguments.clone();

        if is_sub_agent_call {
            with_usage_paused(usage_tracker, workspace_id, on_event, || async {
                if allowed {
                    self.tools.execute(&name, &arguments, tool_context).await
                } else {
                    format!("错误：禁止调用工具 '{}'；请检查可用工具列表。", name)
                }
            })
            .await
        } else if allowed {
            self.tools.execute(&name, &arguments, tool_context).await
        } else {
            format!("错误：禁止调用工具 '{}'；请检查可用工具列表。", name)
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
        prompt.push_str(&format!(
            "\n\n## 系统时间\n\n当前本地时间：{}",
            super::prompt::current_local_time()
        ));
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
        prompt
    }

    fn build_tool_definitions(&self, workspace: &Path) -> Vec<ToolDefinition> {
        let configured = self.allowed_tools.lock().clone();
        let mut defs = self.tools.definitions_for_workspace(
            workspace,
            Option::<std::iter::Empty<&str>>::None,
            true,
        );

        if !configured.is_empty() {
            let allowed: std::collections::HashSet<String> = configured.into_iter().collect();
            defs.retain(|def| {
                allowed.contains(&def.function.name)
                    || def.function.name == "call_sub_agent"
                    || def.function.name == "list_sub_agents"
            });
        }
        defs
    }
}

fn empty_plain_chat_response_error(
    response: &LlmResponse,
    provider: &OpenAiCompatProvider,
    tool_count: usize,
    enable_thinking: bool,
) -> String {
    let raw_response = response.raw_response.trim();
    let response_detail = if raw_response.is_empty() {
        "<空>".to_string()
    } else {
        truncate_for_display(raw_response, 4_000, "\n...[LLM 接口响应内容已截断]")
    };

    format!(
        "LLM 返回了空响应且没有工具调用，无法继续执行。\n请求摘要：model={}, tools={}, enable_thinking={}\nLLM 接口响应内容：\n{}",
        provider.model(),
        tool_count,
        enable_thinking,
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
