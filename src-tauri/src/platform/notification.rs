use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

use chrono::Utc;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::project::atomic_write;

// ── Security: hardcoded allowed notification source ──────────────────────────

// Remote notifications disabled for internal deployment
const _NOTIFICATIONS_URL: &str = "";
const FETCH_INTERVAL_SECS: i64 = 3600; // 1 hour
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

static NOTIFICATION_STORE_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

// ── Remote JSON types ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RemoteNotification {
    id: String,
    #[serde(rename = "type")]
    notif_type: String,
    level: String,
    title: String,
    body: String,
    url: Option<String>,
    created_at: String,
    expires_at: Option<String>,
    popup: bool,
    min_app_version: Option<String>,
    max_app_version: Option<String>,
}

// ── Local storage types ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct NotificationStore {
    read_ids: Vec<String>,
    last_fetched_at: Option<String>,
    cached_notifications: Option<Vec<RemoteNotification>>,
}

// ── Frontend-facing types ────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct NotificationItem {
    pub id: String,
    #[serde(rename = "notifType")]
    pub notif_type: String,
    pub level: String,
    pub title: String,
    pub body: String,
    pub url: Option<String>,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    pub popup: bool,
    #[serde(rename = "isRead")]
    pub is_read: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct NotificationResult {
    pub notifications: Vec<NotificationItem>,
    #[serde(rename = "unreadCount")]
    pub unread_count: usize,
    #[serde(rename = "hasUnreadPopup")]
    pub has_unread_popup: bool,
}

// ── Path helpers ─────────────────────────────────────────────────────────────

fn app_data_dir() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| "找不到用户主目录".to_string())?;
    Ok(home.join(".jkcodingagent"))
}

fn store_path() -> Result<PathBuf, String> {
    Ok(app_data_dir()?.join("notifications.json"))
}

// ── Storage I/O ──────────────────────────────────────────────────────────────

fn load_store() -> NotificationStore {
    let Ok(path) = store_path() else {
        return NotificationStore::default();
    };
    match fs::read_to_string(&path) {
        Ok(data) => serde_json::from_str(&data).unwrap_or_default(),
        Err(_) => NotificationStore::default(),
    }
}

fn save_store(store: &NotificationStore) -> Result<(), String> {
    let path = store_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(store).map_err(|e| e.to_string())?;
    atomic_write(&path, &json)
}

fn notification_store_mutex() -> &'static Mutex<()> {
    NOTIFICATION_STORE_MUTEX.get_or_init(|| Mutex::new(()))
}

fn update_store<T, F>(mutate: F) -> Result<T, String>
where
    F: FnOnce(&mut NotificationStore) -> Result<T, String>,
{
    let _guard = notification_store_mutex().lock();
    let mut store = load_store();
    let result = mutate(&mut store)?;
    save_store(&store)?;
    Ok(result)
}

// ── Utilities ────────────────────────────────────────────────────────────────

fn should_fetch(store: &NotificationStore) -> bool {
    if store.cached_notifications.is_none() {
        return true;
    }

    match &store.last_fetched_at {
        None => true,
        Some(ts) => match chrono::DateTime::parse_from_rfc3339(ts) {
            Ok(last) => {
                let elapsed = (Utc::now() - last.with_timezone(&Utc)).num_seconds();
                elapsed > FETCH_INTERVAL_SECS
            }
            Err(_) => true,
        },
    }
}

fn apply_fetched_notifications(store: &mut NotificationStore, remote: Vec<RemoteNotification>) {
    let remote_ids: HashSet<&str> = remote.iter().map(|n| n.id.as_str()).collect();
    store.read_ids.retain(|id| remote_ids.contains(id.as_str()));
    store.last_fetched_at = Some(Utc::now().to_rfc3339());
    store.cached_notifications = Some(remote);
}

/// Strip control characters (except newline) and limit length to prevent
/// oversized or crafted strings from reaching the UI.
fn sanitize_text(s: &str, max_len: usize) -> String {
    s.chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .take(max_len)
        .collect()
}

/// Only allow http(s) URLs — reject `javascript:`, `data:`, etc.
fn sanitize_url(url: &Option<String>) -> Option<String> {
    url.as_ref().and_then(|u| {
        let trimmed = u.trim();
        if trimmed.starts_with("https://") || trimmed.starts_with("http://") {
            Some(sanitize_text(trimmed, 2000))
        } else {
            None
        }
    })
}

/// Simple semver comparison (major.minor.patch).
fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let parse = |s: &str| -> Vec<u64> {
        s.split('.')
            .map(|part| part.parse::<u64>().unwrap_or(0))
            .collect()
    };
    let va = parse(a);
    let vb = parse(b);
    let max_len = va.len().max(vb.len());
    for i in 0..max_len {
        let a_part = va.get(i).copied().unwrap_or(0);
        let b_part = vb.get(i).copied().unwrap_or(0);
        match a_part.cmp(&b_part) {
            std::cmp::Ordering::Equal => continue,
            other => return other,
        }
    }
    std::cmp::Ordering::Equal
}

/// Check if a notification should be shown for the current app version & date.
fn is_valid(notif: &RemoteNotification, app_version: &str) -> bool {
    // Check expiry
    if let Some(expires) = &notif.expires_at {
        let today = Utc::now().format("%Y-%m-%d").to_string();
        if expires.as_str() < today.as_str() {
            return false;
        }
    }
    // Check min version
    if let Some(min_ver) = &notif.min_app_version {
        if compare_versions(app_version, min_ver) == std::cmp::Ordering::Less {
            return false;
        }
    }
    // Check max version
    if let Some(max_ver) = &notif.max_app_version {
        if compare_versions(app_version, max_ver) == std::cmp::Ordering::Greater {
            return false;
        }
    }
    true
}

// ── HTTP fetch (async, with strict guards) ───────────────────────────────────

async fn fetch_remote() -> Result<Vec<RemoteNotification>, String> {
    // Remote notifications disabled for internal deployment
    Ok(Vec::new())
}

// ── Tauri commands ───────────────────────────────────────────────────────────

#[tauri::command]
pub async fn get_notifications() -> Result<NotificationResult, String> {
    let mut store = tokio::task::spawn_blocking(load_store)
        .await
        .map_err(|e| e.to_string())?;

    let notifications = if should_fetch(&store) {
        match fetch_remote().await {
            Ok(remote) => {
                let cached_remote = remote.clone();
                store = tokio::task::spawn_blocking(move || {
                    update_store(|store| {
                        apply_fetched_notifications(store, cached_remote);
                        Ok(store.clone())
                    })
                })
                .await
                .map_err(|e| e.to_string())??;

                remote
            }
            Err(err) => {
                if let Some(cached) = store.cached_notifications.clone() {
                    cached
                } else {
                    return Err(err);
                }
            }
        }
    } else {
        store.cached_notifications.clone().unwrap_or_default()
    };

    let read_set: HashSet<&str> = store.read_ids.iter().map(|s| s.as_str()).collect();

    let items: Vec<NotificationItem> = notifications
        .iter()
        .filter(|n| is_valid(n, APP_VERSION))
        .map(|n| NotificationItem {
            id: sanitize_text(&n.id, 100),
            notif_type: sanitize_text(&n.notif_type, 50),
            level: sanitize_text(&n.level, 20),
            title: sanitize_text(&n.title, 200),
            body: sanitize_text(&n.body, 2000),
            url: sanitize_url(&n.url),
            created_at: sanitize_text(&n.created_at, 20),
            popup: n.popup,
            is_read: read_set.contains(n.id.as_str()),
        })
        .collect();

    let unread_count = items.iter().filter(|n| !n.is_read).count();
    let has_unread_popup = items.iter().any(|n| !n.is_read && n.popup);

    Ok(NotificationResult {
        notifications: items,
        unread_count,
        has_unread_popup,
    })
}

#[tauri::command]
pub async fn mark_notification_read(id: String) -> Result<(), String> {
    let sanitized_id = sanitize_text(&id, 100);
    tokio::task::spawn_blocking(move || {
        update_store(|store| {
            if !store.read_ids.contains(&sanitized_id) {
                store.read_ids.push(sanitized_id);
            }
            Ok(())
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn mark_all_notifications_read() -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        update_store(|store| {
            if let Some(cached) = store.cached_notifications.clone() {
                for n in cached {
                    if !store.read_ids.contains(&n.id) {
                        store.read_ids.push(n.id);
                    }
                }
            }
            Ok(())
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notification(id: &str) -> RemoteNotification {
        RemoteNotification {
            id: id.to_string(),
            notif_type: "info".to_string(),
            level: "info".to_string(),
            title: format!("title-{id}"),
            body: format!("body-{id}"),
            url: None,
            created_at: "2026-01-01".to_string(),
            expires_at: None,
            popup: false,
            min_app_version: None,
            max_app_version: None,
        }
    }

    #[test]
    fn apply_fetched_notifications_keeps_only_existing_read_ids_in_remote() {
        let mut store = NotificationStore {
            read_ids: vec!["keep".to_string(), "drop".to_string()],
            last_fetched_at: None,
            cached_notifications: None,
        };

        apply_fetched_notifications(&mut store, vec![notification("keep"), notification("new")]);

        assert_eq!(store.read_ids, vec!["keep".to_string()]);
        assert_eq!(store.cached_notifications.unwrap().len(), 2);
        assert!(store.last_fetched_at.is_some());
    }

    // ── sanitize_text ────────────────────────────────────────────────────────

    #[test]
    fn sanitize_text_keeps_normal_chars() {
        assert_eq!(sanitize_text("Hello World", 100), "Hello World");
    }

    #[test]
    fn sanitize_text_keeps_newlines() {
        assert_eq!(sanitize_text("line1\nline2", 100), "line1\nline2");
    }

    #[test]
    fn sanitize_text_strips_control_chars() {
        let input = "Hello\x00World\x07!";
        assert_eq!(sanitize_text(input, 100), "HelloWorld!");
    }

    #[test]
    fn sanitize_text_truncates_to_max_len() {
        let input = "abcdefghij";
        assert_eq!(sanitize_text(input, 5), "abcde");
    }

    #[test]
    fn sanitize_text_handles_empty() {
        assert_eq!(sanitize_text("", 100), "");
    }

    #[test]
    fn sanitize_text_truncates_zero() {
        assert_eq!(sanitize_text("abc", 0), "");
    }

    #[test]
    fn sanitize_text_preserves_unicode() {
        assert_eq!(sanitize_text("中文测试", 100), "中文测试");
    }

    #[test]
    fn sanitize_text_truncates_unicode_correctly() {
        // Characters, not bytes
        assert_eq!(sanitize_text("中文测试", 2), "中文");
    }

    // ── sanitize_url ─────────────────────────────────────────────────────────

    #[test]
    fn sanitize_url_accepts_https() {
        let result = sanitize_url(&Some("https://example.com".to_string()));
        assert_eq!(result, Some("https://example.com".to_string()));
    }

    #[test]
    fn sanitize_url_accepts_http() {
        let result = sanitize_url(&Some("http://example.com".to_string()));
        assert_eq!(result, Some("http://example.com".to_string()));
    }

    #[test]
    fn sanitize_url_rejects_javascript() {
        let result = sanitize_url(&Some("javascript:alert(1)".to_string()));
        assert_eq!(result, None);
    }

    #[test]
    fn sanitize_url_rejects_data_uri() {
        let result = sanitize_url(&Some("data:text/html,<h1>test</h1>".to_string()));
        assert_eq!(result, None);
    }

    #[test]
    fn sanitize_url_rejects_ftp() {
        let result = sanitize_url(&Some("ftp://files.example.com".to_string()));
        assert_eq!(result, None);
    }

    #[test]
    fn sanitize_url_returns_none_for_none() {
        assert_eq!(sanitize_url(&None), None);
    }

    #[test]
    fn sanitize_url_trims_whitespace() {
        let result = sanitize_url(&Some("  https://example.com  ".to_string()));
        assert_eq!(result, Some("https://example.com".to_string()));
    }

    #[test]
    fn sanitize_url_truncates_long_url() {
        let long_url = format!("https://example.com/{}", "a".repeat(3000));
        let result = sanitize_url(&Some(long_url.clone())).unwrap();
        assert!(result.len() <= 2000);
    }

    // ── compare_versions ─────────────────────────────────────────────────────

    #[test]
    fn compare_versions_equal() {
        assert_eq!(compare_versions("1.0.0", "1.0.0"), std::cmp::Ordering::Equal);
    }

    #[test]
    fn compare_versions_greater_major() {
        assert_eq!(compare_versions("2.0.0", "1.0.0"), std::cmp::Ordering::Greater);
    }

    #[test]
    fn compare_versions_greater_minor() {
        assert_eq!(compare_versions("1.2.0", "1.1.0"), std::cmp::Ordering::Greater);
    }

    #[test]
    fn compare_versions_greater_patch() {
        assert_eq!(compare_versions("1.0.2", "1.0.1"), std::cmp::Ordering::Greater);
    }

    #[test]
    fn compare_versions_less() {
        assert_eq!(compare_versions("0.9.0", "1.0.0"), std::cmp::Ordering::Less);
    }

    #[test]
    fn compare_versions_different_lengths() {
        // "1.0" vs "1.0.0" should be equal
        assert_eq!(compare_versions("1.0", "1.0.0"), std::cmp::Ordering::Equal);
    }

    #[test]
    fn compare_versions_one_part_vs_three() {
        assert_eq!(compare_versions("2", "1.9.9"), std::cmp::Ordering::Greater);
    }

    #[test]
    fn compare_versions_empty_strings() {
        assert_eq!(compare_versions("", ""), std::cmp::Ordering::Equal);
    }

    #[test]
    fn compare_versions_non_numeric_treated_as_zero() {
        assert_eq!(compare_versions("a.b.c", "0.0.0"), std::cmp::Ordering::Equal);
    }

    #[test]
    fn compare_versions_mixed() {
        assert_eq!(compare_versions("1.0", "0.9.9"), std::cmp::Ordering::Greater);
    }

    // ── is_valid ─────────────────────────────────────────────────────────────

    #[test]
    fn is_valid_no_constraints() {
        let notif = notification("1");
        assert!(is_valid(&notif, "1.0.0"));
    }

    #[test]
    fn is_valid_expired_notification() {
        let mut notif = notification("1");
        notif.expires_at = Some("2020-01-01".to_string());
        assert!(!is_valid(&notif, "1.0.0"));
    }

    #[test]
    fn is_valid_future_expiry() {
        let mut notif = notification("1");
        notif.expires_at = Some("2099-12-31".to_string());
        assert!(is_valid(&notif, "1.0.0"));
    }

    #[test]
    fn is_valid_min_version_met() {
        let mut notif = notification("1");
        notif.min_app_version = Some("1.0.0".to_string());
        assert!(is_valid(&notif, "2.0.0"));
    }

    #[test]
    fn is_valid_min_version_not_met() {
        let mut notif = notification("1");
        notif.min_app_version = Some("2.0.0".to_string());
        assert!(!is_valid(&notif, "1.0.0"));
    }

    #[test]
    fn is_valid_max_version_met() {
        let mut notif = notification("1");
        notif.max_app_version = Some("3.0.0".to_string());
        assert!(is_valid(&notif, "2.0.0"));
    }

    #[test]
    fn is_valid_max_version_exceeded() {
        let mut notif = notification("1");
        notif.max_app_version = Some("1.0.0".to_string());
        assert!(!is_valid(&notif, "2.0.0"));
    }

    #[test]
    fn is_valid_version_range_satisfied() {
        let mut notif = notification("1");
        notif.min_app_version = Some("1.0.0".to_string());
        notif.max_app_version = Some("3.0.0".to_string());
        assert!(is_valid(&notif, "2.0.0"));
    }

    #[test]
    fn is_valid_version_range_outside() {
        let mut notif = notification("1");
        notif.min_app_version = Some("2.0.0".to_string());
        notif.max_app_version = Some("3.0.0".to_string());
        assert!(!is_valid(&notif, "1.0.0"));
        assert!(!is_valid(&notif, "4.0.0"));
        assert!(is_valid(&notif, "2.5.0"));
    }

    // ── should_fetch ─────────────────────────────────────────────────────────

    #[test]
    fn should_fetch_when_no_cache() {
        let store = NotificationStore {
            read_ids: vec![],
            last_fetched_at: None,
            cached_notifications: None,
        };
        assert!(should_fetch(&store));
    }

    #[test]
    fn should_fetch_when_no_last_fetched() {
        let store = NotificationStore {
            read_ids: vec![],
            last_fetched_at: None,
            cached_notifications: Some(vec![]),
        };
        assert!(should_fetch(&store));
    }

    #[test]
    fn should_fetch_when_cache_is_old() {
        let store = NotificationStore {
            read_ids: vec![],
            last_fetched_at: Some("2020-01-01T00:00:00Z".to_string()),
            cached_notifications: Some(vec![]),
        };
        assert!(should_fetch(&store));
    }

    #[test]
    fn should_fetch_when_invalid_timestamp() {
        let store = NotificationStore {
            read_ids: vec![],
            last_fetched_at: Some("not-a-timestamp".to_string()),
            cached_notifications: Some(vec![]),
        };
        assert!(should_fetch(&store));
    }

    #[test]
    fn should_not_fetch_when_cache_is_recent() {
        let recent = chrono::Utc::now().to_rfc3339();
        let store = NotificationStore {
            read_ids: vec![],
            last_fetched_at: Some(recent),
            cached_notifications: Some(vec![]),
        };
        assert!(!should_fetch(&store));
    }

    // ── NotificationStore default ────────────────────────────────────────────

    #[test]
    fn notification_store_default() {
        let store = NotificationStore::default();
        assert!(store.read_ids.is_empty());
        assert!(store.last_fetched_at.is_none());
        assert!(store.cached_notifications.is_none());
    }

    #[test]
    fn notification_store_serializes_round_trip() {
        let store = NotificationStore {
            read_ids: vec!["a".to_string(), "b".to_string()],
            last_fetched_at: Some("2026-01-01T00:00:00Z".to_string()),
            cached_notifications: None,
        };
        let json = serde_json::to_string(&store).unwrap();
        let parsed: NotificationStore = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.read_ids, store.read_ids);
        assert_eq!(parsed.last_fetched_at, store.last_fetched_at);
    }

    // ── RemoteNotification serialization ─────────────────────────────────────

    #[test]
    fn remote_notification_serializes_with_type_rename() {
        let notif = notification("test-1");
        let json = serde_json::to_string(&notif).unwrap();
        assert!(json.contains("\"type\""));
        assert!(!json.contains("\"notif_type\""));
    }

    // ── NotificationResult construction ──────────────────────────────────────

    #[test]
    fn notification_result_counts_unread() {
        let items = [
            NotificationItem {
                id: "1".to_string(),
                notif_type: "info".to_string(),
                level: "info".to_string(),
                title: "t1".to_string(),
                body: "b1".to_string(),
                url: None,
                created_at: "2026-01-01".to_string(),
                popup: false,
                is_read: true,
            },
            NotificationItem {
                id: "2".to_string(),
                notif_type: "info".to_string(),
                level: "info".to_string(),
                title: "t2".to_string(),
                body: "b2".to_string(),
                url: None,
                created_at: "2026-01-01".to_string(),
                popup: true,
                is_read: false,
            },
            NotificationItem {
                id: "3".to_string(),
                notif_type: "info".to_string(),
                level: "info".to_string(),
                title: "t3".to_string(),
                body: "b3".to_string(),
                url: None,
                created_at: "2026-01-01".to_string(),
                popup: false,
                is_read: false,
            },
        ];
        let unread_count = items.iter().filter(|n| !n.is_read).count();
        let has_unread_popup = items.iter().any(|n| !n.is_read && n.popup);
        assert_eq!(unread_count, 2);
        assert!(has_unread_popup);
    }

    #[test]
    fn notification_result_no_unread_popup_when_read() {
        let items = [NotificationItem {
            id: "1".to_string(),
            notif_type: "info".to_string(),
            level: "info".to_string(),
            title: "t1".to_string(),
            body: "b1".to_string(),
            url: None,
            created_at: "2026-01-01".to_string(),
            popup: true,
            is_read: true,
        }];
        let has_unread_popup = items.iter().any(|n| !n.is_read && n.popup);
        assert!(!has_unread_popup);
    }

    // ── apply_fetched_notifications ──────────────────────────────────────────

    #[test]
    fn apply_fetched_clears_all_read_ids_when_none_in_remote() {
        let mut store = NotificationStore {
            read_ids: vec!["a".to_string(), "b".to_string()],
            last_fetched_at: None,
            cached_notifications: None,
        };
        apply_fetched_notifications(&mut store, vec![notification("c")]);
        assert!(store.read_ids.is_empty());
    }

    #[test]
    fn apply_fetched_sets_last_fetched_and_caches() {
        let mut store = NotificationStore::default();
        let remote = vec![notification("x"), notification("y")];
        apply_fetched_notifications(&mut store, remote);
        assert!(store.last_fetched_at.is_some());
        let cached = store.cached_notifications.unwrap();
        assert_eq!(cached.len(), 2);
    }
}
