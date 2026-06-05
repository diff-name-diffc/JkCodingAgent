mod browser;
mod common;
mod filesystem;
mod image_edit;
mod image_generation;
mod knowledge;
mod search;
mod shell;

use super::registry::AgentTool;

pub(super) fn builtin_tools() -> Vec<Box<dyn AgentTool>> {
    let mut tools = vec![
        filesystem::read_file_tool(),
        filesystem::write_file_tool(),
        filesystem::edit_file_tool(),
        filesystem::list_dir_tool(),
        search::glob_tool(),
        search::grep_tool(),
        knowledge::search_knowledge_base_tool(),
        knowledge::read_knowledge_page_tool(),
        shell::exec_tool(),
        shell::message_tool(),
        image_generation::generate_image_tool(),
        image_edit::edit_image_tool(),
    ];
    tools.extend(browser::browser_tools());
    tools
}

pub(super) fn plain_chat_tools() -> Vec<Box<dyn AgentTool>> {
    let mut tools = vec![
        image_generation::generate_image_tool(),
        image_edit::edit_image_tool(),
    ];
    tools.extend(browser::browser_tools());
    tools
}
