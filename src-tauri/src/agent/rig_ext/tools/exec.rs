//! 命令执行工具组（T2.2）：local_zsh / ssh_* / ssh_memo_* / sync_directory。

use rig::tool::PortableDynamicTool;

use super::deps::RigToolDeps;

pub(crate) fn exec_tools(_deps: &RigToolDeps) -> Vec<PortableDynamicTool> {
    Vec::new()
}
