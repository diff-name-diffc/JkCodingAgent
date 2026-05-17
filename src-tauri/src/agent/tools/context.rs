use std::path::PathBuf;

use tauri::AppHandle;

use crate::agent::llm::OpenAiCompatProvider;

#[derive(Clone)]
pub struct ToolContext {
    pub workspace_id: String,
    pub workspace: PathBuf,
    pub session_title: String,
    pub exec_timeout_secs: u64,
    pub restrict_to_workspace: bool,
    pub app_handle: Option<AppHandle>,
    pub llm_provider: Option<OpenAiCompatProvider>,
    pub vision_model: String,
    pub image_model_url: String,
    pub image_model_api_key: String,
    pub image_model: String,
    pub image_edit_model: String,
}
