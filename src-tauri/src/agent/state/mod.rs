use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::Arc;

use super::agents::{OrchestratorAgent, PlainChatAgent};
use super::config::DispatcherAgentConfig;
use super::db::{AgentContext, AhaSettingsV2, ChatCategoryAgentConfig, DispatcherDb};
use super::sub_agent::db::ToolInfo;
use super::sub_agent::SubAgentManager;
use super::tools::ToolRegistry;
use crate::mcp::McpRegistry;
use crate::ssh_tool::SshSessionManager;

mod generation;
mod run;
mod tool_catalog;

use generation::GenerationGate;
pub(crate) use generation::GenerationGuard;
use run::{ActiveRunHandle, ActiveRunStore, GraphRunRegistry};

pub(crate) use run::GraphRunHandle;
use tool_catalog::{tool_infos_from_registry, ToolCatalog};

/// 应用级状态聚合器，由 Tauri `.manage()` 托管，是整个调度智能体的长寿宿主。
///
/// 与短命的 `OrchestratorAgent`（每轮 run 重建）相对，DispatcherState 持有跨轮次、
/// 跨会话共享的资源：DB 连接池、并发控制、工具目录缓存等。
///
/// 职责：
/// - 初始化并持有所有共享服务（DB / MCP / SSH / 子智能体管理器）。
/// - `build_run_agent` / `build_plain_chat_agent`：每轮按需构建短命 Agent 并实时应用 DB 设置。
/// - 运行状态管理（同一 workspace 禁止重入）与异步生成的"最新代胜出"机制。
///
/// 构造（G11-02）：`new` 为 async——配置初始化、DB 打开/迁移、中断运行恢复、
/// 子智能体配置加载等阻塞 I/O 全部在 spawn_blocking 中执行；失败返回带上下文
/// 的 Err 由调用方展示可读错误，而不是在启动路径上 panic。
pub struct DispatcherState {
    services: AgentServices,
    active_runs: ActiveRunStore,
    graph_runs: GraphRunRegistry,
    title_generations: GenerationGate,
    keywords_generations: GenerationGate,
    tools: ToolCatalog,
}

/// 跨轮次共享的基础服务集合。单独抽出便于构造时一次性初始化。
struct AgentServices {
    config: DispatcherAgentConfig,
    mcp_registry: McpRegistry,
    ssh_manager: SshSessionManager,
    db: DispatcherDb,
    sub_agent_manager: Option<Arc<SubAgentManager>>,
}

impl DispatcherState {
    pub async fn new() -> Result<Self> {
        let config = tokio::task::spawn_blocking(DispatcherAgentConfig::load)
            .await
            .context("等待智能体配置初始化任务失败")?
            .context("加载智能体配置失败")?;

        let db_path = config.db_path.clone();
        let (db, sub_agent_manager) = tokio::task::spawn_blocking(
            move || -> Result<(DispatcherDb, Option<Arc<SubAgentManager>>)> {
                let db = DispatcherDb::new(db_path).context("打开本地数据库失败")?;
                super::graph::store::GraphStore::new(&db)
                    .fail_interrupted_runs(None)
                    .context("恢复中断的执行图运行")?;
                // G11-04：子智能体配置加载失败时显式降级为 None（禁用子智能体
                // 能力并在 UI 可见），而不是带着不完整状态继续静默运行。
                let manager = Arc::new(SubAgentManager::new(db.pool()));
                let manager = match manager.load_all() {
                    Ok(_) => Some(manager),
                    Err(error) => {
                        eprintln!("子智能体配置加载失败，已禁用子智能体能力：{error:#}");
                        None
                    }
                };
                Ok((db, manager))
            },
        )
        .await
        .context("等待数据库初始化任务失败")??;

        // SSH 配置已收敛为全局 SQLite 权威源：管理器直接共享 DispatcherDb
        // 连接池，配置 / 主机密钥 / 审计全部读写全局库。
        let ssh_manager = SshSessionManager::new(db.pool());

        // MCP 全局注册表以全局库为唯一权威源：构造期强制持有 DB，
        // refresh 时按「全局 ∪ 项目文件（同名覆盖）」合并。
        let mcp_registry = McpRegistry::new(db.clone());

        // G11-07：初始工具目录即包含子智能体工具
        //（修复旧实现缺失 call_sub_agent / list_sub_agents 的问题）。
        let mut initial_tool_registry =
            ToolRegistry::default_tools(mcp_registry.clone(), ssh_manager.clone());
        if let Some(manager) = &sub_agent_manager {
            register_sub_agent_tools(manager, &mut initial_tool_registry);
        }
        let initial_tool_names = initial_tool_registry.tool_names_and_descriptions();

        Ok(Self {
            services: AgentServices {
                config,
                mcp_registry,
                ssh_manager,
                db,
                sub_agent_manager,
            },
            active_runs: ActiveRunStore::default(),
            graph_runs: GraphRunRegistry::default(),
            title_generations: GenerationGate::default(),
            keywords_generations: GenerationGate::default(),
            tools: ToolCatalog::new(initial_tool_names),
        })
    }

    pub(crate) fn db(&self) -> &DispatcherDb {
        &self.services.db
    }

    pub fn sub_agent_manager(&self) -> Option<Arc<SubAgentManager>> {
        self.services.sub_agent_manager.clone()
    }

    pub(crate) fn agent_config(&self) -> DispatcherAgentConfig {
        self.services.config.clone()
    }

    pub(crate) fn mcp_registry(&self) -> McpRegistry {
        self.services.mcp_registry.clone()
    }

    pub(crate) fn ssh_manager(&self) -> SshSessionManager {
        self.services.ssh_manager.clone()
    }

    /// 子智能体工具选择清单。它必须与 SubAgentRuntime 实际继承的普通聊天
    /// execution profile 一致；项目编排器工具和嵌套子智能体工具都不能混入。
    ///
    /// G11-07：每次读取重建并 refresh 缓存——子智能体配置变更点在
    /// sub_agent/commands.rs（本模块边界外，无法在变更后主动触发刷新），
    /// 而工具枚举为纯内存操作、成本可忽略，读取即重建从机制上消除缓存陈旧。
    pub fn registered_tool_names(&self) -> Option<Vec<(String, String)>> {
        let registry = ToolRegistry::plain_chat_tools(
            self.services.mcp_registry.clone(),
            self.services.ssh_manager.clone(),
        );
        let names = registry.tool_names_and_descriptions();
        self.tools.refresh(names);
        self.tools.registered_tool_names()
    }

    /// 每轮构建一个新的 OrchestratorAgent（短命对象）并从 DB 实时应用设置。
    /// 这是"每轮现建现用"模式的核心——保证设置变更在下一轮立即生效，
    /// 而当前轮次内部保持一致。
    ///
    /// G11-01 / G7-06：DB 读取放入阻塞线程池执行；失败以 Result 透传可读错误，
    /// 不再 expect panic 导致 async 命令崩溃。
    pub(crate) async fn build_run_agent(&self) -> std::result::Result<OrchestratorAgent, String> {
        let mut agent =
            OrchestratorAgent::new(self.services.config.clone(), self.services.db.clone());

        let db = self.services.db.clone();
        let settings = tokio::task::spawn_blocking(move || db.get_settings_v2())
            .await
            .map_err(|error| format!("错误：加载调度设置任务失败：{error}"))?
            .map_err(|error| format!("错误：加载调度设置失败：{error}"))?;
        agent.apply_settings_v2(&settings, AgentContext::Project);
        agent.set_context_debug(settings.context_debug);

        Ok(agent)
    }

    /// 构建聊天模式的 PlainChatAgent，并叠加该聊天会话的分类级配置
    /// （每个聊天可独立配置系统提示与工具集）。
    ///
    /// G7-06：本方法原为同步 fn 却在 async 链上直接执行 DB 查询，
    /// 现改为 async 并将 DB 读取包入 spawn_blocking。
    pub(crate) async fn build_plain_chat_agent(
        &self,
        workspace_id: &str,
    ) -> std::result::Result<PlainChatAgent, String> {
        let agent = PlainChatAgent::new(
            self.services.config.clone(),
            self.services.mcp_registry.clone(),
            self.services.ssh_manager.clone(),
            self.services.sub_agent_manager.clone(),
        );

        let db = self.services.db.clone();
        let session_id = workspace_id.to_string();
        let (settings, category_config) = tokio::task::spawn_blocking(
            move || -> Result<(AhaSettingsV2, Option<ChatCategoryAgentConfig>)> {
                let settings = db.get_settings_v2()?;
                let category_config = db.get_chat_session_category_agent_config(&session_id)?;
                Ok((settings, category_config))
            },
        )
        .await
        .map_err(|error| format!("错误：加载聊天设置任务失败：{error}"))?
        .map_err(|error| format!("错误：加载聊天设置失败：{error}"))?;

        agent.apply_settings_v2(&settings, AgentContext::Chat);
        if let Some(category_config) = category_config {
            agent.apply_category_config(&category_config);
        }

        Ok(agent)
    }

    pub(crate) async fn list_agent_tools(
        &self,
        context: AgentContext,
    ) -> std::result::Result<Vec<ToolInfo>, String> {
        // 工具清单服务于设置/分类的允许列表配置，只枚举内置工具：
        // 动态（MCP）工具清单由 `mcp_global_status`（默认复用新鲜窗口缓存）单独提供
        // （前端按「普通工具 / MCP 工具」两个折叠区分别渲染）。MCP 工具
        // 同样按分类允许列表显式名单制门禁（见 PlainChatAgent 的定义层过滤），
        // 服务器级启停则在 MCP 注册表层（设置中心全局页与项目启停开关）。
        let chat_mode = matches!(context, AgentContext::Chat);
        self.enumerate_tools(chat_mode).await
    }

    /// 构建注册表并枚举内置工具信息。G11-06：整个同步枚举过程（注册表构建、
    /// schema 构建、排序去重）放入阻塞线程池，不占用 async 执行器线程。
    async fn enumerate_tools(&self, chat_mode: bool) -> std::result::Result<Vec<ToolInfo>, String> {
        let mcp_registry = self.services.mcp_registry.clone();
        let ssh_manager = self.services.ssh_manager.clone();
        let sub_agent_manager = self.services.sub_agent_manager.clone();
        tokio::task::spawn_blocking(move || {
            let mut registry = if chat_mode {
                ToolRegistry::plain_chat_tools(mcp_registry, ssh_manager)
            } else {
                ToolRegistry::orchestrator_tools()
            };
            if chat_mode {
                if let Some(manager) = sub_agent_manager {
                    register_sub_agent_tools(&manager, &mut registry);
                }
            }
            let mut tools = tool_infos_from_registry(&registry, None, false);
            if !chat_mode {
                tools.retain(|tool| {
                    crate::agent::tools::ORCHESTRATOR_RUNTIME_TOOL_NAMES
                        .contains(&tool.name.as_str())
                });
            }
            tools
        })
        .await
        .map_err(|error| format!("错误：枚举工具列表任务失败：{error}"))
    }

    /// G11-03：校验前端传入的项目路径。
    /// 1) canonicalize（解析符号链接与 ..），路径不存在直接报错；
    /// 2) 包含校验：必须位于应用数据目录（plain-chat 等工作区）之下，
    ///    或是已注册项目的路径——拒绝其他一切越权路径，
    ///    防止借 project_path 探测/操作任意目录。
    pub(crate) async fn validate_project_workspace(
        &self,
        project_path: &str,
    ) -> std::result::Result<PathBuf, String> {
        let raw = PathBuf::from(project_path.trim());
        let root_dir = self.services.config.root_dir.clone();
        let db = self.services.db.clone();
        tokio::task::spawn_blocking(move || -> std::result::Result<PathBuf, String> {
            let canonical = raw
                .canonicalize()
                .map_err(|error| format!("错误：项目路径无效或不存在：{error}"))?;
            if canonical.starts_with(&root_dir) {
                return Ok(canonical);
            }
            let projects = db
                .list_projects()
                .map_err(|error| format!("错误：加载项目列表失败：{error}"))?;
            let registered = projects.iter().any(|project| {
                let registered_path = PathBuf::from(&project.path);
                registered_path == canonical
                    || registered_path
                        .canonicalize()
                        .ok()
                        .is_some_and(|resolved| resolved == canonical)
            });
            if registered {
                Ok(canonical)
            } else {
                Err("错误：项目路径不在已注册的工作区范围内".to_string())
            }
        })
        .await
        .map_err(|error| format!("错误：校验项目路径任务失败：{error}"))?
    }

    pub(crate) fn begin_run(&self, workspace_id: &str) -> Result<ActiveRunHandle, String> {
        self.active_runs.begin(workspace_id)
    }

    /// 显式结束 run（消费句柄）。句柄本身是 RAII 的——panic/提前 return
    /// 路径丢弃句柄同样会清理注册条目，本方法只是显式入口。
    pub(crate) fn finish_run(&self, handle: ActiveRunHandle) {
        self.active_runs.finish(handle);
    }

    pub(crate) fn stop_run(&self, workspace_id: &str) -> bool {
        self.active_runs.stop(workspace_id)
    }

    pub(crate) fn begin_graph_run(
        &self,
        plan_id: &str,
    ) -> std::result::Result<GraphRunHandle, String> {
        self.graph_runs.begin(plan_id)
    }

    pub(crate) fn finish_graph_run(&self, plan_id: &str) {
        self.graph_runs.finish(plan_id);
    }

    pub(crate) fn cancel_graph_run(&self, plan_id: &str) -> bool {
        self.graph_runs.cancel(plan_id)
    }

    /// 恢复暂停中（高危写检查点）的图运行。
    pub(crate) fn resume_graph_run(&self, plan_id: &str) -> bool {
        self.graph_runs.resume(plan_id)
    }

    /// 开始新一代标题生成，返回代际守卫（G11-13：守卫 Drop 自动结算条目）。
    pub(crate) fn begin_title_generation(&self, workspace_id: &str) -> GenerationGuard {
        self.title_generations.begin(workspace_id)
    }

    /// 开始新一代关键字生成，返回代际守卫（G11-13：守卫 Drop 自动结算条目）。
    pub(crate) fn begin_keywords_generation(&self, workspace_id: &str) -> GenerationGuard {
        self.keywords_generations.begin(workspace_id)
    }
}

fn register_sub_agent_tools(manager: &Arc<SubAgentManager>, registry: &mut ToolRegistry) {
    registry.add_tool(Box::new(super::sub_agent::SubAgentTool::new(Arc::clone(
        manager,
    ))));
    registry.add_tool(Box::new(super::sub_agent::ListSubAgentsTool::new(
        Arc::clone(manager),
    )));
}
