use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::Mutex as ParkingMutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{oneshot, Mutex};
use tokio::time::{timeout, Duration};

use crate::platform::get_login_shell_path;
use crate::project::config::{read_project_config, BrowserConfig};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserStatus {
    pub session_id: String,
    pub state: String,
    pub url: Option<String>,
    pub message: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BrowserFrameEvent {
    session_id: String,
    data: String,
    width: u32,
    height: u32,
}

#[derive(Default)]
pub struct BrowserManager {
    sessions: Mutex<HashMap<String, Arc<BrowserProcess>>>,
}

struct BrowserProcess {
    session_id: String,
    stdin: Mutex<ChildStdin>,
    child: Mutex<Child>,
    pending: Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>,
    status: ParkingMutex<BrowserStatus>,
    next_id: AtomicU64,
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
        let options = load_launch_options(&project_path, &session_id)?;
        if !options.config.enabled {
            return Err("CloakBrowser 已在项目配置中禁用：.jkcodingagent/config.toml [browser].enabled = false".to_string());
        }
        let profile_directory = options.profile_directory.clone();
        let user_data_dir = options.user_data_dir;

        let process = spawn_sidecar(&app, &session_id).await?;
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
                let status = status_from_value(&value).unwrap_or_else(|| process.status());
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
            })
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

impl BrowserProcess {
    fn status(&self) -> BrowserStatus {
        self.status.lock().clone()
    }

    async fn request_with_timeout(
        &self,
        method: &str,
        params: Value,
        request_timeout: Duration,
    ) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        let payload = json!({
            "id": id,
            "method": method,
            "params": params
        });
        let line = serde_json::to_string(&payload).map_err(|error| error.to_string())?;
        let write_result = async {
            let mut stdin = self.stdin.lock().await;
            stdin.write_all(line.as_bytes()).await?;
            stdin.write_all(b"\n").await?;
            stdin.flush().await
        }
        .await;

        if let Err(error) = write_result {
            self.pending.lock().await.remove(&id);
            return Err(format!("写入 CloakBrowser sidecar 请求失败：{error}"));
        }

        match timeout(request_timeout, rx).await {
            Ok(result) => result.map_err(|_| "CloakBrowser sidecar 响应通道已关闭".to_string())?,
            Err(_) => {
                self.pending.lock().await.remove(&id);
                let status = self.status();
                let status_detail = status
                    .message
                    .as_deref()
                    .filter(|message| !message.trim().is_empty())
                    .unwrap_or(&status.state);
                Err(format!(
                    "CloakBrowser 工具调用超时（method={method}，等待 {} 秒）。当前浏览器状态：{status_detail}",
                    request_timeout.as_secs()
                ))
            }
        }
    }

    async fn kill(&self) -> Result<(), String> {
        let mut child = self.child.lock().await;
        match child.kill().await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::InvalidInput => Ok(()),
            Err(error) => Err(format!("关闭 CloakBrowser sidecar 失败：{error}")),
        }
    }
}

fn empty_to_null(value: &str) -> Value {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        Value::Null
    } else {
        Value::String(trimmed.to_string())
    }
}

fn browser_command_timeout(params: &Value) -> Duration {
    let requested_ms = params
        .get("timeout")
        .and_then(Value::as_u64)
        .unwrap_or(30_000);
    let padded_ms = requested_ms.saturating_add(15_000).clamp(1_000, 180_000);
    Duration::from_millis(padded_ms)
}

fn unique_browser_profile_dir(base_dir: &Path, session_id: &str) -> Result<PathBuf, String> {
    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("生成浏览器 profile 目录失败：系统时间异常：{error}"))?
        .as_millis();
    let run_id = format!("{}-{timestamp_ms}", sanitize_profile_segment(session_id));
    Ok(base_dir.join("runs").join(run_id))
}

fn imported_chrome_profile_root(base_dir: &Path) -> PathBuf {
    base_dir.join("imported-chrome")
}

fn imported_chrome_profile_marker(base_dir: &Path) -> PathBuf {
    imported_chrome_profile_root(base_dir).join(".jkcodingagent-profile-name")
}

fn imported_chrome_profile_marker_for_root(profile_root: &Path) -> PathBuf {
    profile_root.join(".jkcodingagent-profile-name")
}

fn resolve_browser_profile_dir(
    base_dir: &Path,
    session_id: &str,
) -> Result<(PathBuf, Option<String>), String> {
    let imported_root = imported_chrome_profile_root(base_dir);
    let marker = imported_chrome_profile_marker(base_dir);
    if imported_root.exists() && marker.exists() {
        let profile_name = fs::read_to_string(&marker)
            .map_err(|error| format!("读取导入的 Chrome profile 标记失败：{error}"))?
            .trim()
            .to_string();
        if profile_name.is_empty() {
            return Err("导入的 Chrome profile 标记为空，请重新导入登录态".to_string());
        }
        return Ok((imported_root, Some(profile_name)));
    }

    Ok((unique_browser_profile_dir(base_dir, session_id)?, None))
}

fn sanitize_profile_segment(value: &str) -> String {
    let mut sanitized = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();

    if sanitized.is_empty() {
        sanitized = "session".to_string();
    }
    sanitized.truncate(80);
    sanitized
}

fn load_launch_options(
    project_path: &Path,
    session_id: &str,
) -> Result<BrowserLaunchOptions, String> {
    let config = read_project_config(project_path.to_string_lossy().to_string())?;
    let base_profile_dir = project_path.join(".jkcodingagent").join("browser-profile");
    let (user_data_dir, profile_directory) =
        resolve_browser_profile_dir(&base_profile_dir, session_id)?;
    Ok(BrowserLaunchOptions {
        user_data_dir,
        profile_directory,
        config: config.browser,
    })
}

fn selected_chrome_profile_dir(selected_path: &Path) -> Result<(PathBuf, String), String> {
    if selected_path.join("Local State").is_file() {
        let default_profile = selected_path.join("Default");
        if default_profile.is_dir() {
            return Ok((default_profile, "Default".to_string()));
        }
        return Err("选择的是 Chrome 根目录，但其中没有 Default profile 目录".to_string());
    }

    let profile_name = selected_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "无法识别所选 Chrome profile 目录名".to_string())?
        .to_string();
    let parent = selected_path
        .parent()
        .ok_or_else(|| "所选 Chrome profile 目录没有父目录".to_string())?;
    if !parent.join("Local State").is_file() {
        return Err(
            "请选择 Chrome profile 目录（如 Default / Profile 1），或 Chrome 用户数据根目录"
                .to_string(),
        );
    }
    Ok((selected_path.to_path_buf(), profile_name))
}

fn should_skip_profile_entry(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };

    matches!(
        name,
        "SingletonLock"
            | "SingletonSocket"
            | "SingletonCookie"
            | "Crashpad"
            | "BrowserMetrics"
            | "ShaderCache"
            | "GrShaderCache"
            | "GraphiteDawnCache"
            | "Cache"
            | "Code Cache"
            | "GPUCache"
    )
}

fn chrome_lock_indicators(source_root: &Path) -> Vec<PathBuf> {
    ["SingletonLock", "SingletonSocket", "SingletonCookie"]
        .iter()
        .map(|name| source_root.join(name))
        .filter(|path| path.exists())
        .collect()
}

fn copy_dir_recursive(source: &Path, target: &Path) -> Result<(), String> {
    fs::create_dir_all(target)
        .map_err(|error| format!("创建目标目录失败（{}）：{error}", target.display()))?;
    for entry in fs::read_dir(source)
        .map_err(|error| format!("读取目录失败（{}）：{error}", source.display()))?
    {
        let entry = entry.map_err(|error| format!("读取目录项失败：{error}"))?;
        let source_path = entry.path();
        if should_skip_profile_entry(&source_path) {
            continue;
        }
        let target_path = target.join(entry.file_name());
        let file_type = entry
            .file_type()
            .map_err(|error| format!("读取文件类型失败（{}）：{error}", source_path.display()))?;
        if file_type.is_dir() {
            copy_dir_recursive(&source_path, &target_path)?;
        } else if file_type.is_file() {
            fs::copy(&source_path, &target_path).map_err(|error| {
                format!(
                    "复制文件失败（{} → {}）：{error}",
                    source_path.display(),
                    target_path.display()
                )
            })?;
        }
    }
    Ok(())
}

fn import_chrome_profile_blocking(
    project_path: PathBuf,
    chrome_profile_path: PathBuf,
) -> Result<BrowserProfileImportResult, String> {
    let (source_profile, profile_name) = selected_chrome_profile_dir(&chrome_profile_path)?;
    let source_root = source_profile
        .parent()
        .ok_or_else(|| "无法定位 Chrome 用户数据根目录".to_string())?;

    let locks = chrome_lock_indicators(source_root);
    if !locks.is_empty() {
        let lock_list = locks
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join("、");
        return Err(format!(
            "检测到 Chrome Profile 仍在使用中，请完全退出 Google Chrome 后再导入。锁文件：{lock_list}"
        ));
    }

    let base_profile_dir = project_path.join(".jkcodingagent").join("browser-profile");
    let target_root = imported_chrome_profile_root(&base_profile_dir);
    if source_root == target_root || source_profile == target_root.join(&profile_name) {
        return Err("不能把导入目标目录作为 Chrome profile 来源".to_string());
    }

    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("生成 Chrome 登录态临时目录失败：系统时间异常：{error}"))?
        .as_millis();
    let staging_root = base_profile_dir.join(format!("imported-chrome.tmp-{timestamp_ms}"));
    if staging_root.exists() {
        fs::remove_dir_all(&staging_root).map_err(|error| {
            format!(
                "清理 Chrome 登录态临时目录失败（{}）：{error}",
                staging_root.display()
            )
        })?;
    }
    fs::create_dir_all(&staging_root).map_err(|error| {
        format!(
            "创建 Chrome 登录态临时目录失败（{}）：{error}",
            staging_root.display()
        )
    })?;

    fs::copy(
        source_root.join("Local State"),
        staging_root.join("Local State"),
    )
    .map_err(|error| format!("复制 Chrome Local State 失败：{error}"))?;
    copy_dir_recursive(&source_profile, &staging_root.join(&profile_name))?;
    fs::write(
        imported_chrome_profile_marker_for_root(&staging_root),
        &profile_name,
    )
    .map_err(|error| format!("写入 Chrome profile 标记失败：{error}"))?;

    if target_root.exists() {
        fs::remove_dir_all(&target_root).map_err(|error| {
            format!(
                "清理旧的 Chrome 登录态副本失败（{}）：{error}",
                target_root.display()
            )
        })?;
    }
    fs::rename(&staging_root, &target_root).map_err(|error| {
        let _ = fs::remove_dir_all(&staging_root);
        format!(
            "启用新的 Chrome 登录态副本失败（{} → {}）：{error}",
            staging_root.display(),
            target_root.display()
        )
    })?;

    Ok(BrowserProfileImportResult {
        profile_name,
        target_path: target_root.to_string_lossy().to_string(),
    })
}

fn chrome_user_data_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();

    if let Some(home) = dirs::home_dir() {
        #[cfg(target_os = "macos")]
        {
            let app_support = home.join("Library").join("Application Support");
            roots.push(app_support.join("Google").join("Chrome"));
            roots.push(app_support.join("Google").join("Chrome Canary"));
            roots.push(app_support.join("Chromium"));
        }

        #[cfg(target_os = "linux")]
        {
            let config = home.join(".config");
            roots.push(config.join("google-chrome"));
            roots.push(config.join("google-chrome-beta"));
            roots.push(config.join("google-chrome-unstable"));
            roots.push(config.join("chromium"));
        }
    }

    #[cfg(target_os = "windows")]
    {
        if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
            let local_app_data = PathBuf::from(local_app_data);
            roots.push(
                local_app_data
                    .join("Google")
                    .join("Chrome")
                    .join("User Data"),
            );
            roots.push(
                local_app_data
                    .join("Google")
                    .join("Chrome SxS")
                    .join("User Data"),
            );
            roots.push(local_app_data.join("Chromium").join("User Data"));
        }
    }

    roots
}

fn is_chrome_profile_dir_name(name: &str) -> bool {
    name == "Default"
        || name
            .strip_prefix("Profile ")
            .is_some_and(|suffix| !suffix.is_empty())
}

fn chrome_profile_sort_key(candidate: &BrowserProfileCandidate) -> (u8, u32, String) {
    if candidate.profile_name == "Default" {
        return (0, 0, candidate.profile_name.clone());
    }

    let number = candidate
        .profile_name
        .strip_prefix("Profile ")
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(u32::MAX);
    (1, number, candidate.profile_name.clone())
}

fn scan_chrome_profile_candidates_blocking() -> Vec<BrowserProfileCandidate> {
    let mut candidates = Vec::new();
    for root in chrome_user_data_roots() {
        if !root.join("Local State").is_file() {
            continue;
        }

        let Ok(entries) = fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() || !path.join("Preferences").is_file() {
                continue;
            }
            let Some(profile_name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if !is_chrome_profile_dir_name(profile_name) {
                continue;
            }
            candidates.push(BrowserProfileCandidate {
                profile_name: profile_name.to_string(),
                path: path.to_string_lossy().to_string(),
                user_data_root: root.to_string_lossy().to_string(),
            });
        }
    }

    candidates.sort_by_key(chrome_profile_sort_key);
    candidates
}

fn plain_chat_browser_workspace() -> Result<PathBuf, String> {
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

async fn spawn_sidecar(app: &AppHandle, session_id: &str) -> Result<Arc<BrowserProcess>, String> {
    let driver_path = resolve_driver_path(app)?;
    let node_path = resolve_node_path(app);
    let node_modules_hint = resolve_node_modules_hint(app);
    let shell_path = get_login_shell_path();

    let mut command = Command::new(&node_path);
    command
        .arg(&driver_path)
        .env("JKC_BROWSER_SESSION_ID", session_id)
        .env("JKC_BROWSER_NODE_MODULES", node_modules_hint)
        .env("PATH", shell_path)
        .env("NODE_NO_WARNINGS", "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    if let Some(cwd) = driver_path.parent() {
        command.current_dir(cwd);
    }

    let mut child = command.spawn().map_err(|error| {
        format!(
            "启动 CloakBrowser Node sidecar 失败：{error}。已尝试执行：{} {}",
            node_path.display(),
            driver_path.display()
        )
    })?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| "CloakBrowser sidecar stdin 不可用".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "CloakBrowser sidecar stdout 不可用".to_string())?;
    let stderr = child.stderr.take();

    let process = Arc::new(BrowserProcess {
        session_id: session_id.to_string(),
        stdin: Mutex::new(stdin),
        child: Mutex::new(child),
        pending: Mutex::new(HashMap::new()),
        status: ParkingMutex::new(BrowserStatus {
            session_id: session_id.to_string(),
            state: "starting".to_string(),
            url: None,
            message: Some("正在启动 CloakBrowser sidecar".to_string()),
        }),
        next_id: AtomicU64::new(1),
    });

    spawn_stdout_reader(app.clone(), Arc::clone(&process), stdout);
    if let Some(stderr) = stderr {
        spawn_stderr_reader(app.clone(), session_id.to_string(), stderr);
    }

    Ok(process)
}

fn spawn_stdout_reader(
    app: AppHandle,
    process: Arc<BrowserProcess>,
    stdout: tokio::process::ChildStdout,
) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                emit_log(&app, &process.session_id, format!("sidecar stdout: {line}"));
                continue;
            };
            if let Some(id) = value.get("id").and_then(Value::as_u64) {
                let result = if value.get("ok").and_then(Value::as_bool).unwrap_or(false) {
                    Ok(value.get("result").cloned().unwrap_or(Value::Null))
                } else {
                    Err(value
                        .get("error")
                        .and_then(Value::as_str)
                        .unwrap_or("CloakBrowser sidecar 返回未知错误")
                        .to_string())
                };
                if let Some(tx) = process.pending.lock().await.remove(&id) {
                    let _ = tx.send(result);
                }
                continue;
            }

            match value.get("event").and_then(Value::as_str) {
                Some("status") => {
                    if let Some(status) = status_from_value(&value) {
                        *process.status.lock() = status.clone();
                        let _ = app.emit("browser-status", status);
                    }
                }
                Some("frame") => {
                    let data = value
                        .get("data")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let width = value.get("width").and_then(Value::as_u64).unwrap_or(0) as u32;
                    let height = value.get("height").and_then(Value::as_u64).unwrap_or(0) as u32;
                    let _ = app.emit(
                        "browser-frame",
                        BrowserFrameEvent {
                            session_id: process.session_id.clone(),
                            data,
                            width,
                            height,
                        },
                    );
                }
                Some("log") => {
                    if let Some(message) = value.get("message").and_then(Value::as_str) {
                        emit_log(&app, &process.session_id, message.to_string());
                    }
                }
                _ => {}
            }
        }
        let closed = BrowserStatus {
            session_id: process.session_id.clone(),
            state: "closed".to_string(),
            url: process.status().url,
            message: Some("CloakBrowser sidecar 已退出".to_string()),
        };
        *process.status.lock() = closed.clone();
        let mut pending = process.pending.lock().await;
        for (_, tx) in pending.drain() {
            let _ = tx.send(Err("CloakBrowser sidecar 已退出".to_string()));
        }
        drop(pending);
        let _ = app.emit("browser-status", closed);
    });
}

fn spawn_stderr_reader(app: AppHandle, session_id: String, stderr: tokio::process::ChildStderr) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            emit_log(&app, &session_id, format!("sidecar stderr: {line}"));
        }
    });
}

fn emit_log(app: &AppHandle, session_id: &str, message: String) {
    let _ = app.emit(
        "browser-log",
        json!({
            "sessionId": session_id,
            "message": message
        }),
    );
}

fn status_from_value(value: &Value) -> Option<BrowserStatus> {
    let status_value = value.get("status").unwrap_or(value);
    let session_id = status_value
        .get("sessionId")
        .or_else(|| value.get("sessionId"))?
        .as_str()?
        .to_string();
    Some(BrowserStatus {
        session_id,
        state: status_value
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        url: status_value
            .get("url")
            .and_then(Value::as_str)
            .map(str::to_string),
        message: status_value
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

fn resolve_driver_path(app: &AppHandle) -> Result<PathBuf, String> {
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

fn resolve_node_path(app: &AppHandle) -> PathBuf {
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

fn resolve_node_modules_hint(app: &AppHandle) -> String {
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

#[tauri::command]
pub async fn browser_start(
    app: AppHandle,
    manager: tauri::State<'_, BrowserManager>,
    session_id: String,
    project_path: String,
) -> Result<BrowserStatus, String> {
    manager.start(app, session_id, project_path).await
}

#[tauri::command]
pub async fn browser_start_plain_chat(
    app: AppHandle,
    manager: tauri::State<'_, BrowserManager>,
    session_id: String,
) -> Result<BrowserStatus, String> {
    let workspace = plain_chat_browser_workspace()?;
    manager
        .start(app, session_id, workspace.to_string_lossy().to_string())
        .await
}

#[tauri::command]
pub async fn browser_import_chrome_profile(
    manager: tauri::State<'_, BrowserManager>,
    session_id: String,
    project_path: Option<String>,
    chrome_profile_path: String,
) -> Result<BrowserProfileImportResult, String> {
    manager.stop(&session_id).await?;
    let project_path = match project_path {
        Some(path) if !path.trim().is_empty() => PathBuf::from(path),
        _ => plain_chat_browser_workspace()?,
    };
    let chrome_profile_path = PathBuf::from(chrome_profile_path);

    tokio::task::spawn_blocking(move || {
        import_chrome_profile_blocking(project_path, chrome_profile_path)
    })
    .await
    .map_err(|error| format!("导入 Chrome 登录态任务失败：{error}"))?
}

#[tauri::command]
pub async fn browser_list_chrome_profile_candidates() -> Result<Vec<BrowserProfileCandidate>, String>
{
    tokio::task::spawn_blocking(scan_chrome_profile_candidates_blocking)
        .await
        .map_err(|error| format!("扫描 Chrome Profile 任务失败：{error}"))
}

#[tauri::command]
pub async fn browser_stop(
    manager: tauri::State<'_, BrowserManager>,
    session_id: String,
) -> Result<(), String> {
    manager.stop(&session_id).await
}

#[tauri::command]
pub async fn browser_click_at(
    app: AppHandle,
    manager: tauri::State<'_, BrowserManager>,
    session_id: String,
    project_path: Option<String>,
    x: f64,
    y: f64,
) -> Result<Value, String> {
    let project_path = match project_path {
        Some(path) if !path.trim().is_empty() => path,
        _ => plain_chat_browser_workspace()?
            .to_string_lossy()
            .to_string(),
    };
    manager
        .command(
            app,
            session_id,
            project_path,
            "click",
            json!({ "x": x, "y": y, "timeout": 30_000 }),
        )
        .await
}

#[tauri::command]
pub async fn browser_go_back(
    app: AppHandle,
    manager: tauri::State<'_, BrowserManager>,
    session_id: String,
    project_path: Option<String>,
) -> Result<Value, String> {
    let project_path = match project_path {
        Some(path) if !path.trim().is_empty() => path,
        _ => plain_chat_browser_workspace()?
            .to_string_lossy()
            .to_string(),
    };
    manager
        .command(
            app,
            session_id,
            project_path,
            "back",
            json!({ "timeout": 30_000 }),
        )
        .await
}

#[tauri::command]
pub async fn browser_navigate(
    app: AppHandle,
    manager: tauri::State<'_, BrowserManager>,
    session_id: String,
    url: String,
    project_path: Option<String>,
) -> Result<Value, String> {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err("URL 必须以 http:// 或 https:// 开头".to_string());
    }
    let project_path = match project_path {
        Some(path) if !path.trim().is_empty() => path,
        _ => plain_chat_browser_workspace()?
            .to_string_lossy()
            .to_string(),
    };
    manager
        .command(
            app,
            session_id,
            project_path,
            "open_url",
            json!({ "url": url, "timeout": 30_000 }),
        )
        .await
}

#[tauri::command]
pub async fn browser_reload(
    app: AppHandle,
    manager: tauri::State<'_, BrowserManager>,
    session_id: String,
    project_path: Option<String>,
) -> Result<Value, String> {
    let project_path = match project_path {
        Some(path) if !path.trim().is_empty() => path,
        _ => plain_chat_browser_workspace()?
            .to_string_lossy()
            .to_string(),
    };
    manager
        .command(
            app,
            session_id,
            project_path,
            "reload",
            json!({ "timeout": 30_000 }),
        )
        .await
}

#[tauri::command]
pub async fn browser_get_status(
    manager: tauri::State<'_, BrowserManager>,
    session_id: String,
) -> Result<BrowserStatus, String> {
    Ok(manager.status(&session_id).await)
}
