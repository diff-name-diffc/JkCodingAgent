use anyhow::Result;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::watch;
use tokio::time::{sleep, Duration};

use super::config::DispatcherAgentConfig;
use super::db::{
    DispatcherDb, DispatcherMessageRecord, DispatcherSessionRecord, DispatcherSettingsRecord,
    DispatcherToolArtifactRecord,
};
use super::llm;
use super::runtime::{AgentEvent, AgentTurn, DispatchFeedbackState, DispatcherAgent};
use crate::project::mcp::ProjectMcpRegistry;
use crate::shared::TaskManager;

pub struct DispatcherState {
    agent: tokio::sync::Mutex<DispatcherAgent>,
    db: DispatcherDb,
    active_runs: Mutex<HashMap<String, ActiveRunEntry>>,
    next_run_generation: AtomicU64,
}

#[derive(Clone)]
struct ActiveRunEntry {
    generation: u64,
    stop_tx: watch::Sender<bool>,
}

struct ActiveRunHandle {
    generation: u64,
    cancel_rx: watch::Receiver<bool>,
}

impl DispatcherState {
    pub fn new(project_mcp_registry: ProjectMcpRegistry) -> Result<Self> {
        let config = DispatcherAgentConfig::load()?;
        let db = DispatcherDb::new(config.db_path.clone())?;
        let mut agent = DispatcherAgent::new(config, project_mcp_registry);

        if let Ok(Some(settings)) = db.get_settings() {
            agent.apply_settings(&settings);
            agent.set_auto_approve_dispatch(settings.auto_approve_dispatch);
            agent.set_context_debug(settings.context_debug);
        }

        Ok(Self {
            agent: tokio::sync::Mutex::new(agent),
            db,
            active_runs: Mutex::new(HashMap::new()),
            next_run_generation: AtomicU64::new(1),
        })
    }

    fn begin_run(&self, workspace_id: &str) -> ActiveRunHandle {
        let generation = self.next_run_generation.fetch_add(1, Ordering::Relaxed);
        let (stop_tx, cancel_rx) = watch::channel(false);
        self.active_runs.lock().insert(
            workspace_id.to_string(),
            ActiveRunEntry {
                generation,
                stop_tx,
            },
        );
        ActiveRunHandle {
            generation,
            cancel_rx,
        }
    }

    fn finish_run(&self, workspace_id: &str, generation: u64) {
        let mut active_runs = self.active_runs.lock();
        let should_remove = active_runs
            .get(workspace_id)
            .is_some_and(|entry| entry.generation == generation);
        if should_remove {
            active_runs.remove(workspace_id);
        }
    }

    fn stop_run(&self, workspace_id: &str) -> bool {
        let tx = self
            .active_runs
            .lock()
            .get(workspace_id)
            .map(|entry| entry.stop_tx.clone());

        tx.is_some_and(|sender| sender.send(true).is_ok())
    }
}

fn has_live_subprocess(task_manager: &TaskManager, task_id: &str) -> bool {
    task_manager.child_handles.lock().contains_key(task_id)
}

fn is_codex_subprocess(task_manager: &TaskManager, task_id: &str) -> bool {
    task_manager.codex_sessions.lock().contains_key(task_id)
}

fn write_to_subprocess(
    task_manager: &TaskManager,
    task_id: &str,
    data: &[u8],
) -> Result<(), String> {
    if !task_manager.pty_writers.lock().contains_key(task_id) {
        return Err(format!("No active PTY writer found for task {}", task_id));
    }
    task_manager.write_to_pty(task_id, data, true)
}

async fn submit_subprocess_line(
    task_manager: &TaskManager,
    task_id: &str,
    text: &str,
    with_lf_fallback: bool,
) -> Result<(), String> {
    let line = text.trim_end_matches(['\r', '\n']);
    if !line.is_empty() {
        write_to_subprocess(task_manager, task_id, line.as_bytes())?;
    }

    sleep(Duration::from_millis(120)).await;
    if has_live_subprocess(task_manager, task_id) {
        write_to_subprocess(task_manager, task_id, b"\r")?;
    }

    if with_lf_fallback {
        sleep(Duration::from_millis(120)).await;
        if has_live_subprocess(task_manager, task_id) {
            let _ = write_to_subprocess(task_manager, task_id, b"\n");
        }
    }

    Ok(())
}

#[tauri::command]
pub async fn dispatcher_send_message(
    state: tauri::State<'_, DispatcherState>,
    workspace_id: String,
    project_path: String,
    content: String,
    on_event: tauri::ipc::Channel<AgentEvent>,
) -> Result<AgentTurn, String> {
    if let Ok(Some(settings)) = state.db.get_settings() {
        let mut agent = state.agent.lock().await;
        agent.apply_settings(&settings);
        agent.set_auto_approve_dispatch(settings.auto_approve_dispatch);
        agent.set_context_debug(settings.context_debug);
    }

    let run_handle = state.begin_run(&workspace_id);
    let agent = state.agent.lock().await;
    let result = agent
        .run(
            &state.db,
            &workspace_id,
            &project_path,
            &content,
            on_event,
            run_handle.cancel_rx,
        )
        .await
        .map_err(|error| error.to_string());
    drop(agent);
    state.finish_run(&workspace_id, run_handle.generation);
    result
}

#[tauri::command]
pub fn dispatcher_list_messages(
    state: tauri::State<'_, DispatcherState>,
    workspace_id: String,
) -> Result<Vec<DispatcherMessageRecord>, String> {
    state
        .db
        .list_visible_messages(&workspace_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn dispatcher_clear_messages(
    state: tauri::State<'_, DispatcherState>,
    workspace_id: String,
) -> Result<(), String> {
    state
        .db
        .clear_messages(&workspace_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn dispatcher_get_tool_artifact(
    state: tauri::State<'_, DispatcherState>,
    workspace_id: String,
    artifact_id: String,
) -> Result<DispatcherToolArtifactRecord, String> {
    state
        .db
        .get_tool_artifact(&workspace_id, &artifact_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn dispatcher_get_settings(
    state: tauri::State<'_, DispatcherState>,
) -> Result<Option<DispatcherSettingsRecord>, String> {
    state.db.get_settings().map_err(|error| error.to_string())
}

#[tauri::command]
pub fn dispatcher_list_sessions(
    state: tauri::State<'_, DispatcherState>,
    project_id: String,
) -> Result<Vec<DispatcherSessionRecord>, String> {
    state
        .db
        .list_sessions(&project_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn dispatcher_create_session(
    state: tauri::State<'_, DispatcherState>,
    project_id: String,
    title: String,
) -> Result<DispatcherSessionRecord, String> {
    state
        .db
        .create_session(&project_id, &title)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn dispatcher_delete_session(
    state: tauri::State<'_, DispatcherState>,
    session_id: String,
) -> Result<(), String> {
    state
        .db
        .delete_session(&session_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn dispatcher_save_settings(
    state: tauri::State<'_, DispatcherState>,
    api_base: String,
    api_key: String,
    model: String,
    auto_approve_dispatch: bool,
    context_debug: bool,
) -> Result<DispatcherSettingsRecord, String> {
    let record = state
        .db
        .save_settings(
            &api_base,
            &api_key,
            &model,
            auto_approve_dispatch,
            context_debug,
        )
        .map_err(|error| error.to_string())?;

    let mut agent = state.agent.lock().await;
    agent.apply_settings(&record);
    agent.set_auto_approve_dispatch(record.auto_approve_dispatch);
    agent.set_context_debug(record.context_debug);
    Ok(record)
}

#[tauri::command]
pub async fn dispatcher_fetch_models(
    api_base: String,
    api_key: String,
) -> Result<Vec<String>, String> {
    llm::fetch_models(&api_base, &api_key)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn dispatcher_set_auto_approve_dispatch(
    state: tauri::State<'_, DispatcherState>,
    auto_approve_dispatch: bool,
) -> Result<DispatcherSettingsRecord, String> {
    let record = state
        .db
        .set_auto_approve_dispatch(auto_approve_dispatch)
        .map_err(|error| error.to_string())?;
    let mut agent = state.agent.lock().await;
    agent.apply_settings(&record);
    agent.set_auto_approve_dispatch(record.auto_approve_dispatch);
    agent.set_context_debug(record.context_debug);
    Ok(record)
}

#[tauri::command]
pub async fn dispatcher_continue_after_dispatch(
    state: tauri::State<'_, DispatcherState>,
    workspace_id: String,
    project_path: String,
    dispatch_result: String,
    dispatch_state: String,
    on_event: tauri::ipc::Channel<AgentEvent>,
) -> Result<AgentTurn, String> {
    let agent = state.agent.lock().await;
    let run_handle = state.begin_run(&workspace_id);
    let result = agent
        .continue_after_dispatch(
            &state.db,
            &workspace_id,
            &project_path,
            &dispatch_result,
            DispatchFeedbackState::from_wire(&dispatch_state),
            on_event,
            run_handle.cancel_rx,
        )
        .await
        .map_err(|error| error.to_string());
    drop(agent);
    state.finish_run(&workspace_id, run_handle.generation);
    result
}

#[tauri::command]
pub fn dispatcher_stop_run(state: tauri::State<'_, DispatcherState>, workspace_id: String) -> bool {
    state.stop_run(&workspace_id)
}

#[tauri::command]
pub async fn dispatcher_register_subprocess(
    task_manager: tauri::State<'_, TaskManager>,
    state: tauri::State<'_, DispatcherState>,
    workspace_id: String,
    task_id: String,
    dispatch_id: String,
    agent: String,
    description: String,
) -> Result<(), String> {
    task_manager
        .dispatcher_subprocess_ids
        .lock()
        .insert(task_id.clone(), dispatch_id.clone());
    let agent_runtime = state.agent.lock().await;
    agent_runtime.register_subprocess(&workspace_id, &task_id, &dispatch_id, &agent, &description);
    Ok(())
}

#[tauri::command]
pub async fn dispatcher_mark_subprocess_round_completed(
    state: tauri::State<'_, DispatcherState>,
    task_id: String,
) -> Result<(), String> {
    let agent = state.agent.lock().await;
    agent.mark_subprocess_round_completed(&task_id);
    Ok(())
}

#[tauri::command]
pub async fn dispatcher_mark_subprocess_running(
    state: tauri::State<'_, DispatcherState>,
    task_id: String,
) -> Result<(), String> {
    let agent = state.agent.lock().await;
    agent.mark_subprocess_running(&task_id);
    Ok(())
}

#[tauri::command]
pub async fn dispatcher_mark_subprocess_stopped(
    state: tauri::State<'_, DispatcherState>,
    task_id: String,
) -> Result<(), String> {
    let agent = state.agent.lock().await;
    agent.mark_subprocess_stopped(&task_id);
    Ok(())
}

#[tauri::command]
pub async fn dispatcher_mark_subprocess_finished(
    state: tauri::State<'_, DispatcherState>,
    task_id: String,
) -> Result<(), String> {
    let agent = state.agent.lock().await;
    agent.mark_subprocess_finished(&task_id);
    Ok(())
}

#[tauri::command]
pub async fn dispatcher_send_to_subprocess(
    task_manager: tauri::State<'_, TaskManager>,
    task_id: String,
    text: String,
) -> Result<(), String> {
    let is_codex = is_codex_subprocess(&task_manager, &task_id);
    submit_subprocess_line(&task_manager, &task_id, &text, is_codex).await
}

#[tauri::command]
pub async fn dispatcher_exit_subprocess(
    task_manager: tauri::State<'_, TaskManager>,
    task_id: String,
) -> Result<(), String> {
    let is_codex = is_codex_subprocess(&task_manager, &task_id);

    if is_codex {
        submit_subprocess_line(&task_manager, &task_id, "/exit", true).await?;
    } else {
        submit_subprocess_line(&task_manager, &task_id, "/exit", false).await?;
    }

    task_manager
        .dispatcher_exited_subprocesses
        .lock()
        .insert(task_id);

    Ok(())
}

#[tauri::command]
pub fn dispatcher_is_subprocess_exited(
    task_manager: tauri::State<'_, TaskManager>,
    task_id: String,
) -> bool {
    task_manager
        .dispatcher_exited_subprocesses
        .lock()
        .contains(&task_id)
}
