mod browser;
mod common;
mod filesystem;
mod image_edit;
mod image_generation;
mod local_zsh;
mod search;
mod shell;
mod ssh;

use super::registry::AgentTool;
use crate::ssh_tool::SshSessionManager;

pub(super) fn builtin_tools(ssh_manager: SshSessionManager) -> Vec<Box<dyn AgentTool>> {
    let mut tools = vec![
        filesystem::read_file_tool(),
        filesystem::write_file_tool(),
        filesystem::edit_file_tool(),
        filesystem::list_dir_tool(),
        search::glob_tool(),
        search::grep_tool(),
        shell::exec_tool(),
        shell::message_tool(),
        image_generation::generate_image_tool(),
        image_edit::edit_image_tool(),
    ];
    tools.extend(browser::browser_tools());
    tools.extend(ssh::ssh_tools(ssh_manager));
    tools
}

pub(super) fn plain_chat_tools(ssh_manager: SshSessionManager) -> Vec<Box<dyn AgentTool>> {
    let mut tools = vec![
        local_zsh::local_zsh_tool(),
        image_generation::generate_image_tool(),
        image_edit::edit_image_tool(),
    ];
    tools.extend(browser::browser_tools());
    tools.extend(ssh::ssh_tools(ssh_manager));
    tools
}
