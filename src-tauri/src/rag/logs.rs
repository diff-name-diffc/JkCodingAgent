//! RAG sidecar 运行日志的内存环形缓冲。
//!
//! 日志只用于设置页实时诊断，不落盘；超过容量后丢弃最旧记录。

use std::collections::VecDeque;

use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

const MAX_RAG_LOG_LINES: usize = 2000;
const RAG_LOG_EVENT: &str = "rag-log";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RagLogEntry {
    pub seq: u64,
    pub ts: i64,
    pub stream: RagLogStream,
    pub text: String,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RagLogStream {
    Stdout,
    Stderr,
    System,
}

#[derive(Default)]
pub struct RagLogStore {
    inner: Mutex<RagLogState>,
}

#[derive(Default)]
struct RagLogState {
    next_seq: u64,
    entries: VecDeque<RagLogEntry>,
}

impl RagLogStore {
    pub fn append(&self, app: &AppHandle, stream: RagLogStream, text: impl AsRef<str>) {
        let text = redact_log_text(text.as_ref()).trim_end().to_string();
        if text.is_empty() {
            return;
        }

        let entry = {
            let mut state = self.inner.lock();
            let entry = RagLogEntry {
                seq: state.next_seq,
                ts: chrono::Utc::now().timestamp_millis(),
                stream,
                text,
            };
            state.next_seq = state.next_seq.saturating_add(1);
            state.entries.push_back(entry.clone());
            while state.entries.len() > MAX_RAG_LOG_LINES {
                state.entries.pop_front();
            }
            entry
        };

        let _ = app.emit(RAG_LOG_EVENT, entry);
    }

    pub fn append_system(&self, app: &AppHandle, text: impl AsRef<str>) {
        self.append(app, RagLogStream::System, text);
    }

    pub fn snapshot(&self) -> Vec<RagLogEntry> {
        self.inner.lock().entries.iter().cloned().collect()
    }

    pub fn clear(&self) {
        self.inner.lock().entries.clear();
    }
}

fn redact_log_text(input: &str) -> String {
    input
        .split(' ')
        .map(redact_token)
        .collect::<Vec<_>>()
        .join(" ")
}

fn redact_token(token: &str) -> String {
    let lower = token.to_ascii_lowercase();
    if lower.starts_with("sk-") {
        return "sk-***".to_string();
    }
    if lower.starts_with("authorization:") {
        return "Authorization: ***".to_string();
    }
    if lower.starts_with("api_key=")
        || lower.starts_with("api-key=")
        || lower.starts_with("apikey=")
        || lower.starts_with("token=")
    {
        if let Some((key, _)) = token.split_once('=') {
            return format!("{key}=***");
        }
    }
    token.to_string()
}
