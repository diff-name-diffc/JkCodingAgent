//! 普通聊天 Agent：不绑定项目，按聊天分类配置装配模型槽位与工具面。
//!
//! 每轮 run 由命令层现建现用（设置变更下一轮即生效），本结构只负责装配：
//! 1. 设置 → 用途槽位规格（`resolve_purpose_specs`）→ rig 模型；
//! 2. 工具面：exec + media + MCP（全局作用域）+ 子智能体工具，按允许列表过滤；
//! 3. 系统提示：配置提示 + 分类上下文 + 系统时间 + 子智能体清单 + MCP 清单
//!    + SSH 备忘录纪律 + 运行工作目录块；
//! 4. 历史（DB 最近若干轮对话）→ rig 消息；
//! 5. 运行 `rig_ext::r#loop::run_rig_loop`（三段式策略：审查门禁 + 台账 + 超时）。

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
use crate::agent::rig_ext::events::AgentEvent;
use crate::agent::rig_ext::message::chat_history_to_rig;
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
use crate::agent::rig_ext::tools::deps::{ImageToolConfig, RigToolDeps, ToolCallSlot};
use crate::agent::rig_ext::tools::exec::exec_tools;
use crate::agent::rig_ext::tools::mcp::mcp_tools;
use crate::agent::rig_ext::tools::media::media_tools;
use crate::agent::sub_agent::config::SubAgentConfig;
use crate::agent::sub_agent::SubAgentManager;
use crate::mcp::{tool_definitions_from_snapshot, McpRegistry, McpScope, ResolvedMcpTool};
use crate::ssh_tool::SshSessionManager;

use super::{retain_allowed_tools, session_workspace_dir_name, tool_result_policies_from_specs};

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
    sub_agent_manager: Option<Arc<SubAgentManager>>,
    app_handle: Option<AppHandle>,

    specs: PurposeModelSpecs,
    /// 配置的系统提示（空 → `DEFAULT_PLAIN_CHAT_SYSTEM_PROMPT`）。
    system_prompt: Mutex<String>,
    /// 工具允许列表（内置空列表 = 全部放行；MCP 显式名单制）。
    allowed_tools: Mutex<Vec<String>>,
    category_context: Mutex<Option<(String, String)>>,
    /// 审查配置（settings.review；未配置 → None，命令类工具 fail-closed）。
    review_config: Mutex<Option<crate::agent::db::settings::SshReviewConfig>>,
    image_credentials: Mutex<crate::agent::db::settings::ImageModelCredentials>,
    /// 本 run 会话已启用的子智能体快照（run 入口异步拉取一次）。
    sub_agent_exposure: Mutex<Option<SubAgentExposure>>,
    tool_call_id: ToolCallSlot,
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
        sub_agent_manager: Option<Arc<SubAgentManager>>,
    ) -> Self {
        Self {
            specs: resolve_purpose_specs(&AhaSettingsV2::default(), AgentContext::Chat, &config),
            config,
            mcp_registry,
            ssh_manager,
            sub_agent_manager,
            app_handle: None,
            system_prompt: Mutex::new(DEFAULT_PLAIN_CHAT_SYSTEM_PROMPT.to_string()),
            allowed_tools: Mutex::new(Vec::new()),
            category_context: Mutex::new(None),
            review_config: Mutex::new(None),
            image_credentials: Mutex::new(Default::default()),
            sub_agent_exposure: Mutex::new(None),
            tool_call_id: ToolCallSlot::default(),
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
        let active_chat = ctx_config
            .chat_model_configs
            .iter()
            .find(|c| c.active)
            .or_else(|| ctx_config.chat_model_configs.first());
        if let Some(chat) = active_chat {
            if !chat.system_prompt.trim().is_empty() {
                *self.system_prompt.lock() = chat.system_prompt.trim().to_string();
            }
        }
        *self.allowed_tools.lock() = ctx_config.allowed_tools.clone();
        *self.review_config.lock() = settings
            .review
            .is_configured()
            .then(|| settings.review.clone());
        *self.image_credentials.lock() = settings.shared.image_model_credentials();
        // 基础设置重应用时同步清除分类叠加（对齐旧实现的顺序契约）。
        *self.category_context.lock() = None;
    }

    /// 叠加聊天分类级配置（系统提示 + 允许列表）。
    pub fn apply_category_config(&mut self, config: &ChatCategoryAgentConfig) {
        *self.allowed_tools.lock() = config.allowed_tools.clone();
        *self.system_prompt.lock() = config.system_prompt.clone();
        *self.category_context.lock() =
            Some((config.category_id.clone(), config.category_name.clone()));
    }

    pub fn is_configured(&self) -> bool {
        self.specs.chat.is_configured()
    }

    /// 每个会话独立的文件沙箱：`root_dir/plain-chat-browser/<会话子目录>`。
    pub async fn session_workspace(&self, workspace_id: &str) -> anyhow::Result<PathBuf> {
        let workspace = self
            .config
            .root_dir
            .join("plain-chat-browser")
            .join(session_workspace_dir_name(workspace_id));
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
                self.tool_call_id.clone(),
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
        crate::agent::common::emit(&on_event, AgentEvent::UserMessage { message: user });

        // 工作区准备 + MCP 全局注册表新鲜度刷新（聊天恒为全局作用域）。
        let workspace = self.session_workspace(workspace_id).await?;
        self.mcp_registry
            .ensure_recent(&McpScope::Global)
            .await
            .map_err(|error| anyhow::anyhow!("刷新聊天 MCP 状态失败：{error}"))?;

        if !self.is_configured() {
            anyhow::bail!(
                "错误：聊天 LLM API Key 未配置。请在设置中配置，或设置 DASHSCOPE_API_KEY / OPENAI_API_KEY 环境变量。"
            );
        }
        crate::agent::config::validate_provider_completeness(
            &self.specs.chat.api_key,
            &self.specs.chat.api_base,
            &self.specs.chat.model,
        )?;

        self.warm_sub_agent_exposure(workspace_id).await;

        // 工具依赖（构造期快照：会话沙箱、审查输入、凭据、取消信号）。
        let deps = self
            .build_deps(db, workspace_id, &workspace, &request.cancel_rx)
            .await;
        let surface = self.build_surface(&deps, workspace_id).await;

        // 历史：DB 最近若干轮对话（不含 system，逐轮由 preamble 重建）。
        let history = db.load_llm_history_async(workspace_id).await?;
        let messages = chat_history_to_rig(history).await;

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

        let extra_dirs_for_prompt = chat_image_dir(workspace_id);
        let surface_for_prompt = surface.definitions();
        let has_local_zsh = surface_for_prompt
            .iter()
            .any(|definition| definition.name == "local_zsh");
        // 系统提示：静态部分（配置提示/分类/子智能体/MCP/备忘录/工作目录）在本轮
        // 装配期渲染一次；系统时间逐轮重建（G9-17：时间不随 run 陈旧）。
        let base_preamble = format!(
            "{}{}",
            self.base_system_prompt(workspace_id),
            render_runtime_workspace(&workspace, true, &extra_dirs_for_prompt, has_local_zsh)
        );
        let mut hooks = RigLoopHooks::from_chat_spec(&self.specs.chat);
        hooks.max_iterations = self.config.max_tool_iterations;
        hooks.model_selection = Some(selection);
        hooks.default_model_name = self.specs.chat.model.clone();
        hooks.context_window = self.specs.chat.context_window;
        hooks.preamble_for_iteration = Some(Box::new(move |_iteration| {
            Some(format!(
                "{base_preamble}\n\n## 系统时间\n\n当前本地时间：{}",
                crate::agent::prompt::current_local_time()
            ))
        }));

        let policy = AppToolExecutionPolicy::new(
            db,
            &on_event,
            AppToolPolicyConfig {
                workspace_id: workspace_id.to_string(),
                workspace: workspace.clone(),
                review: self.review_context(db, workspace_id, None).await,
                cancel_rx: Some(request.cancel_rx.clone()),
                trace: Default::default(),
                tool_call_id: self.tool_call_id.clone(),
            },
        );

        let mut usage_tracker = crate::agent::common::UsageTracker::new();
        run_rig_loop(
            db,
            workspace_id,
            &model,
            messages,
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

    /// 构造工具依赖快照。
    async fn build_deps(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        workspace: &std::path::Path,
        cancel_rx: &watch::Receiver<bool>,
    ) -> RigToolDeps {
        RigToolDeps {
            workspace_id: workspace_id.to_string(),
            workspace: workspace.to_path_buf(),
            // 普通聊天没有项目语境：MCP 一律走全局注册表。
            mcp_scope: McpScope::Global,
            exec_timeout_secs: self.config.exec_timeout_secs,
            restrict_to_workspace: true,
            // 只放行当前会话的图片目录（chat-images/{workspace_id}）。
            extra_allowed_dirs: chat_image_dir(workspace_id),
            app_handle: self.app_handle.clone(),
            db: db.clone(),
            ssh_manager: self.ssh_manager.clone(),
            mcp_registry: self.mcp_registry.clone(),
            sub_agent_manager: self.sub_agent_manager.clone(),
            cancel_rx: Some(cancel_rx.clone()),
            vision_spec: self.specs.vision.clone(),
            image: self.image_tool_config(),
            review: self.review_context(db, workspace_id, None).await,
            tool_call_id: self.tool_call_id.clone(),
        }
    }

    fn image_tool_config(&self) -> ImageToolConfig {
        let credentials = self.image_credentials.lock().clone();
        ImageToolConfig {
            url: credentials.url,
            api_key: credentials.api_key,
            model: credentials.model,
            edit_model: credentials.edit_model,
        }
    }

    /// 审查上下文：会话标题 + 用户任务 + 最近对话（读取失败降级为 None）。
    async fn review_context(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        executor_task: Option<String>,
    ) -> RigReviewContext {
        let session_title = db
            .get_session_title_async(workspace_id)
            .await
            .unwrap_or_else(|_| "untitled".to_string());
        let user_task = db
            .get_latest_user_message_content_async(workspace_id)
            .await
            .ok()
            .flatten();
        let review_conversation = match db
            .get_recent_review_dialogue_async(
                workspace_id,
                crate::agent::ssh_review::REVIEW_DIALOGUE_FETCH_LIMIT,
            )
            .await
        {
            Ok(messages) => crate::agent::ssh_review::render_dialogue_for_review(&messages),
            Err(error) => {
                eprintln!(
                    "读取审查对话上下文失败，降级为无对话上下文（workspace_id={workspace_id}）：{error:#}"
                );
                None
            }
        };
        RigReviewContext {
            config: self.review_config.lock().clone(),
            session_title,
            user_task,
            executor_task,
            review_conversation,
        }
    }

    /// 供事件/诊断：本轮渲染后的基础系统提示（未含运行工作目录块）。
    /// 子智能体工具清单（子智能体保存校验与选择列表用）：与
    /// `RigSubAgentRuntime` 实际继承的 execution profile 同源——exec + media
    /// 加子智能体专用 `notify_user_progress`；**不含**嵌套子智能体工具
    /// （call_sub_agent / list_sub_agents 不得递归派生）。
    pub fn sub_agent_tool_catalog(&self) -> Vec<(String, String)> {
        let deps = self.catalog_deps();
        let mut tools = exec_tools(&deps);
        tools.extend(media_tools(&deps));
        tools.push(crate::agent::rig_ext::sub_agent::notify_user_progress_tool(
            "tool-catalog".to_string(),
            "tool-catalog".to_string(),
            "tool-catalog".to_string(),
            None,
            Arc::new(parking_lot::Mutex::new(Vec::new())),
            "tool-catalog".to_string(),
        ));
        let mut infos = tools
            .into_iter()
            .map(|tool| {
                let definition = tool.definition();
                (definition.name, definition.description)
            })
            .collect::<Vec<_>>();
        infos.sort();
        infos.dedup();
        infos
    }

    /// 清单枚举用的占位依赖（只构造不执行）。
    fn catalog_deps(&self) -> RigToolDeps {
        RigToolDeps {
            workspace_id: "tool-catalog".to_string(),
            workspace: self.config.root_dir.clone(),
            mcp_scope: McpScope::Global,
            exec_timeout_secs: self.config.exec_timeout_secs,
            restrict_to_workspace: true,
            extra_allowed_dirs: Vec::new(),
            app_handle: None,
            db: crate::agent::db::DispatcherDb::new(self.config.db_path.clone())
                .expect("打开工具清单用数据库句柄"),
            ssh_manager: self.ssh_manager.clone(),
            mcp_registry: self.mcp_registry.clone(),
            sub_agent_manager: self.sub_agent_manager.clone(),
            cancel_rx: None,
            vision_spec: self.specs.vision.clone(),
            image: self.image_tool_config(),
            review: RigReviewContext::unconfigured(),
            tool_call_id: ToolCallSlot::default(),
        }
    }

    /// 工具清单（设置页/分类允许列表的枚举来源）：按「占位依赖」装配同一份
    /// 工具面并取名称与描述——清单与实际授权集同源，避免两份清单漂移。
    pub async fn tool_catalog(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
    ) -> Vec<crate::agent::sub_agent::db::ToolInfo> {
        let mut deps = self.catalog_deps();
        deps.workspace_id = workspace_id.to_string();
        deps.db = db.clone();
        let surface = self.build_surface(&deps, workspace_id).await;
        surface
            .definitions()
            .into_iter()
            .map(|definition| crate::agent::sub_agent::db::ToolInfo {
                name: definition.name,
                description: definition.description,
            })
            .collect()
    }
}

/// SSH 运维备忘录纪律（仅在备忘录工具可用时注入）。
const SSH_MEMO_GUIDANCE: &str = "\n\n## SSH 运维备忘录\n\n\
- 每台 SSH 服务器有一份运维备忘录，用 ssh_memo_read / ssh_memo_upsert / ssh_memo_delete 读取与更新。\n\
- 对不熟悉的服务器执行运维操作前，先调用 ssh_memo_read 获取部署路径、特殊命令方式与已知问题，避免重复试错。\n\
- 运维中真实遇到并解决的问题、新发现的部署/服务路径、非通用命令与操作方式等长期有效信息，才更新备忘录：先读后写；修正单行或增删个别条目用 ssh_memo_upsert 局部替换（title+old_text+new_text，new_text 留空即删除该片段），较大改动才把合并去重后的内容用 title+content 整段重写；段落内容里不要写 `## ` 开头的行（子标题用 `### `）。\n\
- 克制记录：只写对后续运维必要的信息，不写密码/密钥/令牌等凭据，不写临时调试输出与过程性日志；重复或过时条目主动合并、删除，防止备忘录无限增长；写入被上限拒绝时先精简旧内容再重试。";

/// 会话图片目录（构造失败按空白名单收紧，fail-closed）。
fn chat_image_dir(workspace_id: &str) -> Vec<PathBuf> {
    match crate::chat_images::workspace_image_dir(workspace_id) {
        Ok(dir) => vec![dir],
        Err(error) => {
            eprintln!(
                "构造会话图片目录失败（workspace_id={workspace_id}），按空白名单收紧：{error}"
            );
            Vec::new()
        }
    }
}

/// 运行工作目录与路径权限块（迁移自 `prompt/runtime_workspace.rs::render`）。
pub(crate) fn render_runtime_workspace(
    workspace: &std::path::Path,
    restricted: bool,
    extra: &[PathBuf],
    local_zsh: bool,
) -> String {
    let mut prompt = format!(
        "\n\n## 本次运行的工作目录与路径权限\n\n\
         - 当前文件工作区（绝对路径）：{}\n\
         - 文件工具与 sync_directory.source 的相对路径均以该工作区为基准；exec 默认在该工作区执行。\n",
        workspace.display()
    );
    if restricted {
        prompt.push_str("- 文件路径限制：只允许当前工作区及下列额外授权路径；上级目录、其他项目目录不会自动获得授权。\n");
    } else {
        prompt.push_str("- 工作区边界限制已关闭；应用敏感路径保护与命令安全审查仍生效。\n");
    }
    if extra.is_empty() {
        prompt.push_str("- 额外授权路径：无。\n");
    } else {
        prompt.push_str("- 额外授权路径（目录内或精确文件）：\n");
        for path in extra {
            prompt.push_str(&format!("  - {}\n", path.display()));
        }
    }
    if local_zsh {
        if let Ok(directory) = crate::agent::rig_ext::tools::exec::local_zsh_dir(workspace) {
            prompt.push_str(&format!(
                "- local_zsh 实际命令工作目录：{}。它与文件工作区不同，且不能使用 cd。命令内的相对路径以此目录为基准。\n\
                 - 上传 local_zsh 生成的文件时，使用该执行目录下产物子目录的绝对路径作为 source，不要将相对路径误当成文件工作区下的路径。\n",
                directory.display()
            ));
        }
    }
    prompt.push_str(
        "- sync_directory.destination 是远端目录，不受本地工作区前缀约束；source 必须是本地已有的产物目录。\n\
         - 不要上传整个 .jkcodingagent 配置根目录或 .git 元数据目录；会话工作区和 local_zsh 的产物子目录可按现有路径权限使用。\n\
         - 不要猜测 /Users、用户主目录、/releases 等目录。路径越界时说明当前工作区和所需路径，让用户在对应项目会话操作或将文件放入允许目录；不要换工具绕过限制或重复尝试同一个被拒绝路径。\n"
    );
    prompt
}
