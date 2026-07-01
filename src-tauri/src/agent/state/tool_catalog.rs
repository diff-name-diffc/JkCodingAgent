use parking_lot::Mutex;
use std::path::Path;

use crate::agent::sub_agent::db::ToolInfo;
use crate::agent::tools::ToolRegistry;

/// 已注册工具名/描述的缓存（工具目录）。
/// 避免 UI 每次查询工具列表时都要重新构建完整 ToolRegistry。
pub(super) struct ToolCatalog {
    registered: Mutex<Option<Vec<(String, String)>>>,
}

impl ToolCatalog {
    pub(super) fn new(initial_tools: Vec<(String, String)>) -> Self {
        Self {
            registered: Mutex::new(Some(initial_tools)),
        }
    }

    pub(super) fn registered_tool_names(&self) -> Option<Vec<(String, String)>> {
        self.registered.lock().clone()
    }

    pub(super) fn set_registered_tools(&self, tools: Vec<(String, String)>) {
        *self.registered.lock() = Some(tools);
    }
}

pub(super) fn tool_infos_from_registry(
    registry: &ToolRegistry,
    workspace: Option<&Path>,
    include_dynamic: bool,
) -> Vec<ToolInfo> {
    let definitions = match workspace {
        Some(workspace) => registry.definitions_for_workspace(
            workspace,
            Option::<std::iter::Empty<&str>>::None,
            include_dynamic,
        ),
        None => registry.definitions_for_workspace(
            Path::new("."),
            Option::<std::iter::Empty<&str>>::None,
            false,
        ),
    };

    let mut tools = definitions
        .into_iter()
        .map(|definition| ToolInfo {
            name: definition.function.name,
            description: definition.function.description,
        })
        .collect::<Vec<_>>();
    tools.sort_by(|left, right| left.name.cmp(&right.name));
    tools.dedup_by(|left, right| left.name == right.name);
    tools
}
