use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use parking_lot::Mutex;
use serde_json::Value;
use tauri::{AppHandle, Emitter};
use tokio::sync::oneshot;
use tokio::time::timeout;
use uuid::Uuid;

use crate::agent::cad::{
    DwgViewerCommand, DwgViewerCommandResult, DwgViewerOpenRequest, DwgViewerSessionRegistration,
    DwgViewerSessionState,
};

#[derive(Debug, Default)]
pub struct DwgViewerBridgeState {
    sessions: Mutex<HashMap<String, DwgViewerSessionState>>,
    pending_results: Mutex<HashMap<String, oneshot::Sender<DwgViewerCommandResult>>>,
}

impl DwgViewerBridgeState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn register_session(
        &self,
        mut registration: DwgViewerSessionRegistration,
    ) -> DwgViewerSessionState {
        registration.file_path = normalize_file_path(&registration.file_path);
        let state = registration.into_state();
        self.sessions
            .lock()
            .insert(state.session_id.clone(), state.clone());
        state
    }

    pub fn unregister_session(&self, session_id: &str) {
        self.sessions.lock().remove(session_id);
    }

    pub fn update_state(&self, state: DwgViewerSessionState) -> DwgViewerSessionState {
        let mut state = state;
        state.file_path = normalize_file_path(&state.file_path);
        self.sessions
            .lock()
            .insert(state.session_id.clone(), state.clone());
        state
    }

    pub fn get_session(&self, session_id: &str) -> Option<DwgViewerSessionState> {
        self.sessions.lock().get(session_id).cloned()
    }

    pub fn list_file_sessions(
        &self,
        workspace_id: &str,
        file_path: &str,
    ) -> Vec<DwgViewerSessionState> {
        let normalized = normalize_file_path(file_path);
        self.sessions
            .lock()
            .values()
            .filter(|state| state.workspace_id == workspace_id && state.file_path == normalized)
            .cloned()
            .collect()
    }

    pub fn best_session_for_file(
        &self,
        workspace_id: &str,
        file_path: &str,
    ) -> Option<DwgViewerSessionState> {
        let mut matches = self.list_file_sessions(workspace_id, file_path);
        matches.sort_by(|left, right| {
            right
                .active
                .cmp(&left.active)
                .then_with(|| right.visible.cmp(&left.visible))
                .then_with(|| right.updated_at.cmp(&left.updated_at))
        });
        matches.into_iter().next()
    }

    pub async fn request_open(
        &self,
        app: &AppHandle,
        workspace_id: &str,
        file_path: &str,
    ) -> Result<()> {
        let normalized = normalize_file_path(file_path);
        app.emit(
            "dwg-viewer/open-request",
            DwgViewerOpenRequest {
                workspace_id: workspace_id.to_string(),
                file_path: normalized,
            },
        )?;
        Ok(())
    }

    pub async fn wait_for_session(
        &self,
        workspace_id: &str,
        file_path: &str,
        timeout_duration: Duration,
    ) -> Result<DwgViewerSessionState> {
        let start = std::time::Instant::now();
        loop {
            if let Some(session) = self.best_session_for_file(workspace_id, file_path) {
                return Ok(session);
            }
            if start.elapsed() >= timeout_duration {
                return Err(anyhow!("等待 DWG Viewer 会话超时"));
            }
            tokio::time::sleep(Duration::from_millis(120)).await;
        }
    }

    pub async fn issue_command(
        &self,
        app: &AppHandle,
        session_id: &str,
        action: &str,
        payload: Value,
        timeout_duration: Duration,
    ) -> Result<DwgViewerCommandResult> {
        if self.get_session(session_id).is_none() {
            return Err(anyhow!("DWG Viewer 会话不存在：{session_id}"));
        }

        let command_id = Uuid::new_v4().to_string();
        let command = DwgViewerCommand {
            command_id: command_id.clone(),
            session_id: session_id.to_string(),
            action: action.to_string(),
            payload,
        };
        let (tx, rx) = oneshot::channel();
        self.pending_results.lock().insert(command_id.clone(), tx);
        if let Err(error) = app.emit("dwg-viewer/command", &command) {
            self.pending_results.lock().remove(&command_id);
            return Err(error.into());
        }

        match timeout(timeout_duration, rx).await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(_)) => Err(anyhow!("DWG Viewer 命令回执通道已关闭")),
            Err(_) => {
                self.pending_results.lock().remove(&command_id);
                Err(anyhow!("DWG Viewer 命令超时"))
            }
        }
    }

    pub fn resolve_command_result(&self, result: DwgViewerCommandResult) -> Result<()> {
        let Some(waiter) = self.pending_results.lock().remove(&result.command_id) else {
            return Err(anyhow!(
                "未找到等待中的 DWG Viewer 命令：{}",
                result.command_id
            ));
        };
        waiter
            .send(result)
            .map_err(|_| anyhow!("DWG Viewer 命令结果无法回传"))
    }
}

fn normalize_file_path(path: &str) -> String {
    let candidate = PathBuf::from(path);
    let normalized = if candidate.exists() {
        candidate.canonicalize().unwrap_or(candidate)
    } else {
        lexical_normalize(&candidate)
    };
    normalized.to_string_lossy().into_owned()
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}
