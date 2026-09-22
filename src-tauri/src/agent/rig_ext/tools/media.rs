//! 多媒体工具组（T2.3a）：generate_image / edit_image / analyze_image /
//! fetch_image / browser_*。

use rig::tool::PortableDynamicTool;

use super::deps::RigToolDeps;

pub(crate) fn media_tools(_deps: &RigToolDeps) -> Vec<PortableDynamicTool> {
    Vec::new()
}
