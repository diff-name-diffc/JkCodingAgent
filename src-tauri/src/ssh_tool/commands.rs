use russh::Disconnect;
use tauri::State;

use super::validation::validate_single_server;
use super::{connect, SshAuditLog, SshServerConfig, SshSessionManager, SshToolsConfig};

#[tauri::command]
pub async fn ssh_tool_load_config(
    manager: State<'_, SshSessionManager>,
) -> Result<SshToolsConfig, String> {
    manager.load_config_async().await
}

#[tauri::command]
pub async fn ssh_tool_load_audit(
    manager: State<'_, SshSessionManager>,
) -> Result<SshAuditLog, String> {
    manager.load_audit_async().await
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

/// 面向用户/智能体的展示名：优先显示名称（name），留空回退为 id。
fn display_name(server: &SshServerConfig) -> &str {
    if server.name.is_empty() {
        &server.id
    } else {
        &server.name
    }
}
