mod browser;
mod common;
mod filesystem;
mod graph_plan_report;
mod image_edit;
mod image_generation;
mod local_zsh;
mod run_tool_program;
mod search;
mod shell;
mod ssh;
mod submit_graph;

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

/// 编排器（项目 Agent）固定工具集：只读探索 + message 答复 + submit_graph 收口
/// + graph_plan_report 运行报告（反思闭环）。
/// 天然无写文件/执行命令能力；协议壳工具不进通用目录。
pub(super) fn orchestrator_tools() -> Vec<Box<dyn AgentTool>> {
    vec![
        filesystem::read_file_tool(),
        filesystem::list_dir_tool(),
        search::glob_tool(),
        search::grep_tool(),
        run_tool_program::run_tool_program_tool(),
        shell::message_tool(),
        submit_graph::submit_graph_tool(),
        graph_plan_report::graph_plan_report_tool(),
    ]
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
