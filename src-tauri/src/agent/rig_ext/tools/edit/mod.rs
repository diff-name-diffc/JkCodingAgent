//! 文件编辑工具组：apply_patch（核心文件编辑能力，移植自 openai/codex 的
//! apply-patch 格式与匹配算法，见 `engine.rs` 模块文档）。
//!
//! 与 `fs` 组（只读数据面）相对，本组是**写侧**工具：路径经 `FsSandbox`
//! 沙箱化（工作区 + 额外授权目录），效果域是工作区内文件变更。

mod apply_patch;
mod engine;

use rig::tool::PortableDynamicTool;

use super::deps::RigToolDeps;
use super::fs::FsSandbox;

pub(crate) fn edit_tools(deps: &RigToolDeps) -> Vec<PortableDynamicTool> {
    vec![apply_patch::apply_patch_tool(FsSandbox::from_deps(deps))]
}
