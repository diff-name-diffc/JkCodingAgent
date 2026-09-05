use std::fs;
use std::path::{Path, PathBuf};

use super::policy::{
    allowed_mcp_tools_by_config, effective_allowed_tools_for_chat_category,
    is_tool_allowed_by_config, retain_allowed_definitions, session_workspace_dir_name,
};
use super::*;

impl PlainChatAgent {
    /// 每个会话独立的文件沙箱（G9-04）：`root_dir/plain-chat-browser/<会话子目录>`。
    ///
    /// 旧实现让所有聊天会话共享固定目录，`restrict_to_workspace: true` 反而把
    /// 并发会话的文件类工具都限定在同一目录内互相覆盖；按会话（workspace_id）
    /// 建子目录后各会话的文件结果互不干扰。该目录只是文件沙箱——聊天的
    /// MCP 配置一律来自全局注册表（`McpScope::Global`），不在这里落任何配置。
    pub(super) async fn session_workspace(&self, workspace_id: &str) -> Result<PathBuf> {
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

    pub(super) async fn build_tool_context(
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
        // 安全审查的「对话上下文」：最近若干轮用户/助手对话（截断渲染）。
        // 读取失败降级为 None（审查仍可依据任务/意图/命令本身判定）；
        // 与 project agent 同口径记日志，避免 DB 故障静默不可诊断。
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
        // 只放行当前会话的图片目录（chat-images/{workspace_id}）：其他会话
        // 图片和全局设置目录均不可见。目录构造失败时按空白名单收紧，
        // 路径规范化失败同样 fail-closed 剔除。
        let chat_image_dir = match crate::chat_images::workspace_image_dir(workspace_id) {
            Ok(dir) => vec![dir],
            Err(error) => {
                eprintln!(
                    "构造会话图片目录失败（workspace_id={workspace_id}），按空白名单收紧：{error}"
                );
                Vec::new()
            }
        };
        // get_settings_v2 是同步 SQLite 读取，async 路径必须经 spawn_blocking，
        // 避免阻塞 Tokio 运行时线程（G9-02）。一次锁读派生 ssh_review 与
        // 图片模型凭据（generate_image / edit_image 工具共用）。
        let settings_db = db.clone();
        let (ssh_review, image_credentials) = tokio::task::spawn_blocking(move || {
            settings_db
                .get_settings_v2()
                .ok()
                .map(|settings| {
                    (
                        settings.review.is_configured().then_some(settings.review),
                        settings.shared.image_model_credentials(),
                    )
                })
                .unwrap_or_default()
        })
        .await
        .unwrap_or_default();
        // 一次锁读派生两个字段，保证 vision_model 与 vision_provider 同源。
        let vision_provider = self.vision_provider.lock().clone();
        let vision_model = vision_provider
            .as_ref()
            .map(|p| p.model().to_string())
            .unwrap_or_default();
        ToolContext {
            workspace_id: workspace_id.to_string(),
            workspace: workspace.to_path_buf(),
            // 普通聊天没有项目语境：MCP 一律走全局注册表，所有会话共享。
            mcp_scope: McpScope::Global,
            session_title,
            user_task,
            executor_task: None,
            review_conversation,
            ssh_review,
            exec_timeout_secs: self.config.exec_timeout_secs,
            restrict_to_workspace: true,
            // 只放行当前会话的图片目录（chat-images/{workspace_id}）；其他
            // 会话图片与全局设置目录不可见，路径规范化失败 fail-closed 剔除。
            extra_allowed_dirs: chat_image_dir,
            app_handle: self.app_handle.clone(),
            llm_provider: Some(provider.clone()),
            vision_model,
            vision_provider,
            image_model_url: image_credentials.url,
            image_model_api_key: image_credentials.api_key,
            image_model: image_credentials.model,
            image_edit_model: image_credentials.edit_model,
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

    pub(super) fn build_effective_system_prompt(&self, workspace_id: &str) -> String {
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
        // MCP 清单与工具定义层共享同一允许列表过滤
        // （`allowed_mcp_tools_for_current_config`）：模型被告知可调用的
        // MCP 工具恒等于实际授权集，未配置的分类不会收到本段；快照缺失
        // （run 入口 prepare_run_workspace 负责预热）时同样按空列表降级。
        let mcp_tools = self.allowed_mcp_tools_for_current_config();
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
    pub(super) async fn warm_sub_agent_exposure(&self, workspace_id: &str) {
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
        if self.tool_allowed_by_config("call_sub_agent")
            || self.tool_allowed_by_config("list_sub_agents")
        {
            return true;
        }

        self.session_has_enabled_sub_agents(workspace_id)
    }

    fn tool_allowed_by_config(&self, tool_name: &str) -> bool {
        let configured = self.allowed_tools.lock();
        is_tool_allowed_by_config(&configured, tool_name)
    }

    fn session_has_enabled_sub_agents(&self, workspace_id: &str) -> bool {
        !self.cached_enabled_sub_agents(workspace_id).is_empty()
    }

    /// 当前允许列表下可用的 MCP 工具（全局作用域快照 ∩ 显式名单）。
    /// 与 `build_tool_definitions` 共享 `allowed_mcp_tools_by_config` 的
    /// 过滤契约：允许列表为空 = 无任何 MCP 工具。
    fn allowed_mcp_tools_for_current_config(&self) -> Vec<ResolvedMcpTool> {
        let configured = self.allowed_tools.lock().clone();
        let snapshot_tools = tool_definitions_from_snapshot(
            self.mcp_registry
                .cached_for_scope(&McpScope::Global)
                .as_ref(),
        );
        allowed_mcp_tools_by_config(snapshot_tools, &configured)
    }

    pub(super) fn build_tool_definitions(
        &self,
        workspace_id: &str,
        scope: &McpScope,
    ) -> Vec<ToolDefinition> {
        let configured = self.allowed_tools.lock().clone();
        let defs =
            self.tools
                .definitions_for_scope(scope, Option::<std::iter::Empty<&str>>::None, true);

        // 混合契约（见 `retain_allowed_definitions`）：内置工具维持
        // 「空列表 = 全部放行」，非空时按允许列表（含启用子智能体时的
        // 子智能体工具豁免）收敛；MCP 工具一律显式名单制，只有被明确
        // 配置的分类/设置才能看到它们。
        let builtin_allowed = if configured.is_empty() {
            None
        } else {
            Some(effective_allowed_tools_for_chat_category(
                configured.clone(),
                self.session_has_enabled_sub_agents(workspace_id),
            ))
        };
        retain_allowed_definitions(defs, builtin_allowed.as_ref(), &configured)
    }
}
