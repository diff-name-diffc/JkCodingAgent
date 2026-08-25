//! MCP 相关 Tauri 命令：项目状态刷新/启停、全局注册表读写。

use std::path::PathBuf;

use tauri::State;

use super::project_file::{
    read_project_mcp_config_sync, set_project_mcp_server_enabled_sync,
    write_project_mcp_config_sync,
};
use super::registry::McpRegistry;
use super::{McpConfig, McpStatus};

#[tauri::command]
pub async fn refresh_project_mcp_status(
    registry: State<'_, McpRegistry>,
    project_path: String,
) -> Result<McpStatus, String> {
    registry
        .refresh(&project_path)
        .await
        .map(|snapshot| snapshot.status)
}

#[tauri::command]
pub async fn set_project_mcp_server_enabled(
    registry: State<'_, McpRegistry>,
    project_path: String,
    server_name: String,
    enabled: bool,
) -> Result<McpStatus, String> {
    let project_path_buf = PathBuf::from(&project_path);
    let global_fallback = registry.attached_db();
    tokio::task::spawn_blocking({
        let project_path_buf = project_path_buf.clone();
        let server_name = server_name.clone();
        move || {
            match set_project_mcp_server_enabled_sync(&project_path_buf, &server_name, enabled) {
                Ok(()) => Ok(()),
                // 项目文件没有该 server（来自全局注册表）时，把全局条目
                // 拷贝进项目文件作为覆盖（copy-on-write），保持「项目覆盖全局」
                // 的单一优先级规则。
                Err(error) => {
                    let Some(db) = global_fallback else {
                        return Err(error);
                    };
                    let global = db
                        .get_global_mcp_config()
                        .map_err(|error| error.to_string())?;
                    let Some(mut server) = global.servers.get(&server_name).cloned() else {
                        return Err(error);
                    };
                    server.enabled = Some(enabled);
                    let loaded = read_project_mcp_config_sync(&project_path_buf)?;
                    let mut config = loaded.config?;
                    config.servers.insert(server_name, server);
                    write_project_mcp_config_sync(&project_path_buf, &config)
                }
            }
        }
    })
    .await
    .map_err(|error| error.to_string())??;

    registry
        .refresh(&project_path)
        .await
        .map(|snapshot| snapshot.status)
}

/// 读取全局 MCP 注册表（设置中心「MCP 服务器」页数据源）。
#[tauri::command]
pub async fn mcp_get_global_config(
    state: State<'_, crate::agent::DispatcherState>,
) -> Result<McpConfig, String> {
    let db = state.db().clone();
    tokio::task::spawn_blocking(move || db.get_global_mcp_config())
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())
}

/// 整列表保存全局 MCP 注册表；成功后清空全部工作区缓存，下一轮
/// ensure_recent 重新按「全局 ∪ 项目」合并。
#[tauri::command]
pub async fn mcp_save_global_config(
    state: State<'_, crate::agent::DispatcherState>,
    registry: State<'_, McpRegistry>,
    config: McpConfig,
) -> Result<McpConfig, String> {
    let db = state.db().clone();
    tokio::task::spawn_blocking(move || db.save_global_mcp_config(&config))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
    registry.invalidate_all();
    let db = state.db().clone();
    tokio::task::spawn_blocking(move || db.get_global_mcp_config())
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())
}
