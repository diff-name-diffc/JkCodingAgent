//! 普通聊天 Agent：按分类配置共享成果目录、模型槽位与工具面。
//!
//! 每轮 run 由命令层现建现用（设置变更下一轮即生效），本结构只负责装配：
//! 1. 设置 → 用途槽位规格（`resolve_purpose_specs`）→ rig 模型；
//! 2. 工具面：exec + media + MCP（全局作用域）+ 子智能体工具，按允许列表过滤；
//! 3. 系统提示：配置提示 + 分类上下文 + 系统时间 + 子智能体清单 + MCP 清单
//!    + SSH 备忘录纪律 + 运行工作目录块；
//! 4. 历史（DB 最近若干轮对话）→ rig 消息；
//! 5. 运行 `rig_ext::r#loop::run_rig_loop`（三段式策略：审查门禁 + 台账 + 超时）。

mod assembly;
mod prompt;

pub(crate) use prompt::render_runtime_workspace;
use prompt::{chat_image_dir, SSH_MEMO_GUIDANCE};

use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;
use tauri::ipc::Channel;
use tauri::AppHandle;
use tokio::sync::watch;

use crate::agent::config::{DispatcherAgentConfig, DEFAULT_PLAIN_CHAT_SYSTEM_PROMPT};
use crate::agent::db::{
    AgentContext, AhaSettingsV2, ChatCategoryAgentConfig, DispatcherDb, DispatcherMessageRecord,
};
use crate::agent::rig_ext::chat_workspace::ChatWorkspace;
use crate::agent::rig_ext::events::AgentEvent;
use crate::agent::rig_ext::message::{apply_stored_session_summary, chat_history_to_rig_with_ids};
use crate::agent::rig_ext::model::{
    completions_model, resolve_purpose_specs, ModelSelectionHandle, PurposeModelSpecs,
    PurposeSwitchingModel,
};
use crate::agent::rig_ext::r#loop::{
    run_rig_loop, AppToolExecutionPolicy, AppToolPolicyConfig, RigLoopHooks, RigToolSurface,
};
use crate::agent::rig_ext::review::RigReviewContext;
use crate::agent::rig_ext::sub_agent::{call_sub_agent_tool, list_sub_agents_tool};
use crate::agent::rig_ext::tool_result::RigSummaryModel;
use crate::agent::rig_ext::tools::deps::{ImageToolConfig, RigToolDeps, ToolTimeoutDefaults};
use crate::agent::rig_ext::tools::edit::edit_tools;
use crate::agent::rig_ext::tools::exec::exec_tools;
use crate::agent::rig_ext::tools::mcp::mcp_tools;
use crate::agent::rig_ext::tools::media::media_tools;
use crate::agent::sub_agent::config::SubAgentConfig;
use crate::agent::sub_agent::SubAgentManager;
use crate::mcp::{tool_definitions_from_snapshot, McpRegistry, McpScope, ResolvedMcpTool};
use crate::ssh_tool::SshSessionManager;

use super::{retain_allowed_tools, tool_result_policies_from_specs};

/// 一轮聊天运行的输入（与命令层 `run_agent_turn_skeleton` 的契约一致）。
pub struct ChatTurnRequest<'a> {
    pub db: &'a DispatcherDb,
    pub workspace_id: &'a str,
    pub user_segments_json: String,
    pub on_event: Channel<AgentEvent>,
    pub cancel_rx: watch::Receiver<bool>,
}

/// 普通聊天 Agent：持有跨轮次稳定的配置与服务句柄 + 本轮设置快照。
pub struct RigPlainChatAgent {
    config: DispatcherAgentConfig,
    mcp_registry: McpRegistry,
    ssh_manager: SshSessionManager,
    log_snapshots: crate::agent::log_snapshot::LogSnapshotManager,
    term_registry: crate::ssh_tool::term::TermSessionRegistry,
    sub_agent_manager: Option<Arc<SubAgentManager>>,
    app_handle: Option<AppHandle>,

    specs: PurposeModelSpecs,
    /// 配置的系统提示（空 → `DEFAULT_PLAIN_CHAT_SYSTEM_PROMPT`）。
    system_prompt: Mutex<String>,
    /// 工具允许列表（内置空列表 = 全部放行；MCP 显式名单制）。
    allowed_tools: Mutex<Vec<String>>,
    category_context: Mutex<Option<(String, String)>>,
    project_directories: Mutex<Vec<String>>,
    /// 审查配置（settings.review；未配置 → None，命令类工具 fail-closed）。
    review_config: Mutex<Option<crate::agent::db::settings::SshReviewConfig>>,
    image_credentials: Mutex<crate::agent::db::settings::ImageModelCredentials>,
    /// 用户配置的工具超时默认（AhaSettingsV2.toolTimeouts 解析产物）：
    /// run 期注入执行策略 deadline 与工具 HTTP 预算（同一解析，防口径漂移）。
    tool_timeouts: Mutex<ToolTimeoutDefaults>,
    /// 工具迭代轮数上限（AhaSettingsV2.maxToolIterations 解析产物；
    /// 未配置 = 内置默认），run 期注入 hooks.max_iterations。
    max_tool_iterations: Mutex<usize>,
    /// 本 run 会话已启用的子智能体快照（run 入口异步拉取一次）。
    sub_agent_exposure: Mutex<Option<SubAgentExposure>>,
}

struct SubAgentExposure {
    workspace_id: String,
    agents: Vec<SubAgentConfig>,
}

impl RigPlainChatAgent {
    pub fn new(
        config: DispatcherAgentConfig,
        mcp_registry: McpRegistry,
        ssh_manager: SshSessionManager,
        log_snapshots: crate::agent::log_snapshot::LogSnapshotManager,
        term_registry: crate::ssh_tool::term::TermSessionRegistry,
        sub_agent_manager: Option<Arc<SubAgentManager>>,
    ) -> Self {
        Self {
            specs: resolve_purpose_specs(&AhaSettingsV2::default(), AgentContext::Chat, &config),
            config,
            mcp_registry,
            ssh_manager,
            log_snapshots,
            term_registry,
            sub_agent_manager,
            app_handle: None,
            system_prompt: Mutex::new(DEFAULT_PLAIN_CHAT_SYSTEM_PROMPT.to_string()),
            allowed_tools: Mutex::new(Vec::new()),
            category_context: Mutex::new(None),
            project_directories: Mutex::new(Vec::new()),
            review_config: Mutex::new(None),
            image_credentials: Mutex::new(Default::default()),
            tool_timeouts: Mutex::new(Default::default()),
            max_tool_iterations: Mutex::new(crate::agent::config::DEFAULT_MAX_TOOL_ITERATIONS),
            sub_agent_exposure: Mutex::new(None),
        }
    }

    pub fn with_app_handle(mut self, app_handle: AppHandle) -> Self {
        self.app_handle = Some(app_handle);
        self
    }

    /// 应用基础设置（槽位规格、系统提示、允许列表、审查与图像凭据），
    /// 并清除分类叠加（分类配置随后叠加）。
    pub fn apply_settings_v2(&mut self, settings: &AhaSettingsV2, context: AgentContext) {
        self.specs = resolve_purpose_specs(settings, context, &self.config);

        let ctx_config = match context {
            AgentContext::Project => &settings.project,
            AgentContext::Chat => &settings.chat,
        };
        *self.allowed_tools.lock() = ctx_config.allowed_tools.clone();
        *self.review_config.lock() = settings
            .review
            .is_configured()
            .then(|| settings.review.clone());
        *self.image_credentials.lock() = settings.shared.image_model_credentials();
        *self.tool_timeouts.lock() = ToolTimeoutDefaults::from(&settings.tool_timeouts);
        *self.max_tool_iterations.lock() =
            crate::agent::config::effective_max_tool_iterations(settings.max_tool_iterations);
        // 基础设置重应用时同步清除分类叠加（对齐旧实现的顺序契约）。
        *self.category_context.lock() = None;
        self.project_directories.lock().clear();
    }

    /// 叠加聊天分类级配置（系统提示 + 允许列表）。
    pub fn apply_category_config(&mut self, config: &ChatCategoryAgentConfig) {
        *self.allowed_tools.lock() = config.allowed_tools.clone();
        *self.system_prompt.lock() = config.system_prompt.clone();
        *self.category_context.lock() =
            Some((config.category_id.clone(), config.category_name.clone()));
        *self.project_directories.lock() = config.project_directories.clone();
    }

    pub fn is_configured(&self) -> bool {
        self.specs.chat.is_configured()
    }

    /// 每个会话独立的文件沙箱：`root_dir/plain-chat-browser/<会话子目录>`。
    pub async fn session_workspace(&self, workspace_id: &str) -> anyhow::Result<PathBuf> {
        let workspace = ChatWorkspace::session_directory(&self.config.root_dir, workspace_id);
        tokio::task::spawn_blocking({
            let workspace = workspace.clone();
            move || std::fs::create_dir_all(&workspace)
        })
        .await
        .map_err(|error| anyhow::anyhow!("create plain chat session workspace panicked: {error}"))?
        .map_err(|error| anyhow::anyhow!("create {}: {error}", workspace.display()))?;
        Ok(workspace)
    }

    /// run 入口预热子智能体缓存（spawn_blocking 包裹同步 SQLite 读取）。
    pub async fn warm_sub_agent_exposure(&self, workspace_id: &str) {
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

    fn has_enabled_sub_agents(&self, workspace_id: &str) -> bool {
        !self.cached_enabled_sub_agents(workspace_id).is_empty()
    }

    /// 本轮系统提示（排除运行工作目录块——后者随工具面在每轮迭代补入）。
    fn base_system_prompt(&self, workspace_id: &str) -> String {
        let mut prompt = {
            let configured = self.system_prompt.lock().trim().to_string();
            if configured.is_empty() {
                DEFAULT_PLAIN_CHAT_SYSTEM_PROMPT.to_string()
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
        let mcp_tools = self.allowed_mcp_tools(workspace_id);
        if !mcp_tools.is_empty() {
            prompt.push_str("\n\n## 当前可用 MCP 工具\n\n");
            prompt.push_str("已接入全局 MCP 注册表中的第三方工具，需要时可直接按工具名调用：\n\n");
            for tool in &mcp_tools {
                let description: String = tool.description.trim().chars().take(120).collect();
                prompt.push_str(&format!("- `{}`：{}\n", tool.canonical_name, description));
            }
        }
        if self.tool_allowed("ssh_memo_read") {
            prompt.push_str(SSH_MEMO_GUIDANCE);
        }
        if crate::agent::rig_ext::tools::log::LOG_TOOL_NAMES
            .iter()
            .any(|name| self.tool_allowed(name))
        {
            prompt.push_str(prompt::LOG_SEARCH_GUIDANCE);
        }
        prompt
    }

    fn tool_allowed(&self, tool_name: &str) -> bool {
        let configured = self.allowed_tools.lock();
        configured.is_empty() || configured.iter().any(|name| name == tool_name)
    }

    /// 当前允许列表下可用的 MCP 工具（全局作用域快照 ∩ 显式名单）。
    fn allowed_mcp_tools(&self, _workspace_id: &str) -> Vec<ResolvedMcpTool> {
        let configured = self.allowed_tools.lock().clone();
        if configured.is_empty() {
            return Vec::new();
        }
        let snapshot_tools = tool_definitions_from_snapshot(
            self.mcp_registry
                .cached_for_scope(&McpScope::Global)
                .as_ref(),
        );
        super::allowed_mcp_tools_by_config(snapshot_tools, &configured)
    }

    /// 装配本轮工具面（含按允许列表过滤）。
    async fn build_surface(&self, deps: &RigToolDeps, workspace_id: &str) -> RigToolSurface {
        let mut tools = exec_tools(deps);
        tools.extend(edit_tools(deps));
        tools.extend(media_tools(deps));
        tools.extend(mcp_tools(deps).await);

        if let Some(manager) = &self.sub_agent_manager {
            let parent_spec = self.specs.chat.clone();
            tools.push(call_sub_agent_tool(
                Arc::clone(manager),
                deps.clone(),
                parent_spec,
                self.app_handle.clone(),
                workspace_id.to_string(),
            ));
            tools.push(list_sub_agents_tool(
                Arc::clone(manager),
                workspace_id.to_string(),
            ));
        }

        let configured = self.allowed_tools.lock().clone();
        let has_sub_agents = self.has_enabled_sub_agents(workspace_id);
        let tools = retain_allowed_tools(tools, &configured, has_sub_agents);
        RigToolSurface::new(tools).with_policies(tool_result_policies_from_specs())
    }

    /// 执行一轮聊天：落库用户消息 → 装配 → 跑 rig 循环 → 返回收口消息。
    pub async fn run_turn(
        &self,
        request: ChatTurnRequest<'_>,
    ) -> anyhow::Result<DispatcherMessageRecord> {
        let db = request.db;
        let workspace_id = request.workspace_id;
        let on_event = request.on_event;

        crate::agent::common::emit(
            &on_event,
            AgentEvent::Started {
                workspace_id: workspace_id.to_string(),
            },
        );

        // 发送前校验：Image 段引用的文件必须存在（半残消息入库会让后续请求反复踩坑）。
        db.validate_chat_image_segments_async(&request.user_segments_json)
            .await?;
        let user = db
            .add_visible_message_from_segments_async(
                workspace_id,
                "user",
                request.user_segments_json.clone(),
            )
            .await?;
        crate::agent::common::emit(
            &on_event,
            AgentEvent::UserMessage {
                message: Box::new(user),
            },
        );

        // 工作区准备 + MCP 全局注册表新鲜度刷新（聊天恒为全局作用域）。
        let workspace = self.session_workspace(workspace_id).await?;
        let directories = self.project_directories.lock().clone();
        let chat_workspace = tokio::task::spawn_blocking({
            let workspace = workspace.clone();
            move || ChatWorkspace::resolve(&workspace, &directories)
        })
        .await
        .map_err(|error| anyhow::anyhow!("解析聊天目录任务失败：{error}"))??;
        self.mcp_registry
            .ensure_recent(&McpScope::Global)
            .await
            .map_err(|error| anyhow::anyhow!("刷新聊天 MCP 状态失败：{error}"))?;

        if !self.is_configured() {
            // 模型配置的唯一权威源是设置中心（dispatcher_settings 表）；env 回退已删除。
            anyhow::bail!("错误：聊天 LLM API Key 未配置。请在设置中心「模型服务」配置后重试。");
        }
        crate::agent::config::validate_provider_completeness(
            &self.specs.chat.api_key,
            &self.specs.chat.api_base,
            &self.specs.chat.model,
        )?;

        self.warm_sub_agent_exposure(workspace_id).await;

        // 工具依赖（构造期快照：会话沙箱、审查输入、凭据、取消信号）。
        let deps = self
            .build_deps(db, workspace_id, &chat_workspace, &request.cancel_rx)
            .await;
        let surface = self.build_surface(&deps, workspace_id).await;

        // 历史：DB 最近若干轮对话（不含 system，逐轮由 preamble 重建），
        // 前插已持久化的滚动摘要（历史级压缩的跨 run 延续）。
        let history = db.load_llm_history_async(workspace_id).await?;
        let history = apply_stored_session_summary(db, workspace_id, history).await?;
        let (messages, message_ids) = chat_history_to_rig_with_ids(history).await;

        // 模型 + 循环钩子。
        let model = PurposeSwitchingModel::from_specs(&self.specs)
            .map_err(|error| anyhow::anyhow!("初始化聊天模型失败：{error}"))?;
        let selection: ModelSelectionHandle = model.selection_handle();
        let summary_model = completions_model(&self.specs.summary)
            .map_err(|error| anyhow::anyhow!("初始化摘要模型失败：{error}"))?;
        let summary = RigSummaryModel {
            model: &summary_model,
            max_tokens: self.specs.summary.max_tokens,
            temperature: self.specs.summary.temperature,
        };

        // 系统提示：静态部分（配置提示/分类/子智能体/MCP/备忘录/工作目录）在本轮
        // 装配期渲染一次；系统时间逐轮重建（G9-17：时间不随 run 陈旧）。
        let base_preamble = format!(
            "{}{}",
            self.base_system_prompt(workspace_id),
            chat_workspace.render_prompt()
        );
        let mut hooks = RigLoopHooks::from_chat_spec(&self.specs.chat);
        hooks.max_iterations = *self.max_tool_iterations.lock();
        hooks.model_selection = Some(selection);
        hooks.default_model_name = self.specs.chat.model.clone();
        hooks.context_window = self.specs.chat.context_window;
        hooks.preamble_for_iteration = Some(Box::new(move |_iteration| {
            Some(format!(
                "{base_preamble}\n\n## 系统时间\n\n当前本地时间：{}",
                crate::agent::prompt::current_local_time()
            ))
        }));

        // 工具回调与统一策略共用同一份运行快照，包括固定目录审查规则。
        let policy = AppToolExecutionPolicy::new(
            db,
            &on_event,
            AppToolPolicyConfig {
                workspace_id: workspace_id.to_string(),
                workspace: workspace.clone(),
                filesystem_roots: deps.extra_allowed_dirs.clone(),
                review: deps.review.clone(),
                cancel_rx: Some(request.cancel_rx.clone()),
                trace: Default::default(),
                tool_timeouts: deps.tool_timeouts.clone(),
                app_handle: self.app_handle.clone(),
            },
        );

        let mut usage_tracker = crate::agent::common::UsageTracker::new();
        run_rig_loop(
            db,
            workspace_id,
            &model,
            messages,
            message_ids,
            &surface,
            &policy,
            Some(&summary),
            &mut hooks,
            &on_event,
            request.cancel_rx.clone(),
            &mut usage_tracker,
        )
        .await
    }
}
