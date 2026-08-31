use std::path::PathBuf;

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

use super::{
    import_chrome_profile_blocking, normalize_browser_url, plain_chat_browser_workspace,
    scan_chrome_profile_candidates_blocking, BrowserManager, BrowserProfileCandidate,
    BrowserProfileImportResult, BrowserStatus,
};

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
    let url = normalize_browser_url(url)?;
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

#[tauri::command]
pub async fn browser_minimize(
    app: AppHandle,
    manager: tauri::State<'_, BrowserManager>,
    session_id: String,
) -> Result<(), String> {
    let status = manager.minimize(&session_id).await?;
    let _ = app.emit("browser-status", status);
    Ok(())
}

#[tauri::command]
pub async fn browser_restore(
    app: AppHandle,
    manager: tauri::State<'_, BrowserManager>,
    session_id: String,
) -> Result<(), String> {
    let status = manager.restore(&session_id).await?;
    let _ = app.emit("browser-status", status);
    Ok(())
}

#[tauri::command]
pub async fn browser_reopen(
    app: AppHandle,
    manager: tauri::State<'_, BrowserManager>,
    session_id: String,
) -> Result<(), String> {
    let status = manager.reopen(&session_id).await?;
    let _ = app.emit("browser-status", status);
    Ok(())
}
