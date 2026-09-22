//! 文件系统与搜索工具组（T2.1）：read_file / list_dir / glob / grep。
//! 只读数据面，编排器与 plain chat 共用。

use rig::tool::PortableDynamicTool;

use super::deps::RigToolDeps;

pub(crate) fn fs_tools(_deps: &RigToolDeps) -> Vec<PortableDynamicTool> {
    Vec::new()
}
