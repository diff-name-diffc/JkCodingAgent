use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::project::atomic_write;
use crate::project::storage::StorageError;
use crate::shared::error::{CommandResult, IntoCommandResult};

type AppSettingsResult<T> = std::result::Result<T, AppSettingsError>;

#[derive(Debug, thiserror::Error)]
pub enum AppSettingsError {
    #[error("找不到用户主目录")]
    HomeDirMissing,
    #[error("{action} 失败（{path}）：{source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{action} 失败（{path}）：{source}")]
    Json {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error(transparent)]
    Storage(#[from] StorageError),
}

fn io_error(
    action: &'static str,
    path: impl Into<PathBuf>,
) -> impl FnOnce(std::io::Error) -> AppSettingsError {
    move |source| AppSettingsError::Io {
        action,
        path: path.into(),
        source,
    }
}

fn json_error(
    action: &'static str,
    path: impl Into<PathBuf>,
) -> impl FnOnce(serde_json::Error) -> AppSettingsError {
    move |source| AppSettingsError::Json {
        action,
        path: path.into(),
        source,
    }
}

static LOGIN_SHELL_ENV: OnceLock<Vec<(String, String)>> = OnceLock::new();
static LOGIN_SHELL_PATH: OnceLock<String> = OnceLock::new();
const ENV_SENTINEL: &[u8] = b"__JKCODINGAGENT_ENV_START__\0";

/// 返回用户 login shell 导出的完整环境变量。
pub fn get_login_shell_env() -> &'static [(String, String)] {
    LOGIN_SHELL_ENV
        .get_or_init(resolve_login_shell_env)
        .as_slice()
}

/// 返回用户 login shell 解析后的完整 PATH。
pub fn get_login_shell_path() -> &'static str {
    LOGIN_SHELL_PATH.get_or_init(|| {
        get_login_shell_env()
            .iter()
            .find(|(key, _)| key == "PATH")
            .map(|(_, value)| value.clone())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(build_fallback_path)
    })
}

fn resolve_login_shell_env() -> Vec<(String, String)> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    read_shell_env(&shell, true)
        .or_else(|| read_shell_env(&shell, false))
        .unwrap_or_else(build_fallback_env)
}

fn read_shell_env(shell: &str, interactive: bool) -> Option<Vec<(String, String)>> {
    let args: &[&str] = if interactive {
        &[
            "-l",
            "-i",
            "-c",
            "printf '__JKCODINGAGENT_ENV_START__\\0'; env -0",
        ]
    } else {
        &[
            "-l",
            "-c",
            "printf '__JKCODINGAGENT_ENV_START__\\0'; env -0",
        ]
    };

    let output = Command::new(shell)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_shell_env_output(&output.stdout)
}

fn parse_shell_env_output(stdout: &[u8]) -> Option<Vec<(String, String)>> {
    let start = stdout
        .windows(ENV_SENTINEL.len())
        .position(|window| window == ENV_SENTINEL)?
        + ENV_SENTINEL.len();

    let env = stdout[start..]
        .split(|byte| *byte == 0)
        .filter_map(|entry| {
            let eq = entry.iter().position(|byte| *byte == b'=')?;
            let key = String::from_utf8_lossy(&entry[..eq]).into_owned();
            if key.is_empty() || matches!(key.as_str(), "PWD" | "OLDPWD" | "SHLVL" | "_") {
                return None;
            }
            Some((key, String::from_utf8_lossy(&entry[eq + 1..]).into_owned()))
        })
        .collect::<Vec<_>>();
    (!env.is_empty()).then_some(env)
}

fn build_fallback_path() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let current = std::env::var("PATH").unwrap_or_default();
    let mut parts = vec![
        format!("{home}/.local/bin"),
        format!("{home}/.npm-global/bin"),
        "/opt/homebrew/bin".to_string(),
        "/opt/homebrew/sbin".to_string(),
        "/usr/local/bin".to_string(),
        "/usr/bin".to_string(),
        "/bin".to_string(),
        "/usr/sbin".to_string(),
        "/sbin".to_string(),
    ];
    for path in current.split(':').filter(|path| !path.is_empty()) {
        if !parts.iter().any(|existing| existing == path) {
            parts.push(path.to_string());
        }
    }
    parts.join(":")
}

fn build_fallback_env() -> Vec<(String, String)> {
    let mut env = std::env::vars()
        .filter(|(key, _)| !matches!(key.as_str(), "PWD" | "OLDPWD" | "SHLVL" | "_"))
        .collect::<Vec<_>>();
    if let Some((_, path)) = env.iter_mut().find(|(key, _)| key == "PATH") {
        *path = build_fallback_path();
    } else {
        env.push(("PATH".to_string(), build_fallback_path()));
    }
    env
}

fn default_theme() -> String {
    "system".to_string()
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AppSettings {
    #[serde(default = "default_theme")]
    pub theme: String,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: default_theme(),
        }
    }
}

fn app_data_dir() -> AppSettingsResult<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".jkcodingagent"))
        .ok_or(AppSettingsError::HomeDirMissing)
}

fn settings_path() -> AppSettingsResult<PathBuf> {
    Ok(app_data_dir()?.join("settings.json"))
}

fn load_settings() -> AppSettingsResult<AppSettings> {
    let path = settings_path()?;
    if !path.exists() {
        return Ok(AppSettings::default());
    }
    let raw = fs::read_to_string(&path).map_err(io_error("读取应用设置", path.clone()))?;
    let value: serde_json::Value =
        serde_json::from_str(&raw).map_err(json_error("解析应用设置", path.clone()))?;
    let contains_legacy_cli_fields = value.as_object().is_some_and(|object| {
        object.contains_key("claude_path") || object.contains_key("codex_path")
    });
    let settings: AppSettings =
        serde_json::from_value(value).map_err(json_error("解析应用设置", path.clone()))?;
    if contains_legacy_cli_fields {
        if let Err(error) = save_app_settings_impl(settings.clone()) {
            eprintln!("[settings] 清理遗留 CLI 配置字段失败：{error}");
        }
    }
    Ok(settings)
}

#[tauri::command]
pub fn load_app_settings() -> CommandResult<AppSettings> {
    load_settings()
        .context("加载应用设置失败")
        .into_command_result()
}

#[tauri::command]
pub fn save_app_settings(settings: AppSettings) -> CommandResult<()> {
    save_app_settings_impl(settings)
        .context("保存应用设置失败")
        .into_command_result()
}

fn save_app_settings_impl(settings: AppSettings) -> AppSettingsResult<()> {
    let dir = app_data_dir()?;
    fs::create_dir_all(&dir).map_err(io_error("创建应用数据目录", dir))?;
    let path = settings_path()?;
    let raw = serde_json::to_string_pretty(&settings)
        .map_err(json_error("序列化应用设置", path.clone()))?;
    atomic_write(&path, &raw)?;
    Ok(())
}
