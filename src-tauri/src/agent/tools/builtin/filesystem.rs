use crate::agent::tools::registry::AgentTool;

mod list_dir;
mod read_file;

pub(super) fn read_file_tool() -> Box<dyn AgentTool> {
    read_file::read_file_tool()
}

pub(super) fn list_dir_tool() -> Box<dyn AgentTool> {
    list_dir::list_dir_tool()
}
