//! 文件系统与搜索工具组（T2.1）：read_file / list_dir / glob / grep。
//! 只读数据面，编排器与 plain chat 共用。
//! 迁移自旧自实现工具层（已随迁移删除）的 filesystem/search 工具；
//! 旧运行时工具上下文的路径沙箱字段收敛为构造期 `FsSandbox`，
//! 结构化 `data` 载荷不再随结果返回（runtime 只持久化原始文本产物）。

use std::path::PathBuf;

use rig::tool::PortableDynamicTool;

use super::common::resolve_path;
use super::deps::RigToolDeps;

mod list_dir;
mod read_file;
mod search;

/// 只读文件工具的路径沙箱（构造期从 `RigToolDeps` 提取，clone 进各工具闭包）。
#[derive(Clone)]
pub(super) struct FsSandbox {
    pub workspace: PathBuf,
    pub restrict_to_workspace: bool,
    pub extra_allowed_dirs: Vec<PathBuf>,
}

impl FsSandbox {
    fn from_deps(deps: &RigToolDeps) -> Self {
        Self {
            workspace: deps.workspace.clone(),
            restrict_to_workspace: deps.restrict_to_workspace,
            extra_allowed_dirs: deps.extra_allowed_dirs.clone(),
        }
    }

    pub fn resolve(&self, raw_path: &str) -> Result<PathBuf, String> {
        resolve_path(
            &self.workspace,
            self.restrict_to_workspace,
            &self.extra_allowed_dirs,
            raw_path,
        )
    }
}

pub(crate) fn fs_tools(deps: &RigToolDeps) -> Vec<PortableDynamicTool> {
    let sandbox = FsSandbox::from_deps(deps);
    vec![
        read_file::read_file_tool(sandbox.clone()),
        list_dir::list_dir_tool(sandbox.clone()),
        search::glob_tool(sandbox.clone()),
        search::grep_tool(sandbox),
    ]
}
