use std::collections::HashSet;
use std::path::PathBuf;

use super::{SshAuthMethod, SshServerConfig, SshToolsConfig};

pub(super) const MAX_TIMEOUT_SECS: u64 = 300;
pub(super) const MAX_OUTPUT_BYTES: usize = 1024 * 1024;
pub(super) const ID_MAX_LEN: usize = 64;
const NAME_MAX_LEN: usize = 64;

pub(super) fn validate_config(mut config: SshToolsConfig) -> Result<SshToolsConfig, String> {
    let mut ids = HashSet::new();
    for server in &mut config.servers {
        normalize_server(server);
        validate_single_server_ref(server)?;
        if !ids.insert(server.id.clone()) {
            return Err(format!("SSH server id 重复：{}", server.id));
        }
    }
    Ok(config)
}

pub(super) fn validate_single_server(
    mut server: SshServerConfig,
) -> Result<SshServerConfig, String> {
    normalize_server(&mut server);
    validate_single_server_ref(&server)?;
    Ok(server)
}

fn normalize_server(server: &mut SshServerConfig) {
    server.id = server.id.trim().to_string();
    server.name = server.name.trim().to_string();
    server.host = server.host.trim().to_string();
    server.username = server.username.trim().to_string();
    server.password = server.password.trim().to_string();
    server.private_key_path = server.private_key_path.trim().to_string();
    server.private_key_passphrase = server.private_key_passphrase.trim().to_string();
    server.description = server.description.trim().to_string();
    server.tags.retain(|tag| !tag.trim().is_empty());
    server.tags = server
        .tags
        .iter()
        .map(|tag| tag.trim().to_string())
        .collect();
    server.default_timeout_secs = server.default_timeout_secs.clamp(1, MAX_TIMEOUT_SECS);
    server.max_output_bytes = server.max_output_bytes.clamp(1, MAX_OUTPUT_BYTES);
}

fn validate_single_server_ref(server: &SshServerConfig) -> Result<(), String> {
    validate_server_id(&server.id)?;
    if server.name.chars().count() > NAME_MAX_LEN {
        return Err(format!(
            "SSH server {} 的名称不能超过 {NAME_MAX_LEN} 个字符",
            server.id
        ));
    }
    if server.host.is_empty() {
        return Err(format!("SSH server {} 缺少 host", server.id));
    }
    if server.username.is_empty() {
        return Err(format!("SSH server {} 缺少 username", server.id));
    }
    match server.auth_method {
        SshAuthMethod::Password => {
            if server.password.is_empty() {
                return Err(format!("SSH server {} 缺少 password", server.id));
            }
        }
        SshAuthMethod::Key => {
            if server.private_key_path.is_empty() {
                return Err(format!(
                    "SSH server {} 缺少 private_key_path（密钥文件路径）",
                    server.id
                ));
            }
            let resolved = expand_key_path(&server.private_key_path);
            if !resolved.is_file() {
                return Err(format!(
                    "SSH server {} 的密钥文件不存在或不是普通文件：{}",
                    server.id, server.private_key_path
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn validate_server_id(id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("SSH server id 不能为空".to_string());
    }
    if id.len() > ID_MAX_LEN {
        return Err(format!("SSH server id 不能超过 {ID_MAX_LEN} 个字符"));
    }
    if !id
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_')
    {
        return Err(format!(
            "SSH server id 仅支持小写字母、数字、短横线和下划线：{id}"
        ));
    }
    Ok(())
}

pub(super) fn validate_session_id(id: &str) -> Result<(), String> {
    if id.trim().is_empty() {
        return Err("错误：session_id 不能为空".to_string());
    }
    if id.len() > ID_MAX_LEN {
        return Err(format!("错误：session_id 不能超过 {ID_MAX_LEN} 个字符"));
    }
    if !id
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.')
    {
        return Err("错误：session_id 仅支持字母、数字、短横线、下划线和点号".to_string());
    }
    Ok(())
}

pub(super) fn validate_command(command: &str) -> Result<(), String> {
    if command.trim().is_empty() {
        return Err("错误：command 不能为空".to_string());
    }
    if command.len() > 8192 {
        return Err("错误：command 长度不能超过 8192 字符".to_string());
    }
    Ok(())
}

/// 展开私钥路径开头的 `~/` 或 `~` 为用户主目录；已是绝对/相对路径时原样返回。
pub(super) fn expand_key_path(raw: &str) -> PathBuf {
    if let Some(rest) = raw.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    } else if raw == "~" {
        if let Some(home) = dirs::home_dir() {
            return home;
        }
    }
    PathBuf::from(raw)
}
