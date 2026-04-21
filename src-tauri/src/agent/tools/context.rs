use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct ToolContext {
    pub workspace: PathBuf,
    pub workspace_id: String,
    pub dispatcher_db_path: PathBuf,
    pub exec_timeout_secs: u64,
    pub restrict_to_workspace: bool,
}
