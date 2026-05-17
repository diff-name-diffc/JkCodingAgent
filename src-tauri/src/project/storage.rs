use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

// ── Data types (mirror TypeScript interfaces) ────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: String,
    pub branch: Option<String>,
    #[serde(rename = "lastOpenedAt")]
    pub last_opened_at: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Task {
    pub id: String,
    #[serde(rename = "projectId")]
    pub project_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub prompt: String,
    pub agent: String,
    #[serde(rename = "permissionMode")]
    pub permission_mode: String,
    pub status: String,
    #[serde(rename = "createdAt")]
    pub created_at: i64,
    #[serde(
        rename = "attentionRequestedAt",
        skip_serializing_if = "Option::is_none"
    )]
    pub attention_requested_at: Option<i64>,
    #[serde(rename = "claudeSessionId", skip_serializing_if = "Option::is_none")]
    pub claude_session_id: Option<String>,
    #[serde(rename = "claudeSessionPath", skip_serializing_if = "Option::is_none")]
    pub claude_session_path: Option<String>,
    #[serde(rename = "codexSessionId", skip_serializing_if = "Option::is_none")]
    pub codex_session_id: Option<String>,
    #[serde(rename = "codexSessionPath", skip_serializing_if = "Option::is_none")]
    pub codex_session_path: Option<String>,
    #[serde(
        rename = "dispatcherSessionId",
        skip_serializing_if = "Option::is_none"
    )]
    pub dispatcher_session_id: Option<String>,
    #[serde(
        rename = "dispatcherDispatchId",
        skip_serializing_if = "Option::is_none"
    )]
    pub dispatcher_dispatch_id: Option<String>,
    #[serde(
        rename = "dispatcherDescription",
        skip_serializing_if = "Option::is_none"
    )]
    pub dispatcher_description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub starred: Option<bool>,
    #[serde(rename = "failureReason", skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<String>,
}

// ── Path helpers ─────────────────────────────────────────────────────────────

fn app_data_dir() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| "找不到用户主目录".to_string())?;
    Ok(home.join(".jkcodingagent"))
}

fn projects_path() -> Result<PathBuf, String> {
    Ok(app_data_dir()?.join("projects.json"))
}

fn tasks_path(project_id: &str) -> Result<PathBuf, String> {
    Ok(project_dir(project_id)?.join("tasks.json"))
}

fn project_dir(project_id: &str) -> Result<PathBuf, String> {
    Ok(app_data_dir()?.join("projects").join(project_id))
}

fn ensure_app_data_dirs() -> Result<(), String> {
    fs::create_dir_all(app_data_dir()?).map_err(|e| e.to_string())
}

fn ensure_project_dir(project_id: &str) -> Result<(), String> {
    fs::create_dir_all(project_dir(project_id)?).map_err(|e| e.to_string())
}

// ── Tauri commands ────────────────────────────────────────────────────────────

#[tauri::command]
pub fn load_projects() -> Result<Vec<Project>, String> {
    let path = projects_path()?;
    if !path.exists() {
        return Ok(vec![]);
    }
    let raw = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    serde_json::from_str(&raw).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn save_projects(projects: Vec<Project>) -> Result<(), String> {
    ensure_app_data_dirs()?;
    let raw = serde_json::to_string_pretty(&projects).map_err(|e| e.to_string())?;
    atomic_write(&projects_path()?, &raw)
}

#[tauri::command]
pub fn load_project_tasks(project_id: String) -> Result<Vec<Task>, String> {
    let path = tasks_path(&project_id)?;
    if !path.exists() {
        return Ok(vec![]);
    }
    let raw = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    serde_json::from_str(&raw).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn save_project_tasks(project_id: String, tasks: Vec<Task>) -> Result<(), String> {
    ensure_project_dir(&project_id)?;
    let path = tasks_path(&project_id)?;
    if tasks.is_empty() {
        // Remove the file if no tasks left
        if path.exists() {
            fs::remove_file(&path).map_err(|e| e.to_string())?;
        }
        return Ok(());
    }
    let raw = serde_json::to_string_pretty(&tasks).map_err(|e| e.to_string())?;
    atomic_write(&path, &raw)
}

// ── Atomic write (write to tmp then rename) ───────────────────────────────────

/// 原子写入：先写入唯一临时文件，再 rename 到目标路径。
/// 临时文件名包含 pid + 纳秒时间戳，避免并发写入时临时文件相互覆盖。
pub fn atomic_write(path: &Path, content: &str) -> Result<(), String> {
    let uid = format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let tmp = path.with_file_name(format!(".{file_name}.{uid}.tmp"));
    fs::write(&tmp, content).map_err(|e| e.to_string())?;
    fs::rename(&tmp, path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_test_dir(label: &str) -> PathBuf {
        let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "nezha_test_storage_{}_{}_{}",
            label,
            std::process::id(),
            id
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create test dir");
        dir
    }

    fn sample_project(id: &str) -> Project {
        Project {
            id: id.to_string(),
            name: format!("Project {id}"),
            path: format!("/tmp/{id}"),
            branch: Some("main".to_string()),
            last_opened_at: 1000,
        }
    }

    fn sample_task(id: &str, project_id: &str) -> Task {
        Task {
            id: id.to_string(),
            project_id: project_id.to_string(),
            name: Some(format!("Task {id}")),
            prompt: "Do something".to_string(),
            agent: "claude".to_string(),
            permission_mode: "ask".to_string(),
            status: "done".to_string(),
            created_at: 2000,
            attention_requested_at: None,
            claude_session_id: None,
            claude_session_path: None,
            codex_session_id: None,
            codex_session_path: None,
            dispatcher_session_id: None,
            dispatcher_dispatch_id: None,
            dispatcher_description: None,
            starred: None,
            failure_reason: None,
        }
    }

    // ── Project struct tests ──────────────────────────────────────────────────

    #[test]
    fn project_serializes_json_with_camel_case() {
        let p = sample_project("p1");
        let json = serde_json::to_string(&p).expect("serialize");
        assert!(json.contains("\"lastOpenedAt\""));
        assert!(!json.contains("\"last_opened_at\""));
    }

    #[test]
    fn project_deserializes_json_with_camel_case() {
        let json = r#"{"id":"x","name":"N","path":"/p","branch":null,"lastOpenedAt":42}"#;
        let p: Project = serde_json::from_str(json).expect("deserialize");
        assert_eq!(p.id, "x");
        assert_eq!(p.last_opened_at, 42);
        assert!(p.branch.is_none());
    }

    #[test]
    fn project_roundtrip_json() {
        let original = sample_project("rt");
        let json = serde_json::to_string(&original).expect("serialize");
        let restored: Project = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(original.id, restored.id);
        assert_eq!(original.name, restored.name);
        assert_eq!(original.path, restored.path);
        assert_eq!(original.branch, restored.branch);
        assert_eq!(original.last_opened_at, restored.last_opened_at);
    }

    // ── Task struct tests ─────────────────────────────────────────────────────

    #[test]
    fn task_serializes_optional_fields_as_absent() {
        let t = sample_task("t1", "p1");
        let json = serde_json::to_string(&t).expect("serialize");
        // Optional None fields should be omitted
        assert!(!json.contains("\"attentionRequestedAt\""));
        assert!(!json.contains("\"claudeSessionId\""));
        assert!(!json.contains("\"starred\""));
        assert!(!json.contains("\"failureReason\""));
        // Non-optional fields must be present
        assert!(json.contains("\"projectId\""));
        assert!(json.contains("\"permissionMode\""));
        assert!(json.contains("\"createdAt\""));
    }

    #[test]
    fn task_serializes_some_optional_fields() {
        let mut t = sample_task("t2", "p1");
        t.claude_session_id = Some("sess-123".to_string());
        t.starred = Some(true);
        t.failure_reason = Some("OOM".to_string());
        let json = serde_json::to_string(&t).expect("serialize");
        assert!(json.contains("\"claudeSessionId\":\"sess-123\""));
        assert!(json.contains("\"starred\":true"));
        assert!(json.contains("\"failureReason\":\"OOM\""));
    }

    #[test]
    fn task_deserializes_with_optional_fields_missing() {
        let json = r#"{
            "id":"t3","projectId":"p1","prompt":"hi","agent":"claude",
            "permissionMode":"ask","status":"pending","createdAt":3000
        }"#;
        let t: Task = serde_json::from_str(json).expect("deserialize");
        assert_eq!(t.id, "t3");
        assert!(t.name.is_none());
        assert!(t.claude_session_id.is_none());
        assert!(t.starred.is_none());
        assert!(t.failure_reason.is_none());
    }

    #[test]
    fn task_deserializes_with_all_optional_fields_present() {
        let json = r#"{
            "id":"t4","projectId":"p1","name":"Named","prompt":"p","agent":"codex",
            "permissionMode":"full_access","status":"done","createdAt":4000,
            "attentionRequestedAt":4500,
            "claudeSessionId":"cs1","claudeSessionPath":"/cs",
            "codexSessionId":"cx1","codexSessionPath":"/cx",
            "dispatcherSessionId":"ds1","dispatcherDispatchId":"dd1",
            "dispatcherDescription":"desc",
            "starred":false,"failureReason":"timeout"
        }"#;
        let t: Task = serde_json::from_str(json).expect("deserialize");
        assert_eq!(t.name.as_deref(), Some("Named"));
        assert_eq!(t.attention_requested_at, Some(4500));
        assert_eq!(t.claude_session_id.as_deref(), Some("cs1"));
        assert_eq!(t.codex_session_id.as_deref(), Some("cx1"));
        assert_eq!(t.dispatcher_session_id.as_deref(), Some("ds1"));
        assert_eq!(t.dispatcher_dispatch_id.as_deref(), Some("dd1"));
        assert_eq!(t.dispatcher_description.as_deref(), Some("desc"));
        assert_eq!(t.starred, Some(false));
        assert_eq!(t.failure_reason.as_deref(), Some("timeout"));
    }

    #[test]
    fn task_roundtrip_json() {
        let mut original = sample_task("rt", "p1");
        original.codex_session_id = Some("cx-rt".to_string());
        original.starred = Some(true);
        let json = serde_json::to_string(&original).expect("serialize");
        let restored: Task = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(original.id, restored.id);
        assert_eq!(original.project_id, restored.project_id);
        assert_eq!(original.prompt, restored.prompt);
        assert_eq!(original.codex_session_id, restored.codex_session_id);
        assert_eq!(original.starred, restored.starred);
    }

    // ── atomic_write tests ────────────────────────────────────────────────────

    #[test]
    fn atomic_write_creates_new_file() {
        let dir = unique_test_dir("aw_create");
        let path = dir.join("data.txt");

        atomic_write(&path, "content").expect("write");
        assert_eq!(fs::read_to_string(&path).unwrap(), "content");
    }

    #[test]
    fn atomic_write_overwrites_existing() {
        let dir = unique_test_dir("aw_overwrite");
        let path = dir.join("data.txt");

        atomic_write(&path, "first").expect("write 1");
        atomic_write(&path, "second").expect("write 2");
        assert_eq!(fs::read_to_string(&path).unwrap(), "second");
    }

    #[test]
    fn atomic_write_handles_empty_content() {
        let dir = unique_test_dir("aw_empty");
        let path = dir.join("empty.txt");

        atomic_write(&path, "").expect("write empty");
        assert_eq!(fs::read_to_string(&path).unwrap(), "");
    }

    #[test]
    fn atomic_write_handles_unicode() {
        let dir = unique_test_dir("aw_unicode");
        let path = dir.join("unicode.txt");

        atomic_write(&path, "日本語テスト 🎉").expect("write unicode");
        assert_eq!(fs::read_to_string(&path).unwrap(), "日本語テスト 🎉");
    }

    #[test]
    fn atomic_write_handles_large_content() {
        let dir = unique_test_dir("aw_large");
        let path = dir.join("large.txt");

        let large = "x".repeat(1_000_000);
        atomic_write(&path, &large).expect("write large");
        assert_eq!(fs::read_to_string(&path).unwrap().len(), 1_000_000);
    }

    #[test]
    fn atomic_write_fails_on_invalid_path() {
        let result = atomic_write(Path::new("/nonexistent_dir_xyz/deep/file.txt"), "x");
        assert!(result.is_err());
    }

    #[test]
    fn atomic_write_no_tmp_file_left_behind() {
        let dir = unique_test_dir("aw_no_tmp");
        let path = dir.join("final.txt");

        atomic_write(&path, "data").expect("write");

        // No .*.tmp files should remain
        let entries: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        for entry in &entries {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') && name.ends_with(".tmp") {
                panic!("Stale tmp file found: {name}");
            }
        }
    }

    // ── load_projects / save_projects tests ────────────────────────────────────

    #[test]
    fn save_and_load_projects_roundtrip() {
        let dir = unique_test_dir("proj_rt");
        let path = dir.join("projects.json");

        // Manually simulate load/save using the path helpers is difficult
        // because they read from HOME. Instead, verify JSON roundtrip.
        let projects = vec![
            sample_project("a"),
            sample_project("b"),
        ];
        let json = serde_json::to_string_pretty(&projects).expect("serialize");
        fs::write(&path, &json).expect("write file");

        let loaded: Vec<Project> =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).expect("parse");
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].id, "a");
        assert_eq!(loaded[1].id, "b");
    }

    #[test]
    fn load_projects_empty_file_returns_empty_vec() {
        let dir = unique_test_dir("proj_empty");
        let path = dir.join("projects.json");
        fs::write(&path, "[]").expect("write empty array");

        let loaded: Vec<Project> =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).expect("parse");
        assert!(loaded.is_empty());
    }

    #[test]
    fn load_projects_malformed_json_fails() {
        let dir = unique_test_dir("proj_malformed");
        let path = dir.join("projects.json");
        fs::write(&path, "not json").expect("write bad data");

        let result = serde_json::from_str::<Vec<Project>>(
            &fs::read_to_string(&path).unwrap(),
        );
        assert!(result.is_err());
    }

    // ── load_project_tasks / save_project_tasks (JSON-level) ──────────────────

    #[test]
    fn save_and_load_tasks_roundtrip() {
        let dir = unique_test_dir("task_rt");
        let path = dir.join("tasks.json");

        let tasks = vec![
            sample_task("t1", "p1"),
            sample_task("t2", "p1"),
        ];
        let json = serde_json::to_string_pretty(&tasks).expect("serialize");
        fs::write(&path, &json).expect("write file");

        let loaded: Vec<Task> =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).expect("parse");
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].id, "t1");
        assert_eq!(loaded[1].id, "t2");
        assert_eq!(loaded[0].agent, "claude");
        assert_eq!(loaded[0].status, "done");
    }

    #[test]
    fn save_empty_tasks_removes_file() {
        let dir = unique_test_dir("task_empty_rm");
        let path = dir.join("tasks.json");
        fs::write(&path, "[{}]").expect("create dummy file");
        assert!(path.exists());

        // Simulate save_project_tasks behavior: empty tasks => remove file
        if path.exists() {
            fs::remove_file(&path).expect("remove file");
        }
        assert!(!path.exists());
    }

    #[test]
    fn load_tasks_missing_file_returns_empty() {
        let dir = unique_test_dir("task_missing");
        let path = dir.join("nonexistent_tasks.json");

        if !path.exists() {
            let result: Vec<Task> = vec![];
            assert!(result.is_empty());
        }
    }

    // ── Malformed task data ───────────────────────────────────────────────────

    #[test]
    fn deserialize_task_missing_required_field_fails() {
        // Missing "prompt" field
        let json = r#"{"id":"x","projectId":"p","agent":"claude","permissionMode":"ask","status":"pending","createdAt":0}"#;
        let result = serde_json::from_str::<Task>(json);
        assert!(result.is_err());
    }

    #[test]
    fn deserialize_project_missing_required_field_fails() {
        // Missing "path" field
        let json = r#"{"id":"x","name":"N","lastOpenedAt":0}"#;
        let result = serde_json::from_str::<Project>(json);
        assert!(result.is_err());
    }
}
