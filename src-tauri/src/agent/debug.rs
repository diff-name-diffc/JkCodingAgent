use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use anyhow::{Context, Result};
use chrono::Utc;
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

        if let Err(error) = append_log_entry(&self.project_root, event, metadata, sections) {
            eprintln!("failed to write context debug log: {error:#}");
        }
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
    let _guard = DEBUG_LOG_WRITE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap();

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
