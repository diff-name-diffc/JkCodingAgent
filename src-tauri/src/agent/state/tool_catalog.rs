use parking_lot::Mutex;
use std::path::Path;

use crate::agent::sub_agent::db::ToolInfo;
use crate::agent::tools::ToolRegistry;

/// 已注册工具名/描述的缓存（工具目录）。
/// 避免 UI 每次查询工具列表时都要重新构建完整 ToolRegistry。
///
/// G11-07：缓存只是「最近一次已知状态」的快照——读取端
/// （`DispatcherState::registered_tool_names`）每次读取都会重建并调用
/// `refresh` 原子替换，子智能体配置变化不会受缓存陈旧影响。
pub(super) struct ToolCatalog {
    registered: Mutex<Option<Vec<(String, String)>>>,
}

impl ToolCatalog {
    pub(super) fn new(initial_tools: Vec<(String, String)>) -> Self {
        Self {
            registered: Mutex::new(Some(initial_tools)),
        }
    }

    /// 重建缓存：原子替换全部内容（G11-07 的刷新/失效入口）。
    pub(super) fn refresh(&self, tools: Vec<(String, String)>) {
        *self.registered.lock() = Some(tools);
    }

    pub(super) fn registered_tool_names(&self) -> Option<Vec<(String, String)>> {
        self.registered.lock().clone()
    }
}

/// 枚举对 UI/LLM 可见的工具列表。
///
/// 注意（G11-06）：本函数为**同步**实现——枚举全部工具 spec（含 MCP 缓存
/// 快照查找、子智能体工具、schema 构建）并排序/去重。当前底层全部是内存
/// 操作，但 async 上下文的调用方必须用 `tokio::task::spawn_blocking` 包裹，
/// 防止未来任何底层实现引入文件/网络/DB 访问时卡死异步执行器。
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
        None => {
            // G11-05：「无工作区」的显式语义——动态（MCP）工具依赖工作区级
            // 缓存快照才能枚举，没有工作区时只返回静态工具。这是契约而非
            // 静默降级：调用方在无工作区时请求动态工具会收到一次性警告。
            if include_dynamic {
                eprintln!(
                    "tool_infos_from_registry: 无工作区上下文，动态（MCP）工具无法枚举，仅返回静态工具"
                );
            }
            registry.definitions_for_workspace(
                Path::new("."),
                Option::<std::iter::Empty<&str>>::None,
                false,
            )
        }
    };

    let mut tools = definitions
        .into_iter()
        .map(|definition| ToolInfo {
            name: definition.function.name,
            description: definition.function.description,
        })
        .collect::<Vec<_>>();
    tools.sort_by(|left, right| left.name.cmp(&right.name));

    // G11-08：同名去重前显式告警，避免注册冲突被静默掩盖。
    // 保留策略：按名排序后保留第一条（注册表层已保证内置工具优先于同名
    // 动态工具，这里是动态来源内部同名等残余冲突的兜底）。
    let mut deduped: Vec<ToolInfo> = Vec::with_capacity(tools.len());
    for tool in tools {
        match deduped.last() {
            Some(last) if last.name == tool.name => {
                if last.description != tool.description {
                    eprintln!(
                        "工具目录：检测到同名工具 '{}' 且描述不一致，保留先登记的一条（丢弃描述：{}）",
                        tool.name, tool.description
                    );
                } else {
                    eprintln!("工具目录：检测到重复工具 '{}'，已去重", tool.name);
                }
            }
            _ => deduped.push(tool),
        }
    }
    deduped
}
