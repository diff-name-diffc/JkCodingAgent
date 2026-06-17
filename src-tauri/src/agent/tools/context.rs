use std::path::PathBuf;
use std::sync::Arc;

use tauri::AppHandle;

use crate::agent::db::settings::SshReviewConfig;
use crate::agent::llm::OpenAiCompatProvider;
use crate::agent::tools::registry::ToolRegistry;

#[derive(Clone)]
pub struct ToolContext {
    pub workspace_id: String,
    pub workspace: PathBuf,
    pub session_title: String,
    /// 当前用户任务（最新一条用户消息文本），供 SSH 安全审查等场景使用。
    pub user_task: Option<String>,
    /// SSH 命令安全审查 AI 配置；None 表示未配置，跳过审查。
    pub ssh_review: Option<SshReviewConfig>,
    pub exec_timeout_secs: u64,
    pub restrict_to_workspace: bool,
    /// 额外允许访问的目录白名单（不受 restrict_to_workspace 限制）
    pub extra_allowed_dirs: Vec<PathBuf>,
    pub app_handle: Option<AppHandle>,
    pub llm_provider: Option<OpenAiCompatProvider>,
    pub vision_model: String,
    pub image_model_url: String,
    pub image_model_api_key: String,
    pub image_model: String,
    pub image_edit_model: String,
    pub sub_agent_tool_registry: Option<Arc<ToolRegistry>>,
    pub current_sub_agent_id: Option<String>,
    pub current_sub_agent_name: Option<String>,
}
