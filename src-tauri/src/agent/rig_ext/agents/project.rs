//! 项目编排 Agent（rig 形态）：`OrchestratorAgent` 的替代实现。
//!
//! 职责不变：用固定只读能力探索项目，核心产物是执行图（DAG）——通过
//! `submit_graph` 协议工具提交，经校验后落 `graph_plans` 并等待用户确认；
//! 图执行由 `agent::graph::runner` 承担。模型可见工具仅四个入口
//! （run_tool_program / message / submit_graph / graph_plan_report），
//! 只读数据面（read_file/list_dir/glob/grep）由 ToolProgram 内部代理。

use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;
use rig::tool::PortableDynamicTool;
use serde_json::Value;
use tauri::ipc::Channel;
use tauri::AppHandle;
use tokio::sync::watch;

use super::project_prompt::{build_iteration_system_prompt, build_static_prompt, log_warning};
use super::project_tools::{
    graph_plan_report_shell, message_shell, submit_graph_shell, ORCHESTRATOR_PROTOCOL_TOOL_NAMES,
};
use crate::agent::config::DispatcherAgentConfig;
use crate::agent::db::{
    AgentContext, AhaSettingsV2, DispatcherDb, DispatcherMessageRecord,
};
use crate::agent::rig_ext::message::chat_history_to_rig;
use crate::agent::rig_ext::model::{
    completions_model, resolve_purpose_specs, PurposeModelSpecs, PurposeSwitchingModel,
};
use crate::agent::rig_ext::r#loop::{
    run_rig_loop, AppToolExecutionPolicy, AppToolPolicyConfig, ProtocolToolHandler, RigLoopHooks,
    RigProtocolAction, RigProtocolResult, RigToolSurface,
};
use crate::agent::rig_ext::review::RigReviewContext;
use crate::agent::rig_ext::tool_result::RigSummaryModel;
use crate::agent::rig_ext::tools::deps::{ImageToolConfig, RigToolDeps, ToolCallSlot};
use crate::agent::rig_ext::tools::fs::fs_tools;
use crate::agent::rig_ext::tools::program::program_tool;
use crate::agent::run_loop::AgentEvent;
use crate::agent::tools::ORCHESTRATOR_RUNTIME_TOOL_NAMES;
use crate::mcp::McpScope;

/// 一轮项目编排的输入。
pub struct OrchestratorTurnRequest<'a> {
    pub db: &'a DispatcherDb,
    pub workspace_id: &'a str,
    pub project_path: &'a str,
    pub user_segments_json: String,
    pub on_event: Channel<AgentEvent>,
    pub cancel_rx: watch::Receiver<bool>,
}

pub struct RigOrchestratorAgent {
    config: DispatcherAgentConfig,
    db: DispatcherDb,
    app_handle: Option<AppHandle>,
    specs: PurposeModelSpecs,
    /// 仅控制 ToolProgram 可代理的数据面能力（空 = 全部固定只读能力）。
    allowed_runtime_tools: Vec<String>,
    context_debug: bool,
    review_config: Option<crate::agent::db::settings::SshReviewConfig>,
    image_credentials: crate::agent::db::settings::ImageModelCredentials,
}

impl RigOrchestratorAgent {
    pub fn new(config: DispatcherAgentConfig, db: DispatcherDb) -> Self {
        let specs = resolve_purpose_specs(&AhaSettingsV2::default(), AgentContext::Project, &config);
        Self {
            config,
            db,
            app_handle: None,
            specs,
            allowed_runtime_tools: Vec::new(),
            context_debug: false,
            review_config: None,
            image_credentials: Default::default(),
        }
    }

    pub fn with_app_handle(mut self, app_handle: AppHandle) -> Self {
        self.app_handle = Some(app_handle);
        self
    }

    pub fn apply_settings_v2(&mut self, settings: &AhaSettingsV2, context: AgentContext) {
        self.specs = resolve_purpose_specs(settings, context, &self.config);
        let ctx_config = match context {
            AgentContext::Project => &settings.project,
            AgentContext::Chat => &settings.chat,
        };
        self.allowed_runtime_tools = ctx_config.allowed_tools.clone();
        self.review_config = settings
            .review
            .is_configured()
            .then(|| settings.review.clone());
        self.image_credentials = settings.shared.image_model_credentials();
    }

    pub fn set_context_debug(&mut self, value: bool) {
        self.context_debug = value;
    }

    pub fn context_debug_enabled(&self) -> bool {
        self.context_debug
    }

    pub fn is_configured(&self) -> bool {
        self.specs.chat.is_configured()
    }

    /// 执行一轮编排：落库用户消息 → 工作区校验 → 装配（提示词/工具面/模型）→
    /// 跑 rig 循环（协议工具由 `RigOrchestratorProtocol` 拦截）。
    pub async fn run_turn(
        &self,
        request: OrchestratorTurnRequest<'_>,
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

        // 工作区边界校验（canonicalize + 受管项目成员校验）+ 建目录。
        let workspace = validate_project_workspace(db, request.project_path).await?;
        let workspace_for_create = workspace.clone();
        tokio::task::spawn_blocking(move || std::fs::create_dir_all(&workspace_for_create))
            .await
            .map_err(|error| anyhow::anyhow!("创建项目工作区任务失败：{error}"))?
            .map_err(|error| anyhow::anyhow!("创建项目工作区失败：{error}"))?;

        if !self.is_configured() {
            anyhow::bail!(
                "错误：项目编排 Agent 的 LLM API Key 未配置。请在设置中配置，或设置 DASHSCOPE_API_KEY / OPENAI_API_KEY 环境变量。"
            );
        }
        crate::agent::config::validate_provider_completeness(
            &self.specs.chat.api_key,
            &self.specs.chat.api_base,
            &self.specs.chat.model,
        )?;

        // 静态提示词 + Harness 目录（含既往节点运行统计的轻量学习回路）。
        let mut static_prompt = build_static_prompt(&self.config.root_dir).await?;
        let catalog = crate::agent::graph::harness::build_harness_catalog();
        let stats = match crate::agent::graph::GraphStore::new(db)
            .node_run_stats_async(workspace_id)
            .await
        {
            Ok(stats) => stats,
            Err(error) => {
                log_warning(&format!(
                    "[graph] 读取节点运行统计失败（{workspace_id}），目录回注不含历史统计：{error:#}"
                ));
                Vec::new()
            }
        };
        static_prompt.push_str("\n\n---\n\n");
        static_prompt.push_str(&super::project_prompt::render_graph_harness_catalog(
            &catalog, &stats,
        ));

        // 工具依赖（项目工作区 + 项目作用域 MCP + 审查/图像凭据）。
        let deps = self.build_deps(workspace_id, &workspace, &request.cancel_rx);
        let surface = self.build_surface(&deps, &static_prompt);
        let definitions = surface.definitions();

        // 历史（不含 system）：系统提示逐轮由 preamble 重建。
        let history = db.load_llm_history_async(workspace_id).await?;
        let messages = chat_history_to_rig(history).await;

        let model = PurposeSwitchingModel::from_specs(&self.specs)
            .map_err(|error| anyhow::anyhow!("初始化编排模型失败：{error}"))?;
        let selection = model.selection_handle();
        let summary_model = completions_model(&self.specs.summary)
            .map_err(|error| anyhow::anyhow!("初始化摘要模型失败：{error}"))?;
        let summary = RigSummaryModel {
            model: &summary_model,
            model_name: &self.specs.summary.model,
            max_tokens: self.specs.summary.max_tokens,
            temperature: self.specs.summary.temperature,
        };

        let extra_dirs = chat_image_dir(workspace_id);
        let has_local_zsh = definitions.iter().any(|definition| definition.name == "local_zsh");
        let base_preamble = format!(
            "{}{}",
            build_iteration_system_prompt(&static_prompt, &definitions),
            super::plain_chat::render_runtime_workspace(
                &workspace,
                self.config.restrict_to_workspace,
                &extra_dirs,
                has_local_zsh,
            )
        );
        let mut hooks = RigLoopHooks::from_chat_spec(&self.specs.chat);
        hooks.max_iterations = self.config.max_tool_iterations;
        hooks.model_selection = Some(selection);
        hooks.default_model_name = self.specs.chat.model.clone();
        hooks.context_window = self.specs.chat.context_window;
        hooks.max_iterations_error = Some(format!(
            "已达到最大工具迭代次数（{}），本轮编排被终止。请检查模型是否陷入工具调用循环。",
            self.config.max_tool_iterations
        ));
        hooks.preamble_for_iteration = Some(Box::new(move |_iteration| {
            Some(format!(
                "{base_preamble}\n\n---\n\n# 系统时间\n\n当前本地时间：{}",
                crate::agent::prompt::current_local_time()
            ))
        }));
        hooks.protocol_handler = Some(Arc::new(RigOrchestratorProtocol {
            db: db.clone(),
            workspace_id: workspace_id.to_string(),
            app_handle: self.app_handle.clone(),
            submitted: Mutex::new(false),
        }));

        let policy = AppToolExecutionPolicy::new(
            db,
            &on_event,
            AppToolPolicyConfig {
                workspace_id: workspace_id.to_string(),
                workspace: workspace.clone(),
                review: self.review_context(db, workspace_id).await,
                cancel_rx: Some(request.cancel_rx.clone()),
                trace: Default::default(),
                tool_call_id: ToolCallSlot::default(),
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

    /// 模型可见工具面：四个入口（run_tool_program + 三个协议壳）。
    fn build_surface(&self, deps: &RigToolDeps, _static_prompt: &str) -> RigToolSurface {
        let configured = &self.allowed_runtime_tools;
        let granted = ORCHESTRATOR_RUNTIME_TOOL_NAMES
            .into_iter()
            .filter(|name| configured.is_empty() || configured.iter().any(|item| item == name))
            .collect::<Vec<_>>();
        let data_plane = fs_tools(deps)
            .into_iter()
            .filter(|tool| granted.contains(&tool.name()))
            .collect::<Vec<PortableDynamicTool>>();
        let granted_note = if granted.is_empty() {
            "（无；当前设置禁止全部数据面能力）".to_string()
        } else {
            granted
                .iter()
                .map(|name| format!("`{name}`"))
                .collect::<Vec<_>>()
                .join("、")
        };

        let tools = vec![
            program_tool(deps, data_plane, Some(granted_note)),
            message_shell(),
            submit_graph_shell(),
            graph_plan_report_shell(),
        ];
        debug_assert_eq!(tools.len(), ORCHESTRATOR_PROTOCOL_TOOL_NAMES.len());
        RigToolSurface::new(tools)
    }

    /// 数据面能力清单（设置页 `settings.project.allowed_tools` 的枚举对象）：
    /// 与运行期 grant 同源，只构造不执行。
    pub fn static_runtime_tool_catalog(
        &self,
    ) -> Vec<crate::agent::sub_agent::db::ToolInfo> {
        let deps = self.catalog_deps();
        fs_tools(&deps)
            .into_iter()
            .map(|tool| {
                let definition = tool.definition();
                crate::agent::sub_agent::db::ToolInfo {
                    name: definition.name,
                    description: definition.description,
                }
            })
            .collect()
    }

    /// 清单枚举用的占位依赖（只构造不执行）。
    fn catalog_deps(&self) -> RigToolDeps {
        RigToolDeps {
            workspace_id: "tool-catalog".to_string(),
            workspace: self.config.root_dir.clone(),
            mcp_scope: McpScope::Project(self.config.root_dir.clone()),
            exec_timeout_secs: self.config.exec_timeout_secs,
            restrict_to_workspace: true,
            extra_allowed_dirs: Vec::new(),
            app_handle: None,
            db: self.db.clone(),
            ssh_manager: crate::ssh_tool::SshSessionManager::new(self.db.pool()),
            mcp_registry: crate::mcp::McpRegistry::new(self.db.clone()),
            sub_agent_manager: None,
            cancel_rx: None,
            vision_spec: self.specs.vision.clone(),
            image: ImageToolConfig {
                url: self.image_credentials.url.clone(),
                api_key: self.image_credentials.api_key.clone(),
                model: self.image_credentials.model.clone(),
                edit_model: self.image_credentials.edit_model.clone(),
            },
            review: RigReviewContext::unconfigured(),
            tool_call_id: ToolCallSlot::default(),
        }
    }

    fn build_deps(
        &self,
        workspace_id: &str,
        workspace: &std::path::Path,
        cancel_rx: &watch::Receiver<bool>,
    ) -> RigToolDeps {
        RigToolDeps {
            workspace_id: workspace_id.to_string(),
            workspace: workspace.to_path_buf(),
            // 项目编排器运行在受管项目工作区：MCP 走「全局 ∪ 项目」合并作用域。
            mcp_scope: McpScope::Project(workspace.to_path_buf()),
            exec_timeout_secs: self.config.exec_timeout_secs,
            restrict_to_workspace: self.config.restrict_to_workspace,
            extra_allowed_dirs: chat_image_dir(workspace_id),
            app_handle: self.app_handle.clone(),
            db: self.db.clone(),
            // 编排器无 SSH / MCP 工具面（模型只见四个入口），依赖仅为构造完备性。
            ssh_manager: crate::ssh_tool::SshSessionManager::new(self.db.pool()),
            mcp_registry: crate::mcp::McpRegistry::new(self.db.clone()),
            sub_agent_manager: None,
            cancel_rx: Some(cancel_rx.clone()),
            vision_spec: self.specs.vision.clone(),
            image: ImageToolConfig {
                url: self.image_credentials.url.clone(),
                api_key: self.image_credentials.api_key.clone(),
                model: self.image_credentials.model.clone(),
                edit_model: self.image_credentials.edit_model.clone(),
            },
            review: RigReviewContext {
                config: self.review_config.clone(),
                session_title: String::new(),
                user_task: None,
                executor_task: None,
                review_conversation: None,
            },
            tool_call_id: ToolCallSlot::default(),
        }
    }

    async fn review_context(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
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
        RigReviewContext {
            config: self.review_config.clone(),
            session_title,
            user_task,
            executor_task: None,
            review_conversation: None,
        }
    }
}

/// 编排器协议处理器：submit_graph / graph_plan_report / message 的宿主侧动作。
struct RigOrchestratorProtocol {
    db: DispatcherDb,
    workspace_id: String,
    app_handle: Option<AppHandle>,
    /// 每轮最多提交一次执行图。
    submitted: Mutex<bool>,
}

#[async_trait::async_trait]
impl ProtocolToolHandler for RigOrchestratorProtocol {
    async fn handle(&self, tool_name: &str, arguments: &Value) -> Option<RigProtocolResult> {
        match tool_name {
            "submit_graph" => {
                let already_submitted = *self.submitted.lock();
                let outcome = super::project_submit::intercept_submit_graph(
                    &self.db,
                    &self.workspace_id,
                    self.app_handle.as_ref(),
                    arguments,
                    already_submitted,
                )
                .await;
                Some(match outcome {
                    Ok(super::project_submit::SubmitGraphInterception::Submitted {
                        display_text,
                        action,
                    }) => {
                        *self.submitted.lock() = true;
                        RigProtocolResult {
                            text: display_text,
                            retryable_error: false,
                            actions: vec![action],
                            final_message: None,
                        }
                    }
                    Ok(super::project_submit::SubmitGraphInterception::Rejected { error }) => {
                        RigProtocolResult::retryable_error(error)
                    }
                    Err(error) => RigProtocolResult::retryable_error(format!(
                        "错误：执行图提交处理失败：{error:#}"
                    )),
                })
            }
            "graph_plan_report" => {
                let report = super::project_report::build_plan_report(
                    &self.db,
                    &self.workspace_id,
                    arguments,
                )
                .await;
                Some(match report {
                    Ok(text) => RigProtocolResult::text_feedback(text),
                    Err(error) => {
                        RigProtocolResult::retryable_error(format!("错误：读取执行图报告失败：{error:#}"))
                    }
                })
            }
            "message" => {
                let content = arguments
                    .get("content")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .unwrap_or_default();
                if content.is_empty() {
                    return Some(RigProtocolResult::retryable_error(
                        "错误：message 缺少内容（content 不能为空）。",
                    ));
                }
                Some(RigProtocolResult {
                    text: format!("已向用户发送最终答复（{} 字符）。", content.chars().count()),
                    retryable_error: false,
                    actions: Vec::new(),
                    final_message: Some(content.to_string()),
                })
            }
            _ => None,
        }
    }

    async fn render_outcome(
        &self,
        actions: &[RigProtocolAction],
        final_message: Option<&str>,
    ) -> Option<String> {
        let mut sections = Vec::new();
        for action in actions {
            match action {
                RigProtocolAction::GraphSubmitted { title, node_count } => sections.push(format!(
                    "🗺️ 执行图《{title}》已生成并通过校验（{node_count} 个节点）。\n\n请在图面板中检查节点设计与任务指令，确认后开始执行。"
                )),
            }
        }
        if let Some(message) = final_message.map(str::trim).filter(|msg| !msg.is_empty()) {
            sections.push(if sections.is_empty() {
                message.to_string()
            } else {
                format!("补充说明：\n{message}")
            });
        }
        (!sections.is_empty()).then(|| sections.join("\n\n"))
    }
}

/// 项目工作区边界校验（审查项 G8-12）：绝对路径、无 `..`、解析符号链接后
/// 必须命中受管项目列表（`projects` 表）。
async fn validate_project_workspace(
    db: &DispatcherDb,
    workspace_path: &str,
) -> anyhow::Result<PathBuf> {
    let db = db.clone();
    let workspace_path = workspace_path.to_string();
    tokio::task::spawn_blocking(move || validate_project_workspace_sync(&db, &workspace_path))
        .await
        .map_err(|error| anyhow::anyhow!("错误：工作区校验任务失败：{error}"))?
}

fn validate_project_workspace_sync(
    db: &DispatcherDb,
    workspace_path: &str,
) -> anyhow::Result<PathBuf> {
    let raw = PathBuf::from(workspace_path);
    anyhow::ensure!(
        raw.is_absolute(),
        "错误：项目路径必须是绝对路径：{workspace_path}"
    );
    anyhow::ensure!(
        !raw.components()
            .any(|component| matches!(component, std::path::Component::ParentDir)),
        "错误：项目路径不允许包含 ..：{workspace_path}"
    );

    let projects = db
        .list_projects()
        .map_err(|error| anyhow::anyhow!("错误：读取受管项目列表失败：{error}"))?;
    anyhow::ensure!(
        !projects.is_empty(),
        "错误：受管项目列表为空，无法校验项目路径：{workspace_path}"
    );

    let candidate = resolve_with_existing_prefix(&raw)
        .ok_or_else(|| anyhow::anyhow!("错误：解析项目路径失败：{workspace_path}"))?;
    let managed = projects.iter().any(|project| {
        resolve_with_existing_prefix(std::path::Path::new(&project.path))
            .is_some_and(|path| path == candidate)
    });
    anyhow::ensure!(
        managed,
        "错误：项目路径不在受管项目列表中：{workspace_path}"
    );
    Ok(candidate)
}

/// 对可能尚不存在的路径做尽力解析：canonicalize 最深的已存在祖先目录，
/// 再拼回缺失的尾部组件；对已存在路径等价于 canonicalize。
fn resolve_with_existing_prefix(path: &std::path::Path) -> Option<PathBuf> {
    let mut missing: Vec<std::ffi::OsString> = Vec::new();
    let mut cursor = path;
    loop {
        match std::fs::symlink_metadata(cursor) {
            Ok(_) => {
                let mut resolved = cursor.canonicalize().ok()?;
                for name in missing.iter().rev() {
                    resolved.push(name);
                }
                return Some(resolved);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let name = cursor.file_name()?;
                missing.push(name.to_os_string());
                cursor = cursor.parent()?;
            }
            Err(_) => return None,
        }
    }
}

/// 会话图片目录（构造失败按空白名单收紧，fail-closed）。
fn chat_image_dir(workspace_id: &str) -> Vec<PathBuf> {
    match crate::chat_images::workspace_image_dir(workspace_id) {
        Ok(dir) => vec![dir],
        Err(error) => {
            log_warning(&format!(
                "[orchestrator] 构造会话图片目录失败，按空白名单收紧（workspace_id={workspace_id}）：{error}"
            ));
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{resolve_with_existing_prefix, validate_project_workspace_sync};
    use std::path::{Component, Path, PathBuf};

    #[test]
    fn resolve_with_existing_prefix_handles_missing_tail() {
        let base = std::env::temp_dir();
        let canonical_base = base.canonicalize().expect("canonicalize temp dir");
        let target = canonical_base
            .join("jk-no-such-dir-a")
            .join("jk-no-such-dir-b");
        let resolved = resolve_with_existing_prefix(&target).expect("resolve");
        assert_eq!(
            resolved,
            canonical_base
                .join("jk-no-such-dir-a")
                .join("jk-no-such-dir-b")
        );
    }

    #[test]
    fn parent_dir_component_detection() {
        let hostile = PathBuf::from("/tmp/foo/../bar");
        assert!(hostile
            .components()
            .any(|component| matches!(component, Component::ParentDir)));
        let clean = Path::new("/tmp/foo/bar");
        assert!(!clean
            .components()
            .any(|component| matches!(component, Component::ParentDir)));
    }

    #[test]
    fn unregistered_or_relative_project_paths_are_rejected() {
        let dir = std::env::temp_dir().join(format!("rig-orch-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let db = crate::agent::db::DispatcherDb::new(dir.join("jkbot.sqlite3")).expect("temp db");

        // 相对路径：直接拒绝（不查库）。
        let relative = validate_project_workspace_sync(&db, "relative/project");
        assert!(relative.is_err());
        // 含 .. 的绝对路径：拒绝。
        let parent = validate_project_workspace_sync(&db, "/tmp/a/../b");
        assert!(parent.is_err());
        // 受管项目列表为空：任何绝对路径都无法通过。
        let unmanaged = validate_project_workspace_sync(&db, "/tmp");
        assert!(unmanaged
            .expect_err("空项目列表必须拒绝")
            .to_string()
            .contains("受管项目列表为空"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
