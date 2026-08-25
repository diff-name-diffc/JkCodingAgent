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
use crate::agent::sub_agent::config::SubAgentConfig;
use crate::agent::sub_agent::{tool::sub_agent_failure_message, SubAgentManager};
use crate::agent::tools::{
    CapabilitySet, ToolAction, ToolContext, ToolRegistry, ToolResult, ToolRunFinishUpdate,
    ToolRuntime, ToolSurface,
};
use crate::mcp::{tool_definitions_from_snapshot, McpRegistry, McpScope};
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
    /// 工具允许列表。契约：**空列表 = 全部放行**（显式 fail-open 默认），
    /// 见 `is_tool_allowed_by_config` 的说明。
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

/// 单个已执行工具收尾的结果。
enum ExecutedToolFinalize {
    Done,
    /// 工具致命失败：当前 run 已按 fatal_error 收尾，由调用方决定中止时机
    /// （只读并行批需先把兄弟结果全部收尾，串行批可立即中止）。
    FatalTool(String),
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

    /// 执行一批 tool_calls，并保证每个已创建的 tool run 记录都走到终态（G9-01）：
    ///
    /// - 取消：尚未开始的工具不再创建 run 记录；已执行完的结果全部持久化并按
    ///   真实状态收尾，不再中途丢弃（旧实现会遗留永久 started 的悬挂记录并丢失结果）。
    /// - 出错（run 创建失败 / 子智能体致命失败 / 持久化失败）：已执行结果先持久化，
    ///   在途 run 以 failed/fatal_error 收尾后再向上传播错误。
    ///
    /// 返回本批工具结果对应的 LLM tool 消息（已落库），由调用方并入上下文。
    #[allow(clippy::too_many_arguments)]
    async fn execute_all_tools(
        &self,
        db: &DispatcherDb,
        tool_calls: &[RequestedToolCall],
        args_map: &std::collections::HashMap<String, String>,
        tool_context: &ToolContext,
        direct_capabilities: &CapabilitySet,
        on_event: &Channel<AgentEvent>,
        cancel_rx: &watch::Receiver<bool>,
        usage_tracker: &mut UsageTracker,
        workspace_id: &str,
        summary_provider: &OpenAiCompatProvider,
        summary_model: &str,
    ) -> Result<Vec<ChatMessage>> {
        let mut llm_messages = Vec::new();
        let readonly_end =
            common::readonly_tool_run_end(&self.tools, &tool_context.mcp_scope, tool_calls, 0);

        if readonly_end >= 2 {
            let readonly_run = &tool_calls[..readonly_end];
            for tool_call in readonly_run {
                common::emit(
                    on_event,
                    AgentEvent::ToolStarted {
                        tool_call_id: tool_call.id.clone(),
                        name: tool_call.name.clone(),
                        arguments: args_map
                            .get(&tool_call.id)
                            .cloned()
                            .unwrap_or_else(|| "{}".to_string()),
                    },
                );
            }

            let mut run_ids = Vec::with_capacity(readonly_run.len());
            for tool_call in readonly_run {
                match self
                    .create_and_start_tool_run(db, workspace_id, tool_context, on_event, tool_call)
                    .await
                {
                    Ok(run_id) => run_ids.push(run_id),
                    Err(error) => {
                        // 补偿：把本批已创建的 run 收敛为 failed，
                        // 避免遗留「已启动未收尾」的悬挂记录。
                        let message = format!("错误：创建工具运行记录失败：{error}");
                        self.finalize_started_runs(db, on_event, &run_ids, "failed", &message)
                            .await;
                        return Err(error);
                    }
                }
            }

            let semaphore = Arc::new(tokio::sync::Semaphore::new(
                crate::agent::tools::MAX_PARALLEL_TOOL_CALLS,
            ));
            let readonly_results: Vec<ToolResult> =
                futures::future::join_all(readonly_run.iter().map(|tool_call| {
                    let cancel_rx = cancel_rx.clone();
                    let semaphore = Arc::clone(&semaphore);
                    async move {
                        let Ok(_permit) = semaphore.acquire().await else {
                            return ToolResult::fatal_error(
                                "只读工具并发调度器意外关闭，已拒绝执行",
                            );
                        };
                        ToolRuntime::execute_tool_with_cancellation(
                            &self.tools,
                            direct_capabilities,
                            tool_call,
                            tool_context,
                            cancel_rx,
                        )
                        .await
                    }
                }))
                .await;

            // 只读批的所有工具此时都已执行完、run 记录都已创建：任何退出路径
            // （取消 / 子智能体致命失败 / 持久化失败）都必须先把它们全部收尾，
            // 不允许遗留悬挂记录或丢弃已执行结果。
            let mut pending = readonly_run
                .iter()
                .zip(readonly_results)
                .zip(run_ids)
                .map(|((tool_call, result), run_id)| (tool_call.clone(), result, run_id))
                .collect::<Vec<_>>();
            let mut fatal_message: Option<String> = None;
            while !pending.is_empty() {
                let (tool_call, result, run_id) = pending.remove(0);
                match self
                    .persist_and_finalize_executed_tool(
                        db,
                        on_event,
                        workspace_id,
                        &tool_context.mcp_scope,
                        &tool_call,
                        result,
                        &run_id,
                        summary_provider,
                        summary_model,
                        usage_tracker,
                        &mut llm_messages,
                    )
                    .await
                {
                    Ok(ExecutedToolFinalize::Done) => {}
                    Ok(ExecutedToolFinalize::FatalTool(message)) => {
                        // 先收尾兄弟结果，循环结束后再中止。
                        fatal_message.get_or_insert(message);
                    }
                    Err(error) => {
                        // 持久化失败：其余已执行的 run 无法落库，统一收敛为
                        // failed 避免悬挂，然后向上抛原始错误。
                        let pending_run_ids = pending
                            .iter()
                            .map(|(_, _, run_id)| run_id.clone())
                            .collect::<Vec<_>>();
                        self.finalize_started_runs(
                            db,
                            on_event,
                            &pending_run_ids,
                            "failed",
                            &format!("错误：工具结果持久化失败：{error}"),
                        )
                        .await;
                        return Err(error);
                    }
                }
            }
            if let Some(message) = fatal_message {
                anyhow::bail!("{}", message);
            }

            let remaining = &tool_calls[readonly_end..];
            for tool_call in remaining {
                if cancellation_requested(cancel_rx) {
                    break; // 尚未创建 run 记录，无悬挂风险
                }
                common::emit(
                    on_event,
                    AgentEvent::ToolStarted {
                        tool_call_id: tool_call.id.clone(),
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
                        direct_capabilities,
                        on_event,
                        usage_tracker,
                        workspace_id,
                        cancel_rx,
                    )
                    .await;
                match self
                    .persist_and_finalize_executed_tool(
                        db,
                        on_event,
                        workspace_id,
                        &tool_context.mcp_scope,
                        tool_call,
                        result,
                        &run_id,
                        summary_provider,
                        summary_model,
                        usage_tracker,
                        &mut llm_messages,
                    )
                    .await?
                {
                    ExecutedToolFinalize::Done => {}
                    // 串行批：后续工具尚未创建 run 记录，可立即中止
                    ExecutedToolFinalize::FatalTool(message) => {
                        anyhow::bail!("{}", message);
                    }
                }
            }

            Ok(llm_messages)
        } else {
            for tool_call in tool_calls {
                if cancellation_requested(cancel_rx) {
                    break; // 尚未创建 run 记录，无悬挂风险
                }
                common::emit(
                    on_event,
                    AgentEvent::ToolStarted {
                        tool_call_id: tool_call.id.clone(),
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
                        direct_capabilities,
                        on_event,
                        usage_tracker,
                        workspace_id,
                        cancel_rx,
                    )
                    .await;
                match self
                    .persist_and_finalize_executed_tool(
                        db,
                        on_event,
                        workspace_id,
                        &tool_context.mcp_scope,
                        tool_call,
                        result,
                        &run_id,
                        summary_provider,
                        summary_model,
                        usage_tracker,
                        &mut llm_messages,
                    )
                    .await?
                {
                    ExecutedToolFinalize::Done => {}
                    // 串行批：后续工具尚未创建 run 记录，可立即中止
                    ExecutedToolFinalize::FatalTool(message) => {
                        anyhow::bail!("{}", message);
                    }
                }
            }
            Ok(llm_messages)
        }
    }

    /// 持久化单个已执行工具的结果并收尾其 run 记录。
    ///
    /// - 检测到结构化致命错误：当前 run 以 fatal_error 收尾，返回
    ///   `FatalTool` 由调用方决定中止时机（不在此处直接抛错，避免
    ///   并行批中其余已执行工具的 run 被悬挂）；
    /// - 结果持久化失败：尽力把当前 run 收敛为 failed 后再向上抛错；
    /// - run 收尾失败：结果已落库，仅告警不中断主流程。
    #[allow(clippy::too_many_arguments)]
    async fn persist_and_finalize_executed_tool(
        &self,
        db: &DispatcherDb,
        on_event: &Channel<AgentEvent>,
        workspace_id: &str,
        mcp_scope: &McpScope,
        tool_call: &RequestedToolCall,
        result: ToolResult,
        run_id: &str,
        summary_provider: &OpenAiCompatProvider,
        summary_model: &str,
        usage_tracker: &mut UsageTracker,
        llm_messages: &mut Vec<ChatMessage>,
    ) -> Result<ExecutedToolFinalize> {
        let result_text = result.output_for_llm();
        let result_metadata_json = result.run_metadata_json();
        let fatal_message = if result.status == crate::agent::tools::ToolStatus::FatalError {
            Some(
                sub_agent_failure_message(&result_text)
                    .unwrap_or(result_text.as_str())
                    .to_string(),
            )
        } else {
            sub_agent_failure_message(&result_text).map(str::to_string)
        };
        if let Some(message) = fatal_message {
            self.finish_tool_run(
                db,
                on_event,
                run_id,
                "fatal_error",
                None,
                None,
                Some("sub_agent_failure"),
                Some(&message),
                None,
                result_metadata_json.as_deref(),
            )
            .await?;
            return Ok(ExecutedToolFinalize::FatalTool(message));
        }

        let tool_message = match persist_tool_result_with_compression(
            db,
            workspace_id,
            on_event,
            tool_call,
            &self.tools,
            mcp_scope,
            &result_text,
            summary_provider,
            summary_model,
            |usage| {
                usage_tracker.record(usage);
            },
        )
        .await
        {
            Ok(message) => message,
            Err(error) => {
                self.finalize_started_runs(
                    db,
                    on_event,
                    &[run_id.to_string()],
                    "failed",
                    &format!("错误：工具结果持久化失败：{error}"),
                )
                .await;
                return Err(error);
            }
        };
        if let Some(message) = tool_message.to_llm_message() {
            llm_messages.push(message);
        }
        if let Err(error) = self
            .finish_tool_run(
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
            .await
        {
            eprintln!(
                "错误：工具 '{}' 的 run 记录收尾失败（结果已持久化）：{}",
                tool_call.name, error
            );
        }
        Ok(ExecutedToolFinalize::Done)
    }

    /// 尽力把一组已创建的 tool run 收敛到终态（补偿路径）：
    /// 二次失败仅告警，绝不掩盖原始错误。
    async fn finalize_started_runs(
        &self,
        db: &DispatcherDb,
        on_event: &Channel<AgentEvent>,
        run_ids: &[String],
        status: &str,
        error_message: &str,
    ) {
        for run_id in run_ids {
            if let Err(error) = self
                .finish_tool_run(
                    db,
                    on_event,
                    run_id,
                    status,
                    None,
                    None,
                    Some("internal"),
                    Some(error_message),
                    None,
                    None,
                )
                .await
            {
                eprintln!("错误：补偿收尾工具 run 记录 {run_id} 失败：{error}");
            }
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
            &tool_context.mcp_scope,
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
        direct_capabilities: &CapabilitySet,
        on_event: &Channel<AgentEvent>,
        usage_tracker: &mut UsageTracker,
        workspace_id: &str,
        cancel_rx: &watch::Receiver<bool>,
    ) -> ToolResult {
        let is_sub_agent_call = tool_call.name == "call_sub_agent";

        if is_sub_agent_call {
            with_usage_paused(usage_tracker, workspace_id, on_event, || async {
                ToolRuntime::execute_tool_with_cancellation(
                    &self.tools,
                    direct_capabilities,
                    tool_call,
                    tool_context,
                    cancel_rx.clone(),
                )
                .await
            })
            .await
        } else {
            ToolRuntime::execute_tool_with_cancellation(
                &self.tools,
                direct_capabilities,
                tool_call,
                tool_context,
                cancel_rx.clone(),
            )
            .await
        }
    }

    /// 每个会话独立的文件沙箱（G9-04）：`root_dir/plain-chat-browser/<会话子目录>`。
    ///
    /// 旧实现让所有聊天会话共享固定目录，`restrict_to_workspace: true` 反而把
    /// 并发会话的文件类工具都限定在同一目录内互相覆盖；按会话（workspace_id）
    /// 建子目录后各会话的文件结果互不干扰。该目录只是文件沙箱——聊天的
    /// MCP 配置一律来自全局注册表（`McpScope::Global`），不在这里落任何配置。
    async fn session_workspace(&self, workspace_id: &str) -> Result<PathBuf> {
        let workspace = self
            .config
            .root_dir
            .join("plain-chat-browser")
            .join(session_workspace_dir_name(workspace_id));
        tokio::task::spawn_blocking({
            let workspace = workspace.clone();
            move || {
                fs::create_dir_all(&workspace)
                    .with_context(|| format!("create {}", workspace.display()))
            }
        })
        .await
        .map_err(|error| {
            anyhow::anyhow!("create plain chat session workspace panicked: {error}")
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
        let chat_image_paths = match db.list_chat_image_paths_async(workspace_id).await {
            Ok(paths) => paths,
            Err(error) => {
                eprintln!(
                    "读取会话聊天图片授权失败（workspace_id={workspace_id}），按空白名单收紧：{error:#}"
                );
                Vec::new()
            }
        };
        // get_settings_v2 是同步 SQLite 读取，async 路径必须经 spawn_blocking，
        // 避免阻塞 Tokio 运行时线程（G9-02）。
        let settings_db = db.clone();
        let ssh_review = tokio::task::spawn_blocking(move || {
            settings_db
                .get_settings_v2()
                .ok()
                .and_then(|settings| settings.review.is_configured().then_some(settings.review))
        })
        .await
        .unwrap_or(None);
        ToolContext {
            workspace_id: workspace_id.to_string(),
            workspace: workspace.to_path_buf(),
            // 普通聊天没有项目语境：MCP 一律走全局注册表，所有会话共享。
            mcp_scope: McpScope::Global,
            session_title,
            user_task,
            ssh_review,
            exec_timeout_secs: self.config.exec_timeout_secs,
            restrict_to_workspace: true,
            // 只放行当前会话已绑定的精确图片文件；其他会话图片和全局设置
            // 目录均不可见。路径规范化失败会继续 fail-closed 剔除。
            extra_allowed_dirs: chat_image_paths,
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
            current_tool_spec_hash: None,
            cancel_rx: None,
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
            // 只读 run 入口预热的缓存，避免在运行期（含 async 路径）直接做
            // 同步 SQLite I/O；缓存未命中时按空列表降级。
            let agents = self.cached_enabled_sub_agents(workspace_id);
            if !agents.is_empty() {
                prompt.push_str("\n\n## 当前可用子智能体\n\n");
                prompt.push_str("以下是当前会话已启用的子智能体，你可以直接调用：\n\n");
                for agent in &agents {
                    prompt.push_str(&format!(
                        "- **{}** (`{}`): {}\n",
                        agent.agent_name, agent.agent_id, agent.description
                    ));
                }
                prompt.push_str(
                    "\n使用方式：调用 call_sub_agent(agent_id, task) 来让子智能体处理特定任务。\n",
                );
            }
        }
        // MCP 工具按全局作用域快照动态注入（run 入口 prepare_run_workspace 已预热
        // 缓存）；快照缺失时按空列表降级，不虚构工具。列出显式清单是为了让模型
        // 明确知道自己已接入哪些第三方工具，避免凭提示词臆断「没有 MCP」。
        let mcp_tools = tool_definitions_from_snapshot(
            self.mcp_registry
                .cached_for_scope(&McpScope::Global)
                .as_ref(),
        );
        if !mcp_tools.is_empty() {
            prompt.push_str("\n\n## 当前可用 MCP 工具\n\n");
            prompt.push_str("已接入全局 MCP 注册表中的第三方工具，需要时可直接按工具名调用：\n\n");
            for tool in &mcp_tools {
                let description: String = tool.description.trim().chars().take(120).collect();
                prompt.push_str(&format!("- `{}`：{}\n", tool.canonical_name, description));
            }
        }
        prompt
    }

    /// run 入口异步预热子智能体缓存（spawn_blocking 包裹 SQLite 读取）。
    /// 同一 run 内的同步路径（系统提示重建、工具定义构建）之后只读缓存。
    async fn warm_sub_agent_exposure(&self, workspace_id: &str) {
        let manager = self.sub_agent_manager.clone();
        let wid = workspace_id.to_string();
        let agents = tokio::task::spawn_blocking(move || {
            manager
                .as_ref()
                .and_then(|manager| manager.get_enabled_for_session(&wid).ok())
                .unwrap_or_default()
        })
        .await
        .unwrap_or_default();
        *self.sub_agent_exposure.lock() = Some(SubAgentExposure {
            workspace_id: workspace_id.to_string(),
            agents,
        });
    }

    fn cached_enabled_sub_agents(&self, workspace_id: &str) -> Vec<SubAgentConfig> {
        self.sub_agent_exposure
            .lock()
            .as_ref()
            .filter(|exposure| exposure.workspace_id == workspace_id)
            .map(|exposure| exposure.agents.clone())
            .unwrap_or_default()
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
        !self.cached_enabled_sub_agents(workspace_id).is_empty()
    }

    fn build_tool_definitions(&self, workspace_id: &str, scope: &McpScope) -> Vec<ToolDefinition> {
        let configured = self.allowed_tools.lock().clone();
        let mut defs = self.tools.definitions_for_scope(
            scope,
            Option::<std::iter::Empty<&str>>::None,
            true,
        );

        // 与 is_tool_allowed_by_config 同契约：configured 为空 = 全部放行，
        // 非空才按允许列表（含启用子智能体时的子智能体工具）收敛内置工具；
        // 动态（MCP）工具不受允许列表约束（注册表层治理），始终保留。
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

    fn tool_surface_for_loop(&self, tool_context: &ToolContext) -> ToolSurface {
        ToolSurface::direct(self.build_tool_definitions(
            &tool_context.workspace_id,
            &tool_context.mcp_scope,
        ))
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

/// 判断工具是否被用户配置的允许列表放行。
///
/// 契约（G9-03，显式声明）：**空列表 = 全部放行**（fail-open 默认）。理由：
/// 1) 普通聊天的工具注册表本身是精选集（plain_chat_tools + 可选子智能体工具），
///    并非全量工具面；2) 设置页与分类配置以「空」表达「不限制」，若改为
///    fail-closed，默认/未配置用户将直接失去全部工具且 UI 无「全部」表达方式。
/// 可执行工具的安全由命令审查门禁（SSH/local_zsh fail-closed 审查）与工作区
/// 限制兜底，而非依赖此处默认拒绝。
///
/// 允许列表只约束内置工具：动态（MCP）工具名随服务器配置生成，静态列表
/// 无法表达，其启停在 MCP 注册表层治理（注册表层已让白名单不拦截动态工具）。
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

/// 由会话 ID 生成会话工作区子目录名（G9-04）。
///
/// 合法形态的 ID（字母数字 / `-` / `_`，≤64 字符，非 `.` 开头）原样使用；
/// 其余先过滤为安全字符，再追加确定性 FNV-1a 哈希后缀，保证：
/// 1) 不含路径分隔符与 `..`，无法越界（workspace_id 来自前端输入）；
/// 2) 不同会话 ID 不折叠到同一目录；
/// 3) 同一会话 ID 跨进程 / 重启始终得到同一目录（哈希确定性，不依赖随机态）。
fn session_workspace_dir_name(workspace_id: &str) -> String {
    let trimmed = workspace_id.trim();
    let is_plain_safe = !trimmed.is_empty()
        && trimmed.len() <= 64
        && !trimmed.starts_with('.')
        && trimmed
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'));
    if is_plain_safe {
        return trimmed.to_string();
    }

    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in trimmed.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let sanitized: String = trimmed
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
        .take(32)
        .collect();
    if sanitized.is_empty() {
        format!("session-{hash:016x}")
    } else {
        format!("{sanitized}-{hash:016x}")
    }
}

#[async_trait]
impl AgentRunAdapter for PlainChatAgent {
    async fn prepare_run_workspace(&self, request: &AgentRunRequest<'_>) -> Result<PathBuf> {
        let workspace = self.session_workspace(request.workspace_id).await?;
        // 聊天 MCP 一律来自全局注册表：所有会话共享同一份快照，
        // 不再按会话目录各存一份相同内容。
        self.mcp_registry
            .ensure_recent(&McpScope::Global)
            .await
            .map_err(anyhow::Error::msg)
            .context("刷新聊天 MCP 状态失败")?;
        Ok(workspace)
    }

    fn provider_snapshot(&self) -> OpenAiCompatProvider {
        self.provider.lock().clone()
    }

    fn provider_missing_message(&self) -> &'static str {
        "错误：聊天 LLM API Key 未配置。请在设置中配置，或设置 DASHSCOPE_API_KEY / OPENAI_API_KEY 环境变量。"
    }

    async fn build_run_prompt(
        &self,
        workspace_id: &str,
        _workspace: &Path,
    ) -> Result<RunPromptState> {
        // run 入口预热子智能体缓存（spawn_blocking），保证后续同步路径
        // （每轮系统提示重建、工具定义构建）不再触发同步 SQLite I/O。
        self.warm_sub_agent_exposure(workspace_id).await;
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

    #[test]
    fn empty_allowlist_explicitly_allows_every_tool() {
        // G9-03 显式契约：空列表 = 全部放行（fail-open 默认）
        assert!(is_tool_allowed_by_config(&[], "local_zsh"));
        assert!(is_tool_allowed_by_config(&[], "call_sub_agent"));
        assert!(!is_tool_allowed_by_config(
            &["browser_read_text".to_string()],
            "local_zsh"
        ));
        assert!(is_tool_allowed_by_config(
            &["local_zsh".to_string()],
            "local_zsh"
        ));
    }

    #[test]
    fn session_workspace_dir_name_keeps_safe_ids_unchanged() {
        assert_eq!(session_workspace_dir_name("abc-123_XYZ"), "abc-123_XYZ");
        assert_eq!(
            session_workspace_dir_name("5f9b2c8e-1a2d-4e3f-9a8b-7c6d5e4f3a2b"),
            "5f9b2c8e-1a2d-4e3f-9a8b-7c6d5e4f3a2b"
        );
    }

    #[test]
    fn session_workspace_dir_name_sanitizes_traversal_and_stays_deterministic() {
        let dotted = session_workspace_dir_name("../etc");
        assert!(!dotted.contains(".."));
        assert!(!dotted.contains('/'));

        let slashed = session_workspace_dir_name("a/b");
        assert!(!slashed.contains('/'));
        // 确定性：同一 ID 每次得到同一目录
        assert_eq!(slashed, session_workspace_dir_name("a/b"));
        // 不折叠：不同 ID 不会撞到同一目录
        assert_ne!(slashed, session_workspace_dir_name("a_b"));

        // 全非法字符退化为 session-哈希
        let blank = session_workspace_dir_name("  ");
        assert!(blank.starts_with("session-"));
        assert_eq!(blank, session_workspace_dir_name("  "));
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
    last_seq: Option<u64>,
) -> Result<DispatcherMessageRecord> {
    let content = build_stopped_plain_chat_reply(partial);
    let usage_stats = usage_tracker.snapshot();
    let reply = persist_assistant_message(db, workspace_id, &content, &usage_stats).await?;
    common::emit(
        on_event,
        AgentEvent::AssistantMessage {
            message: reply.clone(),
            last_seq,
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
