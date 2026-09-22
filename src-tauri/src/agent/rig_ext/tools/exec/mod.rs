//! 命令执行工具组（T2.2）：local_zsh / ssh_* / ssh_memo_* / sync_directory。
//!
//! 迁移自旧自实现工具层（已随迁移删除）的 local_zsh / working_directory /
//! ssh / ssh_memo / sync_directory。
//! 命令安全审查门禁随工具就地恢复（旧实现即自管审查：命令类工具携带完整
//! 目标环境上下文做 fail-closed 判定），审查输入经 `RigToolDeps.review` 注入。

mod local_zsh;
mod ssh;
mod ssh_memo;
mod sync_directory;

use rig::tool::PortableDynamicTool;

use super::deps::RigToolDeps;

pub(crate) use local_zsh::local_zsh_dir;

pub(crate) fn exec_tools(deps: &RigToolDeps) -> Vec<PortableDynamicTool> {
    let mut tools = vec![local_zsh::local_zsh_tool(
        deps.workspace.clone(),
        deps.workspace_id.clone(),
        deps.exec_timeout_secs,
        deps.cancel_rx.clone(),
        deps.review.clone(),
    )];
    tools.extend(ssh::ssh_tools(
        deps.ssh_manager.clone(),
        deps.workspace.clone(),
        deps.workspace_id.clone(),
        deps.db.clone(),
        deps.review.clone(),
    ));
    tools.extend(ssh_memo::ssh_memo_tools(deps.ssh_manager.clone()));
    tools.push(sync_directory::sync_directory_tool(
        deps.ssh_manager.clone(),
        deps.workspace.clone(),
        deps.workspace_id.clone(),
        deps.restrict_to_workspace,
        deps.extra_allowed_dirs.clone(),
        deps.app_handle.clone(),
        deps.db.clone(),
        deps.cancel_rx.clone(),
        deps.review.clone(),
        deps.tool_call_id.clone(),
    ));
    tools
}
