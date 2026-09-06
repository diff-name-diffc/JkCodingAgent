use russh::Disconnect;
use tauri::State;

use super::validation::validate_single_server;
use super::{
    connect, memo, SshAuditLog, SshMemoPayload, SshServerConfig, SshSessionManager,
    SshToolsConfig,
};

/// 设置页一次性载荷（原 ssh_tool_load_config + ssh_tool_load_audit 合并）：
/// 两个数据源在前端唯一消费点永远成对拉取，合并省一次 IPC 往返。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SshSettingsSnapshot {
    pub servers: Vec<SshServerConfig>,
    pub audit: SshAuditLog,
}

#[tauri::command]
pub async fn ssh_tool_load_settings(
    manager: State<'_, SshSessionManager>,
) -> Result<SshSettingsSnapshot, String> {
    let config = manager.load_config_async().await?;
    let audit = manager.load_audit_async().await?;
    Ok(SshSettingsSnapshot {
        servers: config.servers,
        audit,
    })
}

#[tauri::command]
pub async fn ssh_tool_save_config(
    manager: State<'_, SshSessionManager>,
    config: SshToolsConfig,
) -> Result<SshToolsConfig, String> {
    manager.save_config_async(config).await
}

#[tauri::command]
pub async fn ssh_tool_test_server_config(
    manager: State<'_, SshSessionManager>,
    server: SshServerConfig,
    reset_host_key: Option<bool>,
) -> Result<String, String> {
    let config = validate_single_server(server)?;
    if reset_host_key.unwrap_or(false) {
        let ssh_db = manager.db.clone();
        let server_id = config.id.clone();
        tokio::task::spawn_blocking(move || ssh_db.remove_host_key_pin(&server_id))
            .await
            .map_err(|error| error.to_string())??;
    }
    let handle = connect(&config, &manager.db).await?;
    let _ = handle
        .disconnect(Disconnect::ByApplication, "connection test completed", "en")
        .await;
    Ok(format!("连接成功：{}", display_name(&config)))
}

/// 读取指定服务器的运维备忘录（设置页展示）。服务器须存在（不限启用状态）；
/// 尚无备忘录时返回空内容（非错误）。
#[tauri::command]
pub async fn ssh_tool_get_memo(
    manager: State<'_, SshSessionManager>,
    server_id: String,
) -> Result<SshMemoPayload, String> {
    require_server_exists(&manager, &server_id).await?;
    tokio::task::spawn_blocking(move || memo::read_memo(&server_id))
        .await
        .map_err(|error| error.to_string())?
}

/// 人工保存运维备忘录（设置页编辑入口）。与 Agent 工具同一存储与上限
/// 约束（全文 8000 / 单段 4000 字符，超限拒绝）；空内容视为清空备忘录。
#[tauri::command]
pub async fn ssh_tool_save_memo(
    manager: State<'_, SshSessionManager>,
    server_id: String,
    content: String,
) -> Result<SshMemoPayload, String> {
    require_server_exists(&manager, &server_id).await?;
    tokio::task::spawn_blocking(move || memo::save_memo_full(&server_id, &content))
        .await
        .map_err(|error| error.to_string())?
}

async fn require_server_exists(
    manager: &SshSessionManager,
    server_id: &str,
) -> Result<(), String> {
    match manager.find_server_any_async(server_id).await? {
        Some(_) => Ok(()),
        None => Err(format!("未找到 SSH server：{server_id}")),
    }
}

/// 面向用户/智能体的展示名：优先显示名称（name），留空回退为 id。
fn display_name(server: &SshServerConfig) -> &str {
    if server.name.is_empty() {
        &server.id
    } else {
        &server.name
    }
}
