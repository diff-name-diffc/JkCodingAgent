//! 项目级 MCP 配置：`<repo>/.jkcodingagent/mcp.json` 的读写。
//!
//! 文件随仓库提交、团队共享。缺失视为空配置（只读不建）；创建发生在
//! `init_project_config` 与显式写入（如项目内启停全局服务器的 copy-on-write）。

use std::path::Path;

use super::{McpConfig, McpServerConfig};
use crate::project::storage::atomic_write;

pub(super) const DEFAULT_PROJECT_MCP_CONFIG: &str = r#"{
  "mcpServers": {}
}
"#;

#[derive(Debug)]
pub(crate) struct LoadedMcpConfig {
    pub(crate) config_path: std::path::PathBuf,
    pub(crate) config: Result<McpConfig, String>,
}

pub fn ensure_project_mcp_file(project_path: &str) -> Result<(), String> {
    let config_dir = Path::new(project_path).join(".jkcodingagent");
    std::fs::create_dir_all(&config_dir).map_err(|error| error.to_string())?;
    let config_path = config_dir.join("mcp.json");
    if config_path.exists() {
        return Ok(());
    }
    atomic_write(&config_path, DEFAULT_PROJECT_MCP_CONFIG).map_err(|error| error.to_string())
}

pub(crate) fn read_project_mcp_config_sync(project_path: &Path) -> Result<LoadedMcpConfig, String> {
    let config_path = project_path.join(".jkcodingagent").join("mcp.json");
    // 只读不建：文件缺失视为空配置（工作区可只用全局注册表的服务器）。
    match std::fs::read_to_string(&config_path) {
        Ok(raw) => {
            let config = serde_json::from_str::<McpConfig>(&raw)
                .map_err(|error| format!("解析 {} 失败：{error}", config_path.display()));
            Ok(LoadedMcpConfig {
                config_path,
                config,
            })
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(LoadedMcpConfig {
            config_path,
            config: Ok(McpConfig::default()),
        }),
        Err(error) => Err(format!("读取 {} 失败：{error}", config_path.display())),
    }
}

pub(crate) fn write_project_mcp_config_sync(
    project_path: &Path,
    config: &McpConfig,
) -> Result<(), String> {
    ensure_project_mcp_file(project_path.to_string_lossy().as_ref())?;
    let config_path = project_path.join(".jkcodingagent").join("mcp.json");
    let raw = serde_json::to_string_pretty(config).map_err(|error| error.to_string())?;
    atomic_write(&config_path, &raw).map_err(|error| error.to_string())
}

pub(crate) fn set_project_mcp_server_enabled_sync(
    project_path: &Path,
    server_name: &str,
    enabled: bool,
) -> Result<(), String> {
    let loaded = read_project_mcp_config_sync(project_path)?;
    let mut config = loaded.config?;
    let server = config
        .servers
        .get_mut(server_name)
        .ok_or_else(|| format!("未找到 MCP server '{server_name}'"))?;
    server.enabled = Some(enabled);
    write_project_mcp_config_sync(project_path, &config)
}

pub(crate) fn server_enabled(config: &McpServerConfig) -> bool {
    config.enabled.unwrap_or(true)
}
