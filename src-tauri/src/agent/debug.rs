use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, SendError, Sender};
use std::sync::OnceLock;
use std::thread;

use anyhow::{Context, Result};
use chrono::Utc;
use serde::Serialize;

const DEBUG_LOG_DIR: &str = "logs";
const DEBUG_LOG_FILE: &str = "agent.debug";
const DEBUG_LOG_ARCHIVE_FILE: &str = "agent.debug.old";
/// 日志文件大小轮转阈值：超过后滚动为 agent.debug.old（仅保留最近一份）。
const MAX_DEBUG_LOG_BYTES: u64 = 10 * 1024 * 1024;

/// 后台写入线程的入队通道。log() 只做一次无锁 send，所有磁盘 I/O
/// 集中在专用写入线程中顺序执行，调用方不再持锁写盘。
static DEBUG_LOG_SENDER: OnceLock<Sender<DebugLogEntry>> = OnceLock::new();

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

/// 单条待写入日志，经通道移交给后台写入线程。
struct DebugLogEntry {
    project_root: PathBuf,
    event: String,
    metadata: Vec<(String, String)>,
    sections: Vec<DebugSection>,
}

impl ContextDebugLogger {
    pub(crate) fn new(enabled: bool, project_root: impl Into<PathBuf>) -> Self {
        Self {
            enabled,
            project_root: project_root.into(),
        }
    }

    pub(crate) fn enabled(&self) -> bool {
        self.enabled
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

        let entry = DebugLogEntry {
            project_root: self.project_root.clone(),
            event: event.to_string(),
            metadata,
            sections,
        };

        // 正常路径仅入队（不依赖 tokio runtime，任何线程均可调用）；
        // 仅当写入线程不可用（创建失败或意外退出）时才降级为同步写。
        if let Err(SendError(entry)) = debug_log_channel().send(entry) {
            if let Err(error) = write_log_entry_sync(&entry) {
                eprintln!("failed to write context debug log: {error:#}");
            }
        }
    }
}

pub(crate) fn render_json<T: Serialize + ?Sized>(value: &T) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|error| {
        // 错误信息可能含引号/反斜杠/换行，必须先 JSON 转义再嵌入，
        // 否则生成的 {"serializationError":...} 本身不是合法 JSON。
        let message = serde_json::to_string(&error.to_string())
            .unwrap_or_else(|_| "\"serialization error\"".to_string());
        format!("{{\"serializationError\":{message}}}")
    })
}

fn debug_log_channel() -> &'static Sender<DebugLogEntry> {
    DEBUG_LOG_SENDER.get_or_init(|| {
        let (sender, receiver) = mpsc::channel::<DebugLogEntry>();
        if let Err(error) = thread::Builder::new()
            .name("agent-debug-log-writer".to_string())
            .spawn(move || debug_log_writer_loop(receiver))
        {
            // receiver 随闭包被丢弃，后续 send 必然失败，log() 会降级为同步写。
            eprintln!("failed to spawn debug log writer thread: {error}");
        }
        sender
    })
}

/// 后台写入线程主循环：独占文件句柄，按入队顺序写入并逐条刷写。
fn debug_log_writer_loop(receiver: Receiver<DebugLogEntry>) {
    let mut writer = DebugLogWriter::new();
    for entry in receiver {
        if let Err(error) = writer.write_entry(&entry) {
            eprintln!("failed to write context debug log: {error:#}");
        }
    }
}

/// 写入线程独占的写入器：缓存当前项目的文件句柄，切换项目或触发轮转时重开。
struct DebugLogWriter {
    current: Option<(PathBuf, File)>,
}

impl DebugLogWriter {
    fn new() -> Self {
        Self { current: None }
    }

    fn write_entry(&mut self, entry: &DebugLogEntry) -> Result<()> {
        let log_dir = entry.project_root.join(DEBUG_LOG_DIR);
        let file = self.file_for_write(&log_dir)?;
        write_entry_body(file, entry)?;
        file.flush()?;
        Ok(())
    }

    fn file_for_write(&mut self, log_dir: &Path) -> Result<&mut File> {
        let log_path = log_dir.join(DEBUG_LOG_FILE);

        // 先用不可变借用判断缓存句柄是否可复用，避免与后续重开路径的借用冲突。
        let needs_reopen = match self.current.as_ref() {
            Some((dir, file)) if dir == log_dir => file
                .metadata()
                .map(|meta| meta.len() > MAX_DEBUG_LOG_BYTES)
                .unwrap_or(true),
            _ => true,
        };

        if !needs_reopen {
            return Ok(&mut self.current.as_mut().expect("checked above").1);
        }

        // 需要（重新）打开：先关闭旧句柄，再滚动超限文件，
        // 避免句柄仍指向已被 rename 的文件导致内容继续写入 .old。
        self.current = None;
        fs::create_dir_all(log_dir).with_context(|| format!("create {}", log_dir.display()))?;
        rotate_if_oversized(&log_path, log_dir);
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .with_context(|| format!("open {}", log_path.display()))?;
        self.current = Some((log_dir.to_path_buf(), file));
        Ok(&mut self.current.as_mut().expect("file handle just set").1)
    }
}

/// 超过大小阈值时把日志滚动为 agent.debug.old（仅保留最近一份）。
fn rotate_if_oversized(log_path: &Path, log_dir: &Path) {
    if let Ok(meta) = fs::metadata(log_path) {
        if meta.len() > MAX_DEBUG_LOG_BYTES {
            let _ = fs::rename(log_path, log_dir.join(DEBUG_LOG_ARCHIVE_FILE));
        }
    }
}

/// 写入线程不可用时的兜底同步写（无 tokio runtime 的上下文也走这里）。
fn write_log_entry_sync(entry: &DebugLogEntry) -> Result<()> {
    let log_dir = entry.project_root.join(DEBUG_LOG_DIR);
    fs::create_dir_all(&log_dir).with_context(|| format!("create {}", log_dir.display()))?;

    let log_path = log_dir.join(DEBUG_LOG_FILE);
    rotate_if_oversized(&log_path, &log_dir);
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("open {}", log_path.display()))?;
    write_entry_body(&mut file, entry)?;
    file.flush()?;
    Ok(())
}

fn write_entry_body(file: &mut File, entry: &DebugLogEntry) -> Result<()> {
    writeln!(
        file,
        "\n{}\n时间：{}\n事件：{}",
        "=".repeat(96),
        Utc::now().to_rfc3339(),
        entry.event
    )?;

    if !entry.metadata.is_empty() {
        writeln!(file, "元数据：")?;
        for (key, value) in &entry.metadata {
            writeln!(file, "- {key}：{value}")?;
        }
    }

    for section in &entry.sections {
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
    use super::render_json;

    #[test]
    fn render_json_escapes_serialization_error_message() {
        struct Unserializable;

        impl serde::Serialize for Unserializable {
            fn serialize<S: serde::Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
                Err(serde::ser::Error::custom(
                    "bad \"quote\" \\ backslash\nnewline",
                ))
            }
        }

        let rendered = render_json(&Unserializable);
        let parsed: serde_json::Value =
            serde_json::from_str(&rendered).expect("render_json 必须产出合法 JSON");
        let message = parsed["serializationError"]
            .as_str()
            .expect("serializationError 字段存在");
        assert!(message.contains("bad \"quote\""));
        assert!(message.contains('\n'));
    }

    #[test]
    fn render_json_serializes_normal_values() {
        let rendered = render_json(&serde_json::json!({"key": "值"}));
        assert!(rendered.contains("\"key\""));
        assert!(rendered.contains("值"));
    }
}
