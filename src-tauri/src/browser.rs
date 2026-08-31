use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::AppHandle;
use tokio::sync::Mutex;
use tokio::time::Duration;

mod paths;
mod process;
mod profile;
mod url;

use paths::plain_chat_browser_workspace;
use process::{
    browser_command_timeout, empty_to_null, spawn_sidecar, status_from_value, BrowserProcess,
};
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
    #[serde(default)]
    pub minimized: bool,
    #[serde(default)]
    pub has_headed_window: bool,
}

#[derive(Default)]
pub struct BrowserManager {
    sessions: Mutex<HashMap<String, Arc<BrowserProcess>>>,
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

impl BrowserManager {
    pub async fn start(
        &self,
        app: AppHandle,
        session_id: String,
        project_path: String,
    ) -> Result<BrowserStatus, String> {
        if let Some(existing) = self.sessions.lock().await.get(&session_id).cloned() {
            return Ok(existing.status());
        }

        let project_path = PathBuf::from(project_path);
        let options = load_launch_options(&app, &project_path, &session_id)?;
        if !options.config.enabled {
            return Err("CloakBrowser 已在全局浏览器设置中禁用".to_string());
        }
        let profile_directory = options.profile_directory.clone();
        let user_data_dir = options.user_data_dir;
        let project_path_str = project_path.to_string_lossy().to_string();

        let process = spawn_sidecar(&app, &session_id, &project_path_str).await?;
        self.sessions
            .lock()
            .await
            .insert(session_id.clone(), Arc::clone(&process));

        let start_result = process
            .request_with_timeout(
                "start",
                json!({
                    "sessionId": session_id,
                    "userDataDir": user_data_dir,
                    "headless": true,
                    "proxy": empty_to_null(&options.config.proxy),
                    "locale": empty_to_null(&options.config.locale),
                    "timezone": empty_to_null(&options.config.timezone),
                    "profileDirectory": profile_directory,
                    "viewport": {
                        "width": options.config.viewport_width,
                        "height": options.config.viewport_height
                    }
                }),
                Duration::from_secs(180),
            )
            .await;

        match start_result {
            Ok(value) => {
                let status =
                    status_from_value(&value, false, false).unwrap_or_else(|| process.status());
                Ok(status)
            }
            Err(error) => {
                self.sessions.lock().await.remove(&process.session_id);
                let _ = process.kill().await;
                Err(error)
            }
        }
    }

    pub async fn stop(&self, session_id: &str) -> Result<(), String> {
        let process = self.sessions.lock().await.remove(session_id);
        let Some(process) = process else {
            return Ok(());
        };
        let _ = process
            .request_with_timeout("close", json!({}), Duration::from_secs(10))
            .await;
        process.kill().await
    }

    pub async fn status(&self, session_id: &str) -> BrowserStatus {
        self.sessions
            .lock()
            .await
            .get(session_id)
            .map(|process| process.status())
            .unwrap_or_else(|| BrowserStatus {
                session_id: session_id.to_string(),
                state: "closed".to_string(),
                url: None,
                message: None,
                minimized: false,
                has_headed_window: false,
            })
    }

    pub async fn minimize(&self, session_id: &str) -> Result<BrowserStatus, String> {
        let process = self
            .sessions
            .lock()
            .await
            .get(session_id)
            .cloned()
            .ok_or_else(|| format!("未找到会话：{session_id}"))?;
        process
            .request_with_timeout("minimize_window", json!({}), Duration::from_secs(10))
            .await?;
        {
            let mut s = process.status.lock();
            s.minimized = true;
            s.state = "minimized".to_string();
        }
        Ok(process.status())
    }

    pub async fn restore(&self, session_id: &str) -> Result<BrowserStatus, String> {
        let process = self
            .sessions
            .lock()
            .await
            .get(session_id)
            .cloned()
            .ok_or_else(|| format!("未找到会话：{session_id}"))?;
        process
            .request_with_timeout("restore_window", json!({}), Duration::from_secs(10))
            .await?;
        {
            let mut s = process.status.lock();
            s.minimized = false;
            s.state = "ready".to_string();
        }
        Ok(process.status())
    }

    pub async fn reopen(&self, session_id: &str) -> Result<BrowserStatus, String> {
        let process = self
            .sessions
            .lock()
            .await
            .get(session_id)
            .cloned()
            .ok_or_else(|| format!("未找到会话：{session_id}"))?;
        let result = process
            .request_with_timeout("reopen_window", json!({}), Duration::from_secs(15))
            .await?;
        let headed = result
            .get("headed")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        {
            let mut s = process.status.lock();
            s.minimized = false;
            s.state = "ready".to_string();
            if headed {
                s.has_headed_window = true;
            }
        }
        Ok(process.status())
    }

    pub async fn command(
        &self,
        app: AppHandle,
        session_id: String,
        project_path: String,
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        let existing = {
            let sessions = self.sessions.lock().await;
            sessions.get(&session_id).cloned()
        };

        let process = match existing {
            Some(process) => process,
            None => {
                self.start(app, session_id.clone(), project_path).await?;
                self.sessions
                    .lock()
                    .await
                    .get(&session_id)
                    .cloned()
                    .ok_or_else(|| "CloakBrowser sidecar 启动后未注册会话".to_string())?
            }
        };
        let request_timeout = browser_command_timeout(&params);
        process
            .request_with_timeout(method, params, request_timeout)
            .await
    }
}

pub(crate) mod commands;
