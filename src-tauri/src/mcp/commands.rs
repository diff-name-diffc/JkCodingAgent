//! MCP 相关 Tauri 命令：全局/项目状态查询、项目服务器启停、全局注册表读写。
//!
//! 项目命令一律先经 `validate_project_workspace` 校验（canonicalize +
//! 受管项目包含校验），拒绝前端传入的越权路径——状态查询只读文件、
//! 启停开关会写项目文件，两者都不允许触碰未注册目录。

use tauri::State;

use super::project_file::{
    read_project_mcp_config_sync, set_project_mcp_server_enabled_sync,
    write_project_mcp_config_sync,
};
use super::registry::McpRegistry;
use super::{McpConfig, McpScope, McpStatus};
use crate::agent::DispatcherState;

/// 刷新并返回 MCP 状态（原 mcp_project_status + mcp_global_status 合并）。
///
/// 作用域由参数分流：
/// - 传 `project_path`：项目作用域（全局 ∪ 项目文件合并视图）。路径先经
///   `validate_project_workspace` 校验（canonicalize + 受管项目包含校验），
///   拒绝越权路径；项目页低频打开且开关后需真实状态，**无条件全量探活**。
/// - 不传：全局作用域（所有聊天会话共享）。默认复用聊天 run 的新鲜窗口：
///   缓存在 `MCP_REFRESH_MAX_AGE` 内直接返回，过期才全量重检，供设置页
///   工具清单等展示面取数，避免每次打开页面都拉起全部服务器进程；
///   `force_refresh = true` 强制全量刷新（聊天页头部指示灯等需要真实
///   探活的场景）。
#[tauri::command]
pub async fn mcp_status(
    state: State<'_, DispatcherState>,
    registry: State<'_, McpRegistry>,
    project_path: Option<String>,
    force_refresh: Option<bool>,
) -> Result<McpStatus, String> {
    match project_path.as_deref().map(str::trim) {
        // 项目作用域：路径走 validate_project_workspace 校验后无条件全量探活。
        Some("") => Err("错误：project_path 不能为空字符串；不传则返回全局作用域".to_string()),
        Some(project_path) => {
            let scope = validated_project_scope(&state, project_path).await?;
            registry
                .refresh(&scope)
                .await
                .map(|snapshot| snapshot.status)
        }
        None => {
            let snapshot = if force_refresh.unwrap_or(false) {
                registry.refresh(&McpScope::Global).await?
            } else {
                registry.ensure_recent(&McpScope::Global).await?
            };
            Ok(snapshot.status)
        }
    }
}

/// 启用/禁用项目作用域内的服务器。项目文件没有该条目（来自全局注册表）
/// 时，把全局条目拷贝进项目文件作为覆盖（copy-on-write），保持
/// 「项目覆盖全局」的单一优先级规则。
#[tauri::command]
pub async fn mcp_project_set_server_enabled(
    state: State<'_, DispatcherState>,
    registry: State<'_, McpRegistry>,
    project_path: String,
    server_name: String,
    enabled: bool,
) -> Result<McpStatus, String> {
    let scope = validated_project_scope(&state, &project_path).await?;
    let McpScope::Project(project_root) = &scope else {
        unreachable!("validated_project_scope 只会构造 Project 作用域")
    };
    let project_root = project_root.clone();
    let global_db = registry.db().clone();
    tokio::task::spawn_blocking({
        let project_root = project_root.clone();
        let server_name = server_name.clone();
        move || match set_project_mcp_server_enabled_sync(&project_root, &server_name, enabled) {
            Ok(()) => Ok(()),
            Err(error) => {
                let global = global_db
                    .get_global_mcp_config()
                    .map_err(|error| error.to_string())?;
                let Some(mut server) = global.servers.get(&server_name).cloned() else {
                    return Err(error);
                };
                server.enabled = Some(enabled);
                let loaded = read_project_mcp_config_sync(&project_root)?;
                let mut config = loaded.config?;
                config.servers.insert(server_name, server);
                write_project_mcp_config_sync(&project_root, &config)
            }
        }
    })
    .await
    .map_err(|error| error.to_string())??;

    registry
        .refresh(&scope)
        .await
        .map(|snapshot| snapshot.status)
}

/// 读取全局 MCP 注册表（设置中心「MCP 服务器」页数据源）。
#[tauri::command]
pub async fn mcp_global_config_get(state: State<'_, DispatcherState>) -> Result<McpConfig, String> {
    let db = state.db().clone();
    tokio::task::spawn_blocking(move || db.get_global_mcp_config())
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())
}

/// 整列表保存全局 MCP 注册表；成功后清空全部作用域缓存，下一轮
/// `ensure_recent` 重新按「全局 ∪ 项目」合并。
#[tauri::command]
pub async fn mcp_global_config_save(
    state: State<'_, DispatcherState>,
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

/// 校验前端传入的项目路径并构造项目作用域（canonicalize 失败/未注册即报错）。
async fn validated_project_scope(
    state: &DispatcherState,
    project_path: &str,
) -> Result<McpScope, String> {
    let canonical = state.validate_project_workspace(project_path).await?;
    Ok(McpScope::Project(canonical))
}
