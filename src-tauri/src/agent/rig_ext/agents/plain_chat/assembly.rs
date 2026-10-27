//! 工具依赖、审查上下文与工具清单装配。
use super::*;

impl RigPlainChatAgent {
    /// 构造工具依赖快照。
    pub(super) async fn build_deps(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        workspace: &ChatWorkspace,
        cancel_rx: &watch::Receiver<bool>,
    ) -> RigToolDeps {
        // 先取出锁内值再准备异步审查快照，await 不能跨越 MutexGuard。
        let tool_timeouts = self.tool_timeouts.lock().clone();
        let review = self
            .review_context(db, workspace_id, None)
            .await
            .with_chat_workspace(workspace);
        let mut extra_allowed_dirs = workspace.project_roots.clone();
        extra_allowed_dirs.extend(chat_image_dir(workspace_id));
        RigToolDeps {
            workspace_id: workspace_id.to_string(),
            workspace: workspace.session_root.clone(),
            chat_workspace: Some(workspace.clone()),
            // 普通聊天没有项目语境：MCP 一律走全局注册表。
            mcp_scope: McpScope::Global,
            exec_timeout_secs: self.config.exec_timeout_secs,
            restrict_to_workspace: true,
            extra_allowed_dirs,
            app_handle: self.app_handle.clone(),
            db: db.clone(),
            ssh_manager: self.ssh_manager.clone(),
            log_snapshots: Some(self.log_snapshots.clone()),
            term_registry: self.term_registry.clone(),
            mcp_registry: self.mcp_registry.clone(),
            sub_agent_manager: self.sub_agent_manager.clone(),
            cancel_rx: Some(cancel_rx.clone()),
            vision_spec: self.specs.vision.clone(),
            image: self.image_tool_config(),
            tool_timeouts,
            review,
        }
    }

    pub(super) fn image_tool_config(&self) -> ImageToolConfig {
        let credentials = self.image_credentials.lock().clone();
        ImageToolConfig {
            url: credentials.url,
            api_key: credentials.api_key,
            model: credentials.model,
            edit_model: credentials.edit_model,
        }
    }

    /// 审查上下文：会话标题 + 用户任务 + 最近对话（读取失败降级为 None）。
    pub(super) async fn review_context(
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

    /// 子智能体工具清单（子智能体保存校验与选择列表用）：与
    /// `RigSubAgentRuntime` 实际继承的 execution profile 同源——exec + edit +
    /// media 加子智能体专用 `notify_user_progress`；**不含**嵌套子智能体工具
    /// （call_sub_agent / list_sub_agents 不得递归派生）。
    pub fn sub_agent_tool_catalog(&self) -> Vec<(String, String)> {
        let deps = self.catalog_deps();
        let mut tools = exec_tools(&deps);
        tools.extend(edit_tools(&deps));
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
            chat_workspace: Some(ChatWorkspace {
                session_root: self.config.root_dir.join("plain-chat-browser/tool-catalog"),
                project_roots: Vec::new(),
            }),
            mcp_scope: McpScope::Global,
            exec_timeout_secs: self.config.exec_timeout_secs,
            restrict_to_workspace: true,
            extra_allowed_dirs: Vec::new(),
            app_handle: None,
            db: crate::agent::db::DispatcherDb::new(self.config.db_path.clone())
                .expect("打开工具清单用数据库句柄"),
            ssh_manager: self.ssh_manager.clone(),
            log_snapshots: Some(self.log_snapshots.clone()),
            term_registry: self.term_registry.clone(),
            mcp_registry: self.mcp_registry.clone(),
            sub_agent_manager: self.sub_agent_manager.clone(),
            cancel_rx: None,
            vision_spec: self.specs.vision.clone(),
            image: self.image_tool_config(),
            // 清单枚举只取名称/描述不执行，超时默认不影响结果，但保持与
            // 真实装配同源，避免清单面与执行面出现两套配置口径。
            tool_timeouts: self.tool_timeouts.lock().clone(),
            review: RigReviewContext::unconfigured(),
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
