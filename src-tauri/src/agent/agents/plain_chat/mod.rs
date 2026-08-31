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
use crate::agent::sub_agent::config::SubAgentConfig;
use crate::agent::sub_agent::SubAgentManager;
use crate::agent::tools::{
    CapabilitySet, ToolAction, ToolContext, ToolRegistry, ToolResult, ToolRunFinishUpdate,
    ToolRuntime, ToolSurface,
};
use crate::mcp::{tool_definitions_from_snapshot, McpRegistry, McpScope, ResolvedMcpTool};
use crate::shared::truncate_for_display;
use crate::ssh_tool::SshSessionManager;

const SUB_AGENT_TOOL_NAMES: [&str; 2] = ["list_sub_agents", "call_sub_agent"];

mod adapter;
mod context;
mod policy;
mod tool_batch;

use adapter::{emit_stop_and_finish, empty_plain_chat_response_error};

/// MCP 工具名契约（见 `mcp/registry.rs` 的 `resolve_mcp_tool`）：canonical 名
/// 恒为 `mcp__<server>__<tool>`；内置工具名不会以该前缀开头，因此可以用前缀
/// 在允许列表过滤时区分两类工具。
const MCP_TOOL_NAME_PREFIX: &str = "mcp__";

fn is_mcp_tool_name(name: &str) -> bool {
    name.starts_with(MCP_TOOL_NAME_PREFIX)
}

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
    /// 工具允许列表。混合契约：
    /// - 内置工具：**空列表 = 全部放行**（显式 fail-open 默认），
    ///   见 `is_tool_allowed_by_config` 的说明；
    /// - MCP 工具（`mcp__` 前缀名）：一律显式名单制——只有名字被明确写入
    ///   本列表才对该会话可见，空列表即无任何 MCP 工具。
    ///   定义层与系统提示词层共享该过滤（见 `retain_allowed_definitions` /
    ///   `allowed_mcp_tools_by_config`），模型被告知可调用的工具恒等于实际授权集。
    allowed_tools: Mutex<Vec<String>>,
    category_context: Mutex<Option<(String, String)>>,
    mcp_registry: McpRegistry,
    sub_agent_manager: Option<Arc<SubAgentManager>>,
    /// 本 run 会话已启用的子智能体快照。run 入口（`build_run_prompt`）异步
    /// 拉取一次（spawn_blocking）后，同一 run 内的同步路径（系统提示重建、
    /// 工具定义构建）只读缓存，避免在 async 运行期直接做同步 SQLite I/O。
    sub_agent_exposure: Mutex<Option<SubAgentExposure>>,
}

struct SubAgentExposure {
    workspace_id: String,
    agents: Vec<SubAgentConfig>,
}

impl PlainChatAgent {
    pub fn new(
        config: DispatcherAgentConfig,
        mcp_registry: McpRegistry,
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
            ToolRegistry::plain_chat_tools(mcp_registry.clone(), ssh_manager.clone());
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
            mcp_registry,
            sub_agent_manager,
            sub_agent_exposure: Mutex::new(None),
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
        *self.vision_provider.lock() =
            active_vision
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
        // 基础设置重应用时同步清除分类叠加（G9-03）：分类级配置
        // （allowed_tools/system_prompt）只在 apply_category_config 再次应用后
        // 生效（见 build_plain_chat_agent：总是先基础设置、后分类设置）。
        // 否则上一份 category_context 会与新的基础设置并存，一致性依赖调用顺序。
        *self.category_context.lock() = None;
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

    fn tool_surface_for_loop(&self, tool_context: &ToolContext) -> ToolSurface {
        ToolSurface::direct(
            self.build_tool_definitions(&tool_context.workspace_id, &tool_context.mcp_scope),
        )
    }

    fn build_iteration_messages(
        &self,
        ctx: &RunLoopContext<'_>,
        agent_loop: &AgentLoop,
        _tool_definitions: &[ToolDefinition],
    ) -> Result<Vec<ChatMessage>> {
        // 每轮迭代重建系统提示（G9-17）：系统时间、分类上下文、可用子智能体
        // 等动态内容不会随 run 进行而陈旧；history() 返回不含 system 的纯历史，
        // 避免重复插入 system 消息。
        let mut messages = Vec::with_capacity(agent_loop.history().len() + 1);
        messages.push(ChatMessage::system(
            self.build_effective_system_prompt(ctx.workspace_id),
        ));
        messages.extend(agent_loop.history().iter().cloned());
        Ok(messages)
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
        last_seq: Option<u64>,
    ) -> Result<DispatcherMessageRecord> {
        emit_stop_and_finish(
            ctx.db,
            ctx.workspace_id,
            ctx.on_event,
            partial,
            &ctx.usage_tracker,
            last_seq,
        )
        .await
    }

    async fn handle_no_tool_response(
        &self,
        ctx: &RunLoopContext<'_>,
        response: &LlmResponse,
        last_seq: Option<u64>,
    ) -> Result<DispatcherMessageRecord> {
        let content = response.content.trim().to_string();
        if content.is_empty() {
            anyhow::bail!(
                "{}",
                empty_plain_chat_response_error(
                    response,
                    &ctx.provider,
                    self.build_tool_definitions(ctx.workspace_id, &McpScope::Global)
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
                last_seq,
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
        if response.tool_calls.len() > crate::agent::tools::MAX_TOOL_CALLS_PER_BATCH {
            anyhow::bail!(
                "模型单轮返回 {} 个工具调用，超过运行时上限 {}；已在持久化或执行前拒绝。",
                response.tool_calls.len(),
                crate::agent::tools::MAX_TOOL_CALLS_PER_BATCH
            );
        }
        let tool_calls_payload = build_tool_calls_payload(&response.tool_calls, &self.tools)?;
        let args_map = build_args_map(&response.tool_calls, &self.tools)?;
        let mut llm_messages = Vec::new();

        for tc in &tool_calls_payload {
            common::emit(
                ctx.on_event,
                AgentEvent::ToolPlanned {
                    tool_call_id: tc.id.clone(),
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

        // 工具执行与「结果持久化 + run 收尾」统一在 execute_all_tools 内完成：
        // 取消 / 错误 / 子智能体致命失败的任何提前退出路径都会先把已执行结果
        // 落库并把 run 记录收敛到终态（G9-01），不再依赖调用方补收尾。
        let summary_provider = self.summary_provider(&iteration.request_provider);
        let summary_model = self.summary_model();
        let tool_messages = self
            .execute_all_tools(
                ctx.db,
                &response.tool_calls,
                &args_map,
                tool_context,
                &iteration.direct_capabilities,
                ctx.on_event,
                &ctx.cancel_rx,
                &mut ctx.usage_tracker,
                ctx.workspace_id,
                &summary_provider,
                &summary_model,
            )
            .await?;
        llm_messages.extend(tool_messages);

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
