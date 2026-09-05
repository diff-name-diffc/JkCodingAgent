use std::fs;
use std::path::{Path, PathBuf};

use super::*;

impl ArchitectureAgent {
    /// 每个会话独立的文件沙箱：`root_dir/architecture/<会话子目录>`。
    /// 目录命名复用 plain_chat 的净化函数（防路径遍历、确定性、不折叠）。
    /// 当前唯一工具不落盘文件，沙箱为后续扩展保留，且是
    /// `restrict_to_workspace` 语义的锚点。
    pub(super) async fn session_workspace(&self, workspace_id: &str) -> Result<PathBuf> {
        let workspace = self.config.root_dir.join("architecture").join(
            crate::agent::agents::plain_chat::policy::session_workspace_dir_name(workspace_id),
        );
        tokio::task::spawn_blocking({
            let workspace = workspace.clone();
            move || {
                fs::create_dir_all(&workspace)
                    .with_context(|| format!("create {}", workspace.display()))
            }
        })
        .await
        .map_err(|error| {
            anyhow::anyhow!("create architecture session workspace panicked: {error}")
        })??;
        Ok(workspace)
    }

    /// 架构 Agent 的极简工具上下文：单工具不触碰文件/命令/网络，凭据类字段
    /// 一律置空；保留 app_handle（工具经它取状态注册表并 emit 画布事件）、
    /// cancel_rx 注入与会话图片目录放行（截图经 chat_images 落盘）。
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
        // 只放行当前会话的图片目录（执行截图经 save_chat_image 落盘于此）。
        // 目录在首张截图落盘前可能尚不存在，而 `normalize_paths` 会剔除
        // 无法 canonicalize 的路径并告警——故先创建再放行。
        let chat_image_dir = match crate::chat_images::workspace_image_dir(workspace_id) {
            Ok(dir) => match tokio::fs::create_dir_all(&dir).await {
                Ok(()) => vec![dir],
                Err(error) => {
                    eprintln!(
                        "创建会话图片目录失败（{}），按空白名单收紧：{error}",
                        dir.display()
                    );
                    Vec::new()
                }
            },
            Err(error) => {
                eprintln!(
                    "构造会话图片目录失败（workspace_id={workspace_id}），按空白名单收紧：{error}"
                );
                Vec::new()
            }
        };
        let vision_model = provider.model().to_string();
        ToolContext {
            workspace_id: workspace_id.to_string(),
            workspace: workspace.to_path_buf(),
            mcp_scope: McpScope::Global,
            session_title,
            user_task,
            // 架构 Agent 当前无命令执行类工具，不加载审查对话上下文。
            executor_task: None,
            review_conversation: None,
            ssh_review: None,
            exec_timeout_secs: self.config.exec_timeout_secs,
            restrict_to_workspace: true,
            extra_allowed_dirs: chat_image_dir,
            app_handle: self.app_handle.clone(),
            llm_provider: Some(provider.clone()),
            vision_model,
            vision_provider: Some(provider.clone()),
            image_model_url: String::new(),
            image_model_api_key: String::new(),
            image_model: String::new(),
            image_edit_model: String::new(),
            sub_agent_tool_registry: None,
            current_sub_agent_id: None,
            current_sub_agent_name: None,
            current_tool_call_id: None,
            current_tool_spec_hash: None,
            cancel_rx: None,
            sub_agent_parent_tool_call_id: None,
            sub_agent_trace_events: None,
        }
    }
}
