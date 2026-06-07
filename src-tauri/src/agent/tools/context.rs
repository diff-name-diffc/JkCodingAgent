use std::path::PathBuf;
use std::sync::Arc;

use tauri::AppHandle;

use crate::agent::llm::OpenAiCompatProvider;
use crate::agent::tools::registry::ToolRegistry;

#[derive(Clone)]
pub struct ToolContext {
    pub workspace_id: String,
    pub workspace: PathBuf,
    pub session_title: String,
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
}
