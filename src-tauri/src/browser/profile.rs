use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::{AppHandle, Manager};

use super::{
    read_global_browser_config, BrowserLaunchOptions, BrowserProfileCandidate,
    BrowserProfileImportResult,
};

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

pub(super) fn load_launch_options(
    app: &AppHandle,
    project_path: &Path,
    session_id: &str,
) -> Result<BrowserLaunchOptions, String> {
    let config = {
        let dispatcher = app.state::<crate::agent::DispatcherState>();
        read_global_browser_config(dispatcher.db())?
    };
    let base_profile_dir = project_path.join(".jkcodingagent").join("browser-profile");
    let (user_data_dir, profile_directory) =
        resolve_browser_profile_dir(&base_profile_dir, session_id)?;
    Ok(BrowserLaunchOptions {
        user_data_dir,
        profile_directory,
        config,
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

pub(super) fn import_chrome_profile_blocking(
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

pub(super) fn scan_chrome_profile_candidates_blocking() -> Vec<BrowserProfileCandidate> {
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
