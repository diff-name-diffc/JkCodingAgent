use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::Result;
use chrono::Utc;

use super::document;

pub static CANCEL_TOKENS: std::sync::LazyLock<std::sync::Mutex<HashMap<String, Arc<AtomicBool>>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));

pub(crate) fn is_cancelled(job_id: &str) -> bool {
    CANCEL_TOKENS
        .lock()
        .ok()
        .and_then(|map| map.get(job_id).map(|t| t.load(Ordering::Relaxed)))
        .unwrap_or(false)
}

pub(crate) fn set_cancel_token(job_id: &str) {
    if let Ok(mut map) = CANCEL_TOKENS.lock() {
        map.entry(job_id.to_string())
            .or_insert_with(|| Arc::new(AtomicBool::new(false)))
            .store(true, Ordering::Relaxed);
    }
}

pub(crate) fn remove_cancel_token(job_id: &str) {
    if let Ok(mut map) = CANCEL_TOKENS.lock() {
        map.remove(job_id);
    }
}

pub fn set_resource_dir_hint(dir: PathBuf) {
    document::set_resource_dir_hint(dir);
}

pub(crate) fn today() -> String {
    Utc::now().date_naive().to_string()
}

pub(crate) fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

pub(crate) fn truncate_chars(input: &str, max_chars: usize) -> String {
    let mut out = input.chars().take(max_chars).collect::<String>();
    if input.chars().count() > max_chars {
        out.push_str("\n...[truncated]");
    }
    out
}

pub(crate) fn slugify(input: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in input.trim().to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

pub(crate) fn title_from_slug(slug: &str) -> String {
    slug.replace(['-', '_'], " ")
        .split_whitespace()
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn yaml_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

pub(crate) fn page_slug(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|name| name.to_str())
        .map(slugify)
        .unwrap_or_default()
}

pub(crate) fn normalize_path_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

pub(crate) async fn spawn_blocking_string<T, F>(f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())
}