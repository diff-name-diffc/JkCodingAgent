//! 命令执行工具组（T2.2）：local_zsh / ssh_* / ssh_memo_* / sync_directory。
//!
//! 移植自旧 `agent/tools/builtin/{local_zsh,working_directory,ssh,ssh_memo,sync_directory}`。
//! 命令安全审查门禁（`ssh_review` / `review_context`）不在工具层——由 runtime
//! `ToolExecutionPolicy`（Phase 3）在调用前拦截，见各工具内的 TODO(T3) 标注。

mod local_zsh;
mod ssh;
mod ssh_memo;
mod sync_directory;

use rig::tool::PortableDynamicTool;

use super::deps::RigToolDeps;

pub(crate) fn exec_tools(deps: &RigToolDeps) -> Vec<PortableDynamicTool> {
    let mut tools = vec![local_zsh::local_zsh_tool(
        deps.workspace.clone(),
        deps.workspace_id.clone(),
        deps.exec_timeout_secs,
        deps.cancel_rx.clone(),
    )];
    tools.extend(ssh::ssh_tools(
        deps.ssh_manager.clone(),
        deps.workspace.clone(),
        deps.workspace_id.clone(),
        deps.db.clone(),
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
    ));
    tools
}
