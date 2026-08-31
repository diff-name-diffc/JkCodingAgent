use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager};

use crate::platform::get_login_shell_path;

pub(super) fn plain_chat_browser_workspace() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or_else(|| "找不到用户主目录".to_string())?;
    let workspace = home.join(".jkcodingagent").join("plain-chat-browser");
    std::fs::create_dir_all(workspace.join(".jkcodingagent")).map_err(|error| {
        format!(
            "创建普通聊天浏览器工作区失败（{}）：{error}",
            workspace.display()
        )
    })?;
    Ok(workspace)
}

pub(super) fn resolve_driver_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dev_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("resources")
        .join("browser-sidecar")
        .join("driver.mjs");
    if dev_path.exists() {
        return Ok(dev_path);
    }

    let resource_path = app
        .path()
        .resource_dir()
        .map_err(|error| format!("解析应用资源目录失败：{error}"))?
        .join("browser-sidecar")
        .join("driver.mjs");
    if resource_path.exists() {
        Ok(resource_path)
    } else {
        Err(format!(
            "找不到 CloakBrowser sidecar 脚本：{}",
            resource_path.display()
        ))
    }
}

pub(super) fn resolve_node_path(app: &AppHandle) -> PathBuf {
    if let Some(value) = std::env::var_os("JKC_BROWSER_NODE") {
        return PathBuf::from(value);
    }

    if let Ok(resource_dir) = app.path().resource_dir() {
        let bundled = resource_dir
            .join("node")
            .join("bin")
            .join(node_binary_name());
        if bundled.exists() {
            return bundled;
        }
    }

    find_executable_in_path(node_binary_name(), get_login_shell_path())
        .or_else(|| {
            std::env::var_os("PATH").and_then(|path| {
                find_executable_in_path(node_binary_name(), &path.to_string_lossy())
            })
        })
        .unwrap_or_else(|| PathBuf::from(node_binary_name()))
}

fn node_binary_name() -> &'static str {
    if cfg!(windows) {
        "node.exe"
    } else {
        "node"
    }
}

fn find_executable_in_path(binary: &str, path: &str) -> Option<PathBuf> {
    std::env::split_paths(path)
        .map(|dir| dir.join(binary))
        .find(|candidate| candidate.is_file())
}

pub(super) fn resolve_node_modules_hint(app: &AppHandle) -> String {
    let dev_modules = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("node_modules");
    if dev_modules.exists() {
        return dev_modules.to_string_lossy().to_string();
    }

    app.path()
        .resource_dir()
        .ok()
        .map(|path| path.join("node_modules").to_string_lossy().to_string())
        .unwrap_or_default()
}
