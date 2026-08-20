use std::process::{Command, Stdio};
use std::sync::OnceLock;

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::shared::error::{CommandResult, IntoCommandResult};

type AppSettingsResult<T> = std::result::Result<T, AppSettingsError>;

#[derive(Debug, thiserror::Error)]
pub enum AppSettingsError {
    #[error("{action} 失败：{source}")]
    Db {
        action: &'static str,
        #[source]
        source: anyhow::Error,
    },
    #[error("{action} 失败：{source}")]
    Json {
        action: &'static str,
        #[source]
        source: serde_json::Error,
    },
}

fn db_error(
    action: &'static str,
) -> impl FnOnce(anyhow::Error) -> AppSettingsError {
    move |source| AppSettingsError::Db { action, source }
}

fn json_error(action: &'static str) -> impl FnOnce(serde_json::Error) -> AppSettingsError {
    move |source| AppSettingsError::Json { action, source }
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

/// 应用设置存全局库 app_config 表（键 `app_settings`）；v33 迁移已把
/// 旧 settings.json 导入并删除，这里不再读写任何文件。
#[tauri::command]
pub fn load_app_settings(
    state: tauri::State<'_, crate::agent::DispatcherState>,
) -> CommandResult<AppSettings> {
    load_settings(state)
        .context("加载应用设置失败")
        .into_command_result()
}

fn load_settings(
    state: tauri::State<'_, crate::agent::DispatcherState>,
) -> AppSettingsResult<AppSettings> {
    match state
        .db()
        .get_app_config_json(crate::agent::db::app_config::APP_SETTINGS_KEY)
    {
        Ok(Some(raw)) => {
            serde_json::from_str(&raw).map_err(json_error("解析应用设置"))
        }
        Ok(None) => Ok(AppSettings::default()),
        Err(error) => Err(db_error("读取应用设置")(error)),
    }
}

#[tauri::command]
pub fn save_app_settings(
    state: tauri::State<'_, crate::agent::DispatcherState>,
    settings: AppSettings,
) -> CommandResult<()> {
    save_app_settings_impl(state, settings)
        .context("保存应用设置失败")
        .into_command_result()
}

fn save_app_settings_impl(
    state: tauri::State<'_, crate::agent::DispatcherState>,
    settings: AppSettings,
) -> AppSettingsResult<()> {
    let raw =
        serde_json::to_string(&settings).map_err(json_error("序列化应用设置"))?;
    state
        .db()
        .set_app_config_json(crate::agent::db::app_config::APP_SETTINGS_KEY, &raw)
        .map_err(db_error("保存应用设置"))
}
