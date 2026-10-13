use std::path::PathBuf;

use tauri::{AppHandle, Manager};
use tokio::process::Command;

use crate::platform::get_login_shell_path;

pub(super) fn plain_chat_browser_workspace() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or_else(|| "找不到用户主目录".to_string())?;
    let workspace = home.join(".jkcodingagent").join("plain-chat-browser");
    std::fs::create_dir_all(workspace.join(".jkcodingagent"))
        .map_err(|error| format!("创建普通聊天浏览器工作区失败：{error}"))?;
    Ok(workspace)
}

pub(super) fn resolve_driver_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dev_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/browser-sidecar/driver.py");
    if dev_path.is_file() {
        return Ok(dev_path);
    }
    let path = app
        .path()
        .resource_dir()
        .map_err(|error| format!("解析应用资源目录失败：{error}"))?
        .join("browser-sidecar/driver.py");
    if path.is_file() {
        Ok(path)
    } else {
        Err(format!(
            "找不到 browser-use Python 服务：{}",
            path.display()
        ))
    }
}

pub(super) fn sidecar_command(driver: &std::path::Path) -> Command {
    let mut command = if let Some(python) = std::env::var_os("JKC_BROWSER_PYTHON") {
        Command::new(python)
    } else {
        let uv = std::env::var_os("JKC_BROWSER_UV")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::split_paths(&get_login_shell_path())
                    .map(|dir| dir.join(if cfg!(windows) { "uv.exe" } else { "uv" }))
                    .find(|path| path.is_file())
            })
            .unwrap_or_else(|| PathBuf::from("uv"));
        let mut command = Command::new(uv);
        command.args([
            "run",
            "--no-project",
            "--python",
            "3.12",
            "--with",
            "browser-use==0.13.11",
            "python",
        ]);
        command
    };
    command
        .arg("-u")
        .arg(driver)
        .env("PATH", get_login_shell_path())
        .env("PYTHONUNBUFFERED", "1")
        .env("ANONYMIZED_TELEMETRY", "false")
        .env("BROWSER_USE_CLOUD_SYNC", "false")
        .env("BROWSER_USE_VERSION_CHECK", "false")
        .env("BROWSER_USE_DISABLE_EXTENSIONS", "1")
        .env("BROWSER_USE_LOGGING_LEVEL", "error");
    command
}
