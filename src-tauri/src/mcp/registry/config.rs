//! 作用域配置加载与合并：全局注册表 ∪ 项目级 `mcp.json`。
//!
//! 合并结果的缓存新鲜度判定也在此处，与加载逻辑同属「配置来源」这一变化原因。

use std::path::PathBuf;
use std::time::Duration;

use crate::mcp::project_file::read_project_mcp_config_sync;
use crate::mcp::{McpConfig, McpScope};

pub(super) struct LoadedScopeConfig {
    pub project_path: Option<PathBuf>,
    pub config_path: Option<PathBuf>,
    pub config: Result<McpConfig, String>,
}

/// 加载作用域对应的合并配置。
///
/// - `Global`：只读全局注册表，完全不触碰文件系统；
/// - `Project`：全局 ∪ 项目级 `mcp.json`（缺失视为空），同名项目覆盖全局。
pub(super) fn load_merged_config(
    db: &crate::agent::db::DispatcherDb,
    scope: &McpScope,
) -> Result<LoadedScopeConfig, String> {
    let global = db
        .get_global_mcp_config()
        .map_err(|error| error.to_string())?;
    match scope {
        McpScope::Global => Ok(LoadedScopeConfig {
            project_path: None,
            config_path: None,
            config: Ok(global),
        }),
        McpScope::Project(path) => {
            let loaded = read_project_mcp_config_sync(path)?;
            let config = loaded.config.map(|project| merge_configs(global, project));
            Ok(LoadedScopeConfig {
                project_path: Some(path.to_path_buf()),
                config_path: Some(loaded.config_path),
                config,
            })
        }
    }
}

/// 合并全局与项目配置：项目同名服务器整体覆盖全局条目（不做字段级合并）。
pub(super) fn merge_configs(global: McpConfig, project: McpConfig) -> McpConfig {
    let mut servers = global.servers;
    servers.extend(project.servers);
    McpConfig { servers }
}

/// 快照新鲜度判定（checked_at 与 now 均为毫秒时间戳）。
pub(super) fn is_fresh(checked_at_ms: i64, now_ms: i64, max_age: Duration) -> bool {
    now_ms.saturating_sub(checked_at_ms) <= max_age.as_millis() as i64
}
