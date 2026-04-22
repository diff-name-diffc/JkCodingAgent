use std::path::PathBuf;
use std::sync::Arc;

use tauri::AppHandle;

use crate::agent::dwg::viewer_bridge::DwgViewerBridgeState;

#[derive(Debug, Clone)]
pub struct ToolContext {
    pub workspace: PathBuf,
    pub workspace_id: String,
    pub dispatcher_db_path: PathBuf,
    pub exec_timeout_secs: u64,
    pub restrict_to_workspace: bool,
    pub app_handle: Option<AppHandle>,
    pub dwg_viewer_bridge: Option<Arc<DwgViewerBridgeState>>,
}
