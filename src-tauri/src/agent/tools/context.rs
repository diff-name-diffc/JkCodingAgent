use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct ToolContext {
    pub workspace: PathBuf,
    pub max_result_chars: usize,
    pub exec_timeout_secs: u64,
    pub restrict_to_workspace: bool,
}
