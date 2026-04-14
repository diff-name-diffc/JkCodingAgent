use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::sync::Arc;

mod agent;
mod analytics;
mod app_settings;
mod config;
mod dispatcher_config;
mod dispatcher_db;
mod dispatcher_llm;
mod dispatcher_tools;
mod fs;
mod git;
mod notification;
mod pty;
mod rope;
mod session;
mod storage;
mod usage;

use agent::{AgentEvent, AgentTurn, DispatcherAgent};
use dispatcher_config::DispatcherAgentConfig;
use dispatcher_db::{
    DispatcherDb, DispatcherMessageRecord, DispatcherSessionRecord, DispatcherSettingsRecord,
};
use session::{ClaudeSessionInfo, CodexSessionInfo};

pub struct TaskManager {
    pub(crate) pty_masters: Mutex<HashMap<String, Box<dyn portable_pty::MasterPty + Send>>>,
    pub(crate) pty_writers: Mutex<HashMap<String, Box<dyn Write + Send>>>,
    pub(crate) child_handles:
        Mutex<HashMap<String, Arc<std::sync::Mutex<Box<dyn portable_pty::Child + Send + Sync>>>>>,
    pub(crate) cancelled_tasks: Mutex<HashSet<String>>,
    pub(crate) codex_sessions: Mutex<HashMap<String, CodexSessionInfo>>,
    pub(crate) claude_sessions: Mutex<HashMap<String, ClaudeSessionInfo>>,
    pub(crate) claimed_session_paths: Mutex<HashSet<String>>,
    /// Maps task_id → dispatch_id for dispatcher-spawned subprocess tracking
    pub(crate) dispatcher_subprocess_ids: Mutex<HashMap<String, String>>,
    /// Subprocess task_ids that were exited by the dispatcher via /exit (skip result injection)
    pub(crate) dispatcher_exited_subprocesses: Mutex<HashSet<String>>,
    /// Maps task_id → AtomicBool, used by session JSONL watcher to force idle emission
    pub(crate) dispatcher_force_idle_flags:
        Mutex<HashMap<String, std::sync::Arc<std::sync::atomic::AtomicBool>>>,
}

impl TaskManager {
    /// Atomically remove a task/shell from all PTY maps (masters, writers, children).
    /// Locks are acquired in a fixed order to prevent deadlocks.
    pub(crate) fn remove_pty_handles(&self, id: &str) {
        let mut masters = self.pty_masters.lock();
        let mut writers = self.pty_writers.lock();
        let mut children = self.child_handles.lock();
        masters.remove(id);
        writers.remove(id);
        children.remove(id);
    }
}

pub struct DispatcherState {
    agent: tokio::sync::Mutex<DispatcherAgent>,
    db: DispatcherDb,
}

// ── Dispatcher Tauri Commands ────────────────────────────────────────────────

#[tauri::command]
async fn dispatcher_send_message(
    state: tauri::State<'_, DispatcherState>,
    workspace_id: String,
    project_path: String,
    content: String,
    on_event: tauri::ipc::Channel<AgentEvent>,
) -> Result<AgentTurn, String> {
    // Apply DB settings override before each agent run
    if let Ok(Some(settings)) = state.db.get_settings() {
        let mut agent = state.agent.lock().await;
        agent.apply_settings(&settings);
        agent.set_auto_approve_dispatch(settings.auto_approve_dispatch);
    }

    let agent = state.agent.lock().await;
    agent
        .run(&state.db, &workspace_id, &project_path, &content, on_event)
        .await
        .map_err(to_command_error)
}

#[tauri::command]
fn dispatcher_list_messages(
    state: tauri::State<'_, DispatcherState>,
    workspace_id: String,
) -> Result<Vec<DispatcherMessageRecord>, String> {
    state
        .db
        .list_visible_messages(&workspace_id)
        .map_err(to_command_error)
}

#[tauri::command]
fn dispatcher_clear_messages(
    state: tauri::State<'_, DispatcherState>,
    workspace_id: String,
) -> Result<(), String> {
    state
        .db
        .clear_messages(&workspace_id)
        .map_err(to_command_error)
}

#[tauri::command]
fn dispatcher_get_settings(
    state: tauri::State<'_, DispatcherState>,
) -> Result<Option<DispatcherSettingsRecord>, String> {
    state.db.get_settings().map_err(to_command_error)
}

#[tauri::command]
fn dispatcher_list_sessions(
    state: tauri::State<'_, DispatcherState>,
    project_id: String,
) -> Result<Vec<DispatcherSessionRecord>, String> {
    state
        .db
        .list_sessions(&project_id)
        .map_err(to_command_error)
}

#[tauri::command]
fn dispatcher_create_session(
    state: tauri::State<'_, DispatcherState>,
    project_id: String,
    title: String,
) -> Result<DispatcherSessionRecord, String> {
    state
        .db
        .create_session(&project_id, &title)
        .map_err(to_command_error)
}

#[tauri::command]
fn dispatcher_delete_session(
    state: tauri::State<'_, DispatcherState>,
    session_id: String,
) -> Result<(), String> {
    state
        .db
        .delete_session(&session_id)
        .map_err(to_command_error)
}

#[tauri::command]
async fn dispatcher_save_settings(
    state: tauri::State<'_, DispatcherState>,
    api_base: String,
    api_key: String,
    model: String,
    auto_approve_dispatch: bool,
) -> Result<DispatcherSettingsRecord, String> {
    let record = state
        .db
        .save_settings(&api_base, &api_key, &model, auto_approve_dispatch)
        .map_err(to_command_error)?;
    // Immediately apply to the running agent
    let mut agent = state.agent.lock().await;
    agent.apply_settings(&record);
    agent.set_auto_approve_dispatch(record.auto_approve_dispatch);
    Ok(record)
}

#[tauri::command]
async fn dispatcher_fetch_models(api_base: String, api_key: String) -> Result<Vec<String>, String> {
    dispatcher_llm::fetch_models(&api_base, &api_key)
        .await
        .map_err(to_command_error)
}

#[tauri::command]
async fn dispatcher_set_auto_approve_dispatch(
    state: tauri::State<'_, DispatcherState>,
    auto_approve_dispatch: bool,
) -> Result<DispatcherSettingsRecord, String> {
    let record = state
        .db
        .set_auto_approve_dispatch(auto_approve_dispatch)
        .map_err(to_command_error)?;
    let mut agent = state.agent.lock().await;
    agent.apply_settings(&record);
    agent.set_auto_approve_dispatch(record.auto_approve_dispatch);
    Ok(record)
}

/// Claude 子任务完成后，将结果注入 Agent 并继续对话循环
#[tauri::command]
async fn dispatcher_continue_after_dispatch(
    state: tauri::State<'_, DispatcherState>,
    workspace_id: String,
    project_path: String,
    dispatch_result: String,
    on_event: tauri::ipc::Channel<AgentEvent>,
) -> Result<AgentTurn, String> {
    let agent = state.agent.lock().await;
    agent
        .continue_after_dispatch(
            &state.db,
            &workspace_id,
            &project_path,
            &dispatch_result,
            on_event,
        )
        .await
        .map_err(to_command_error)
}

/// Register a task as a dispatcher subprocess
#[tauri::command]
fn dispatcher_register_subprocess(
    task_manager: tauri::State<'_, TaskManager>,
    task_id: String,
    dispatch_id: String,
) -> Result<(), String> {
    task_manager
        .dispatcher_subprocess_ids
        .lock()
        .insert(task_id, dispatch_id);
    Ok(())
}

/// Send text input to a dispatcher subprocess PTY
#[tauri::command]
async fn dispatcher_send_to_subprocess(
    task_manager: tauri::State<'_, TaskManager>,
    task_id: String,
    text: String,
) -> Result<(), String> {
    let mut writers = task_manager.pty_writers.lock();
    if let Some(writer) = writers.get_mut(&task_id) {
        writer
            .write_all(text.as_bytes())
            .map_err(|e| e.to_string())?;
        writer.flush().map_err(|e| e.to_string())?;
        Ok(())
    } else {
        Err(format!("No active PTY writer found for task {}", task_id))
    }
}

/// Send /exit to a dispatcher subprocess and mark it as voluntarily exited
#[tauri::command]
async fn dispatcher_exit_subprocess(
    task_manager: tauri::State<'_, TaskManager>,
    task_id: String,
) -> Result<(), String> {
    // Mark as voluntarily exited so we skip result injection on task-status
    task_manager
        .dispatcher_exited_subprocesses
        .lock()
        .insert(task_id.clone());

    let mut writers = task_manager.pty_writers.lock();
    if let Some(writer) = writers.get_mut(&task_id) {
        writer.write_all(b"/exit\r").map_err(|e| e.to_string())?;
        writer.flush().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Check if a subprocess was voluntarily exited by the dispatcher
#[tauri::command]
fn dispatcher_is_subprocess_exited(
    task_manager: tauri::State<'_, TaskManager>,
    task_id: String,
) -> bool {
    task_manager
        .dispatcher_exited_subprocesses
        .lock()
        .contains(&task_id)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Initialize Dispatcher Agent
    let dispatcher_config =
        DispatcherAgentConfig::load().expect("failed to load Dispatcher Agent config");
    let dispatcher_db = DispatcherDb::new(dispatcher_config.db_path.clone())
        .expect("failed to open Dispatcher sqlite database");
    let dispatcher_agent = DispatcherAgent::new(dispatcher_config);

    // Apply DB settings on startup if they exist
    if let Ok(Some(settings)) = dispatcher_db.get_settings() {
        dispatcher_agent.apply_settings(&settings);
    }

    let dispatcher_agent = tokio::sync::Mutex::new(dispatcher_agent);

    tauri::Builder::default()
        .setup(|_app| {
            // 后台预热 login shell 环境，避免第一次启动任务时阻塞
            std::thread::spawn(|| {
                crate::app_settings::get_login_shell_path();
            });
            Ok(())
        })
        .manage(TaskManager {
            pty_masters: Mutex::new(HashMap::new()),
            pty_writers: Mutex::new(HashMap::new()),
            child_handles: Mutex::new(HashMap::new()),
            cancelled_tasks: Mutex::new(HashSet::new()),
            codex_sessions: Default::default(),
            claude_sessions: Default::default(),
            claimed_session_paths: Default::default(),
            dispatcher_subprocess_ids: Default::default(),
            dispatcher_exited_subprocesses: Default::default(),
            dispatcher_force_idle_flags: Default::default(),
        })
        .manage(DispatcherState {
            agent: dispatcher_agent,
            db: dispatcher_db,
        })
        .manage(rope::RopeManager::new())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            // --- Existing commands ---
            pty::run_task,
            pty::resume_task,
            pty::cancel_task,
            pty::send_input,
            pty::resize_pty,
            pty::open_shell,
            pty::kill_shell,
            fs::read_dir_entries,
            fs::read_file_content,
            fs::read_image_preview,
            fs::write_file_content,
            fs::list_project_files,
            fs::get_file_meta,
            fs::read_file_chunk,
            // --- Rope (unified file editing) ---
            rope::rope_open,
            rope::rope_read_lines,
            rope::rope_edit,
            rope::rope_replace_line,
            rope::rope_save,
            rope::rope_is_dirty,
            rope::rope_close,
            rope::rope_undo,
            rope::rope_redo,
            git::generate_commit_message,
            git::git_status,
            git::git_list_branches,
            git::git_create_branch,
            git::git_checkout_branch,
            git::git_log,
            git::git_commit_detail,
            git::git_show_diff,
            git::git_show_file_diff,
            git::git_file_diff,
            git::git_stage,
            git::git_unstage,
            git::git_stage_all,
            git::git_unstage_all,
            git::git_commit,
            git::git_push,
            git::git_pull,
            git::git_remote_counts,
            analytics::read_session_metrics,
            analytics::get_weekly_analytics,
            session::read_session_messages,
            config::init_project_config,
            config::read_project_config,
            config::write_project_config,
            config::read_agent_config_file,
            config::write_agent_config_file,
            storage::load_projects,
            storage::save_projects,
            storage::load_project_tasks,
            storage::save_project_tasks,
            app_settings::load_app_settings,
            app_settings::save_app_settings,
            app_settings::detect_agent_paths,
            app_settings::detect_agent_versions,
            app_settings::detect_agent_versions_for_settings,
            notification::get_notifications,
            notification::mark_notification_read,
            notification::mark_all_notifications_read,
            usage::read_usage_snapshot,
            // --- Dispatcher Agent commands ---
            dispatcher_send_message,
            dispatcher_list_messages,
            dispatcher_clear_messages,
            dispatcher_get_settings,
            dispatcher_list_sessions,
            dispatcher_create_session,
            dispatcher_delete_session,
            dispatcher_save_settings,
            dispatcher_set_auto_approve_dispatch,
            dispatcher_fetch_models,
            dispatcher_continue_after_dispatch,
            dispatcher_register_subprocess,
            dispatcher_send_to_subprocess,
            dispatcher_exit_subprocess,
            dispatcher_is_subprocess_exited,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

fn to_command_error(error: anyhow::Error) -> String {
    error.to_string()
}
