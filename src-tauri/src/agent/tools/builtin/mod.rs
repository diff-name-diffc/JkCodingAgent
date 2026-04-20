mod common;
mod filesystem;
mod search;
mod shell;

use super::registry::AgentTool;

pub(super) fn builtin_tools() -> Vec<Box<dyn AgentTool>> {
    vec![
        filesystem::read_file_tool(),
        filesystem::write_file_tool(),
        filesystem::edit_file_tool(),
        filesystem::list_dir_tool(),
        search::glob_tool(),
        search::grep_tool(),
        shell::exec_tool(),
        shell::message_tool(),
    ]
}
