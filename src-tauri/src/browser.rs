use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

mod cleanup;
mod paths;
mod preview;
mod process;
mod profile;
mod runtime;
mod session;
#[cfg(test)]
mod tests;
mod url;

use paths::plain_chat_browser_workspace;
use profile::{
    import_chrome_profile_blocking, load_launch_options, scan_chrome_profile_candidates_blocking,
};
pub(crate) use url::normalize_browser_url;

/// 全局浏览器选项（app_config 表 `browser` 键）。机器级用户偏好，
/// 不随项目走；v33 前存放在项目 config.toml 的 [browser] 段。
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq)]
pub struct BrowserConfig {
    #[serde(default = "default_browser_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub proxy: String,
    #[serde(default)]
    pub locale: String,
    #[serde(default)]
    pub timezone: String,
    #[serde(default = "default_browser_viewport_width")]
    pub viewport_width: u32,
    #[serde(default = "default_browser_viewport_height")]
    pub viewport_height: u32,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            enabled: default_browser_enabled(),
            proxy: String::new(),
            locale: String::new(),
            timezone: String::new(),
            viewport_width: default_browser_viewport_width(),
            viewport_height: default_browser_viewport_height(),
        }
    }
}

fn default_browser_enabled() -> bool {
    true
}

fn default_browser_viewport_width() -> u32 {
    1280
}

fn default_browser_viewport_height() -> u32 {
    800
}

/// 读取全局浏览器配置；未配置时返回默认值。
pub fn read_global_browser_config(
    db: &crate::agent::db::DispatcherDb,
) -> Result<BrowserConfig, String> {
    match db.get_app_config_json(crate::agent::db::app_config::BROWSER_KEY) {
        Ok(Some(raw)) => serde_json::from_str::<BrowserConfig>(&raw)
            .map_err(|error| format!("解析全局浏览器配置失败：{error}")),
        Ok(None) => Ok(BrowserConfig::default()),
        Err(error) => Err(format!("读取全局浏览器配置失败：{error}")),
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserStatus {
    pub session_id: String,
    pub state: String,
    pub url: Option<String>,
    pub message: Option<String>,
}

#[derive(Default)]
pub struct BrowserManager {
    sessions: Arc<Mutex<HashMap<String, Arc<session::Session>>>>,
}

impl Drop for BrowserManager {
    fn drop(&mut self) {
        let sessions = self.sessions.lock().values().cloned().collect::<Vec<_>>();
        for session in sessions {
            if let Some(process) = session.process() {
                process.abort("应用退出，浏览器服务已终止");
            }
        }
    }
}

#[derive(Debug)]
struct BrowserLaunchOptions {
    user_data_dir: PathBuf,
    profile_directory: Option<String>,
    config: BrowserConfig,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserProfileImportResult {
    profile_name: String,
    target_path: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserProfileCandidate {
    profile_name: String,
    path: String,
    user_data_root: String,
}

/// 浏览器探索事件以运行标识隔离，调用方负责投影到统一子智能体轨迹。
pub type BrowserAgentEventCallback = Arc<dyn Fn(Value) + Send + Sync>;

pub(crate) mod commands;
