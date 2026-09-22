use std::path::PathBuf;

use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use super::{
    import_chrome_profile_blocking, normalize_browser_url, plain_chat_browser_workspace,
    scan_chrome_profile_candidates_blocking, BrowserManager, BrowserProfileCandidate,
    BrowserProfileImportResult, BrowserStatus,
};

/// 解析前端传入的项目路径：空白/缺省回退到纯聊天浏览器的固定工作区；
/// 非空路径经 `validate_project_workspace` 校验（canonicalize + 已注册
/// 工作区包含校验）——浏览器 profile 目录由该路径派生
/// （`{project_path}/.jkcodingagent/browser-profile`，import 时还会整目录
/// 读写），不得放行越权路径（与 G11-03 的执行入口同一闸门）。
async fn resolve_project_path(
    app: &AppHandle,
    project_path: Option<String>,
) -> Result<String, String> {
    match project_path {
        Some(path) if !path.trim().is_empty() => {
            let validated = app
                .state::<crate::agent::DispatcherState>()
                .validate_project_workspace(&path)
                .await?;
            Ok(validated.to_string_lossy().into_owned())
        }
        _ => Ok(plain_chat_browser_workspace()?
            .to_string_lossy()
            .into_owned()),
    }
}

#[tauri::command]
pub async fn browser_start(
    app: AppHandle,
    manager: tauri::State<'_, BrowserManager>,
    session_id: String,
    project_path: Option<String>,
) -> Result<BrowserStatus, String> {
    let project_path = resolve_project_path(&app, project_path).await?;
    manager.start(app, session_id, project_path).await
}

#[tauri::command]
pub async fn browser_import_chrome_profile(
    app: AppHandle,
    manager: tauri::State<'_, BrowserManager>,
    session_id: String,
    project_path: Option<String>,
    chrome_profile_path: String,
) -> Result<BrowserProfileImportResult, String> {
    manager.stop(&session_id).await?;
    let project_path = PathBuf::from(resolve_project_path(&app, project_path).await?);
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
    let project_path = resolve_project_path(&app, project_path).await?;
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
    let project_path = resolve_project_path(&app, project_path).await?;
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
    let url = normalize_browser_url(url)?;
    let project_path = resolve_project_path(&app, project_path).await?;
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
    let project_path = resolve_project_path(&app, project_path).await?;
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
