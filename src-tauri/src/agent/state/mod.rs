use anyhow::{anyhow, Context, Result};
use std::path::PathBuf;
use std::sync::Arc;

use super::agents::{OrchestratorAgent, PlainChatAgent};
use super::config::DispatcherAgentConfig;
use super::db::{AgentContext, DispatcherDb};
use super::sub_agent::db::ToolInfo;
use super::sub_agent::SubAgentManager;
use super::tools::ToolRegistry;
use crate::project::mcp::{ensure_project_mcp_file, ProjectMcpRegistry};
use crate::ssh_tool::SshSessionManager;

mod generation;
mod run;
mod tool_catalog;

use generation::GenerationGate;
use run::{ActiveRunHandle, ActiveRunStore, GraphRunRegistry};
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
    project_mcp_registry: ProjectMcpRegistry,
    ssh_manager: SshSessionManager,
    db: DispatcherDb,
    sub_agent_manager: Option<Arc<SubAgentManager>>,
}

impl DispatcherState {
    pub fn new(
        project_mcp_registry: ProjectMcpRegistry,
        ssh_manager: SshSessionManager,
    ) -> Result<Self> {
        let config = DispatcherAgentConfig::load()?;
        let db = DispatcherDb::new(config.db_path.clone())?;
        super::graph::store::GraphStore::new(&db)
            .fail_interrupted_runs(None)
            .context("恢复中断的执行图运行")?;
        let sub_agent_manager = Arc::new(SubAgentManager::new(db.pool()));
        if let Err(e) = sub_agent_manager.load_all() {
            eprintln!("failed to load sub_agent configs: {}", e);
        }

        let initial_tool_registry =
            ToolRegistry::default_tools(project_mcp_registry.clone(), ssh_manager.clone());
        let initial_tool_names = initial_tool_registry.tool_names_and_descriptions();

        Ok(Self {
            services: AgentServices {
                config,
                project_mcp_registry,
                ssh_manager,
                db,
                sub_agent_manager: Some(sub_agent_manager),
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

    pub(crate) fn project_mcp_registry(&self) -> ProjectMcpRegistry {
        self.services.project_mcp_registry.clone()
    }

    pub(crate) fn ssh_manager(&self) -> SshSessionManager {
        self.services.ssh_manager.clone()
    }

    pub fn registered_tool_names(&self) -> Option<Vec<(String, String)>> {
        self.tools.registered_tool_names()
    }

    /// 每轮构建一个新的 OrchestratorAgent（短命对象）并从 DB 实时应用设置。
    /// 这是"每轮现建现用"模式的核心——保证设置变更在下一轮立即生效，
    /// 而当前轮次内部保持一致。
    pub(crate) fn build_run_agent(&self) -> OrchestratorAgent {
        let mut agent = OrchestratorAgent::new(
            self.services.config.clone(),
            self.services.sub_agent_manager.clone(),
        );

        let settings = self
            .services
            .db
            .get_settings_v2()
            .expect("load dispatcher settings v2");
        agent.apply_settings_v2(&settings, AgentContext::Project);
        agent.set_context_debug(settings.context_debug);

        agent
    }

    /// 构建聊天模式的 PlainChatAgent，并叠加该聊天会话的分类级配置
    /// （每个聊天可独立配置系统提示与工具集）。
    pub(crate) fn build_plain_chat_agent(
        &self,
        workspace_id: &str,
    ) -> std::result::Result<PlainChatAgent, String> {
        let agent = PlainChatAgent::new(
            self.services.config.clone(),
            self.services.project_mcp_registry.clone(),
            self.services.ssh_manager.clone(),
            self.services.sub_agent_manager.clone(),
        );

        let settings = self
            .services
            .db
            .get_settings_v2()
            .map_err(|error| error.to_string())?;
        agent.apply_settings_v2(&settings, AgentContext::Chat);

        if let Some(category_config) = self
            .services
            .db
            .get_chat_session_category_agent_config(workspace_id)
            .map_err(|error| error.to_string())?
        {
            agent.apply_category_config(&category_config);
        }

        Ok(agent)
    }

    pub(crate) async fn list_agent_tools(
        &self,
        context: AgentContext,
        project_path: Option<String>,
    ) -> std::result::Result<Vec<ToolInfo>, String> {
        let (workspace, registry) = match context {
            AgentContext::Project => {
                let Some(project_path) = project_path.filter(|path| !path.trim().is_empty()) else {
                    let mut registry = ToolRegistry::default_tools(
                        self.services.project_mcp_registry.clone(),
                        self.services.ssh_manager.clone(),
                    );
                    self.add_sub_agent_tools(&mut registry);
                    return Ok(tool_infos_from_registry(&registry, None, false));
                };
                let workspace = PathBuf::from(project_path);
                self.services
                    .project_mcp_registry
                    .ensure_recent(&workspace)
                    .await?;
                let mut registry = ToolRegistry::default_tools(
                    self.services.project_mcp_registry.clone(),
                    self.services.ssh_manager.clone(),
                );
                self.add_sub_agent_tools(&mut registry);
                (workspace, registry)
            }
            AgentContext::Chat => {
                let workspace = plain_chat_workspace(&self.services.config)
                    .await
                    .map_err(|error| error.to_string())?;
                self.services
                    .project_mcp_registry
                    .ensure_recent(&workspace)
                    .await?;
                let mut registry = ToolRegistry::plain_chat_tools(
                    self.services.project_mcp_registry.clone(),
                    self.services.ssh_manager.clone(),
                );
                self.add_sub_agent_tools(&mut registry);
                (workspace, registry)
            }
        };

        Ok(tool_infos_from_registry(&registry, Some(&workspace), true))
    }

    pub(crate) async fn ssh_workspace_for_context(
        &self,
        context: AgentContext,
        project_path: Option<String>,
    ) -> std::result::Result<PathBuf, String> {
        match context {
            AgentContext::Project => project_path
                .filter(|path| !path.trim().is_empty())
                .map(PathBuf::from)
                .ok_or_else(|| "项目 SSH 配置需要项目路径".to_string()),
            AgentContext::Chat => plain_chat_workspace(&self.services.config)
                .await
                .map_err(|error| error.to_string()),
        }
    }

    fn add_sub_agent_tools(&self, registry: &mut ToolRegistry) {
        if let Some(manager) = &self.services.sub_agent_manager {
            registry.add_tool(Box::new(super::sub_agent::SubAgentTool::new(Arc::clone(
                manager,
            ))));
            registry.add_tool(Box::new(super::sub_agent::ListSubAgentsTool::new(
                Arc::clone(manager),
            )));
        }
    }

    pub(crate) fn begin_run(&self, workspace_id: &str) -> Result<ActiveRunHandle, String> {
        self.active_runs.begin(workspace_id)
    }

    pub(crate) fn finish_run(&self, workspace_id: &str) {
        self.active_runs.finish(workspace_id);
    }

    pub(crate) fn stop_run(&self, workspace_id: &str) -> bool {
        self.active_runs.stop(workspace_id)
    }

    pub(crate) fn begin_graph_run(
        &self,
        plan_id: &str,
    ) -> std::result::Result<tokio::sync::watch::Receiver<bool>, String> {
        self.graph_runs.begin(plan_id)
    }

    pub(crate) fn finish_graph_run(&self, plan_id: &str) {
        self.graph_runs.finish(plan_id);
    }

    pub(crate) fn cancel_graph_run(&self, plan_id: &str) -> bool {
        self.graph_runs.cancel(plan_id)
    }

    pub(crate) fn begin_title_generation(&self, workspace_id: &str) -> u64 {
        self.title_generations.begin(workspace_id)
    }

    pub(crate) fn finish_latest_title_generation(
        &self,
        workspace_id: &str,
        generation: u64,
    ) -> bool {
        self.title_generations
            .finish_latest(workspace_id, generation)
    }

    pub(crate) fn begin_keywords_generation(&self, workspace_id: &str) -> u64 {
        self.keywords_generations.begin(workspace_id)
    }

    pub(crate) fn finish_latest_keywords_generation(
        &self,
        workspace_id: &str,
        generation: u64,
    ) -> bool {
        self.keywords_generations
            .finish_latest(workspace_id, generation)
    }
}

async fn plain_chat_workspace(config: &DispatcherAgentConfig) -> Result<PathBuf> {
    let workspace = config.root_dir.join("plain-chat-browser");
    let workspace_for_init = workspace.clone();
    tokio::task::spawn_blocking(move || {
        let config_dir = workspace_for_init.join(".jkcodingagent");
        std::fs::create_dir_all(&config_dir)
            .with_context(|| format!("create {}", config_dir.display()))?;
        ensure_project_mcp_file(&workspace_for_init.to_string_lossy()).map_err(anyhow::Error::msg)
    })
    .await
    .map_err(|error| anyhow!("create plain chat workspace task failed: {error}"))??;
    Ok(workspace)
}
