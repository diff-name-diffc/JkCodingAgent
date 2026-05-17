use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result};
use chrono::Utc;
use parking_lot::Mutex;
use serde::Serialize;

const DEBUG_LOG_DIR: &str = "logs";
const DEBUG_LOG_FILE: &str = "agent.debug";

static DEBUG_LOG_WRITE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Debug, Clone)]
pub(crate) struct ContextDebugLogger {
    enabled: bool,
    project_root: PathBuf,
}

#[derive(Debug, Clone)]
pub(crate) struct DebugSection {
    title: String,
    body: String,
}

impl DebugSection {
    pub(crate) fn new(title: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
        }
    }
}

impl ContextDebugLogger {
    pub(crate) fn new(enabled: bool, project_root: impl Into<PathBuf>) -> Self {
        Self {
            enabled,
            project_root: project_root.into(),
        }
    }

    pub(crate) fn log(
        &self,
        event: &str,
        metadata: Vec<(String, String)>,
        sections: Vec<DebugSection>,
    ) {
        if !self.enabled {
            return;
        }

        let project_root = self.project_root.clone();
        let event = event.to_string();
        tokio::task::spawn_blocking(move || {
            if let Err(error) = append_log_entry(&project_root, &event, metadata, sections) {
                eprintln!("failed to write context debug log: {error:#}");
            }
        });
    }
}

pub(crate) fn render_json<T: Serialize + ?Sized>(value: &T) -> String {
    serde_json::to_string_pretty(value)
        .unwrap_or_else(|error| format!("{{\"serializationError\":\"{}\"}}", error))
}

fn append_log_entry(
    project_root: &Path,
    event: &str,
    metadata: Vec<(String, String)>,
    sections: Vec<DebugSection>,
) -> Result<()> {
    let _guard = DEBUG_LOG_WRITE_LOCK.get_or_init(|| Mutex::new(())).lock();

    let log_dir = project_root.join(DEBUG_LOG_DIR);
    fs::create_dir_all(&log_dir).with_context(|| format!("create {}", log_dir.display()))?;

    let log_path = log_dir.join(DEBUG_LOG_FILE);
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("open {}", log_path.display()))?;

    writeln!(
        file,
        "\n{}\n时间：{}\n事件：{}",
        "=".repeat(96),
        Utc::now().to_rfc3339(),
        event
    )?;

    if !metadata.is_empty() {
        writeln!(file, "元数据：")?;
        for (key, value) in metadata {
            writeln!(file, "- {key}：{value}")?;
        }
    }

    for section in sections {
        writeln!(file, "\n【{}】", section.title)?;
        writeln!(file, "{}", section.body)?;
        if !section.body.ends_with('\n') {
            writeln!(file)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_json_pretty_prints_object() {
        let value = serde_json::json!({"key": "value", "num": 42});
        let rendered = render_json(&value);
        assert!(rendered.contains("\"key\""));
        assert!(rendered.contains("\"value\""));
        assert!(rendered.contains("\"num\""));
        assert!(rendered.contains("42"));
        // Pretty-printed has newlines
        assert!(rendered.contains('\n'));
    }

    #[test]
    fn render_json_handles_array() {
        let value = serde_json::json!([1, 2, 3]);
        let rendered = render_json(&value);
        assert!(rendered.contains("1"));
        assert!(rendered.contains("2"));
        assert!(rendered.contains("3"));
    }

    #[test]
    fn render_json_handles_null() {
        let value = serde_json::Value::Null;
        let rendered = render_json(&value);
        assert_eq!(rendered, "null");
    }

    #[test]
    fn render_json_handles_empty_object() {
        let value = serde_json::json!({});
        let rendered = render_json(&value);
        assert_eq!(rendered, "{}");
    }

    #[test]
    fn render_json_handles_empty_array() {
        let value = serde_json::json!([]);
        let rendered = render_json(&value);
        assert_eq!(rendered, "[]");
    }

    #[test]
    fn render_json_handles_nested_structure() {
        let value = serde_json::json!({
            "messages": [{"role": "user", "content": "hello"}],
            "count": 1
        });
        let rendered = render_json(&value);
        assert!(rendered.contains("messages"));
        assert!(rendered.contains("user"));
        assert!(rendered.contains("hello"));
    }

    #[test]
    fn render_json_produces_valid_json() {
        let value = serde_json::json!({"key": "value", "nested": {"a": 1}});
        let rendered = render_json(&value);
        let parsed: serde_json::Value = serde_json::from_str(&rendered).expect("valid JSON");
        assert_eq!(parsed["key"], "value");
        assert_eq!(parsed["nested"]["a"], 1);
    }

    #[test]
    fn debug_section_new_stores_title_and_body() {
        let section = DebugSection::new("Test Title", "Test Body");
        assert_eq!(section.title, "Test Title");
        assert_eq!(section.body, "Test Body");
    }

    #[test]
    fn debug_section_new_accepts_string_refs() {
        let title = String::from("Dynamic Title");
        let body = String::from("Dynamic Body");
        let section = DebugSection::new(title, body);
        assert_eq!(section.title, "Dynamic Title");
        assert_eq!(section.body, "Dynamic Body");
    }

    #[test]
    fn context_debug_logger_new_stores_params() {
        let logger = ContextDebugLogger::new(true, "/tmp/test-project");
        assert!(logger.enabled);
        assert_eq!(logger.project_root, PathBuf::from("/tmp/test-project"));
    }

    #[test]
    fn context_debug_logger_new_disabled() {
        let logger = ContextDebugLogger::new(false, "/tmp/test");
        assert!(!logger.enabled);
    }

    #[test]
    fn context_debug_logger_log_does_nothing_when_disabled() {
        let logger = ContextDebugLogger::new(false, "/nonexistent");
        // Should not panic or error
        logger.log("test-event", vec![], vec![]);
    }
}
