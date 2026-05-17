use anyhow::Result;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::watch;
use tokio::time::{sleep, Duration};

use tauri::{AppHandle, Emitter, Manager};

use super::config::{DispatcherAgentConfig, DEFAULT_SUMMARY_MODEL};
use super::db::{
    DispatcherDb, DispatcherMessageRecord, DispatcherMode, DispatcherSessionKind,
    DispatcherSessionRecord, DispatcherSessionRuntimeState, DispatcherSessionTokenUsageRecord,
    DispatcherSessionTokenUsageSource, DispatcherSettingsRecord, DispatcherToolArtifactRecord,
};
use super::llm;
use super::llm::OpenAiCompatProvider;
use super::runtime::{
    AgentEvent, AgentTurn, DispatchFeedbackState, DispatcherAgent, DispatcherSubprocessRegistry,
};
use super::summary::{fallback_session_title, summarize_session_title, SessionTitleMessage};
use super::voice::{resolve_dashscope_websocket_url, VoiceAsrConfig, VoiceAsrManager};
use crate::browser::BrowserManager;
use crate::project::mcp::ProjectMcpRegistry;
use crate::shared::TaskManager;

const SESSION_TITLE_RECENT_DIALOGUES: usize = 3;

pub struct DispatcherState {
    config: DispatcherAgentConfig,
    project_mcp_registry: ProjectMcpRegistry,
    subprocesses: Arc<DispatcherSubprocessRegistry>,
    db: DispatcherDb,
    active_runs: Mutex<HashMap<String, ActiveRunEntry>>,
    next_run_generation: AtomicU64,
    title_generations: Mutex<HashMap<String, u64>>,
    next_title_generation: AtomicU64,
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
        let subprocesses = Arc::new(DispatcherSubprocessRegistry::default());

        Ok(Self {
            config,
            project_mcp_registry,
            subprocesses,
            db,
            active_runs: Mutex::new(HashMap::new()),
            next_run_generation: AtomicU64::new(1),
            title_generations: Mutex::new(HashMap::new()),
            next_title_generation: AtomicU64::new(1),
        })
    }

    fn build_run_agent(&self) -> DispatcherAgent {
        let mut agent = DispatcherAgent::new(
            self.config.clone(),
            self.project_mcp_registry.clone(),
            Arc::clone(&self.subprocesses),
        );

        if let Ok(Some(settings)) = self.db.get_settings() {
            agent.apply_settings(&settings);
            agent.set_auto_approve_dispatch(settings.auto_approve_dispatch);
            agent.set_context_debug(settings.context_debug);
        }

        agent
    }

    fn begin_run(&self, workspace_id: &str) -> Result<ActiveRunHandle, String> {
        let mut active_runs = self.active_runs.lock();
        if active_runs.contains_key(workspace_id) {
            return Err(format!("会话 {} 已在运行中，请等待当前任务完成", workspace_id));
        }
        let generation = self.next_run_generation.fetch_add(1, Ordering::Relaxed);
        let (stop_tx, cancel_rx) = watch::channel(false);
        active_runs.insert(
            workspace_id.to_string(),
            ActiveRunEntry {
                generation,
                stop_tx,
            },
        );
        Ok(ActiveRunHandle {
            generation,
            cancel_rx,
        })
    }

    pub(crate) fn register_subprocess(
        &self,
        workspace_id: &str,
        task_id: &str,
        dispatch_id: &str,
        agent: &str,
        description: &str,
    ) -> Arc<std::sync::atomic::AtomicBool> {
        self.subprocesses
            .register(workspace_id, task_id, dispatch_id, agent, description)
    }

    pub(crate) fn mark_subprocess_round_completed(&self, task_id: &str) {
        self.subprocesses.mark_round_completed(task_id);
    }

    pub(crate) fn mark_subprocess_running(&self, task_id: &str) {
        self.subprocesses.mark_running(task_id);
    }

    pub(crate) fn mark_subprocess_stopped(&self, task_id: &str) {
        self.subprocesses.mark_stopped(task_id);
    }

    pub(crate) fn mark_subprocess_finished(&self, task_id: &str) {
        self.subprocesses.mark_finished(task_id);
    }

    pub(crate) fn mark_subprocess_exit_requested(&self, task_id: &str) {
        self.subprocesses.mark_exit_requested(task_id);
    }

    pub(crate) fn force_subprocess_idle(&self, task_id: &str) {
        self.subprocesses.force_idle(task_id);
    }

    pub(crate) fn is_subprocess_exit_requested(&self, task_id: &str) -> bool {
        self.subprocesses.is_exit_requested(task_id)
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

    fn begin_title_generation(&self, workspace_id: &str) -> u64 {
        let generation = self.next_title_generation.fetch_add(1, Ordering::Relaxed);
        self.title_generations
            .lock()
            .insert(workspace_id.to_string(), generation);
        generation
    }

    fn finish_latest_title_generation(&self, workspace_id: &str, generation: u64) -> bool {
        let mut title_generations = self.title_generations.lock();
        if title_generations
            .get(workspace_id)
            .is_some_and(|current| *current == generation)
        {
            title_generations.remove(workspace_id);
            return true;
        }
        false
    }
}

fn has_live_subprocess(task_manager: &TaskManager, task_id: &str) -> bool {
    task_manager.child_handles.lock().contains_key(task_id)
}

fn resolve_voice_asr_config(state: &DispatcherState) -> Result<VoiceAsrConfig, String> {
    let saved = state.db.get_settings().ok().flatten();
    let loaded = DispatcherAgentConfig::load().ok();

    let api_key = saved
        .as_ref()
        .map(|settings| settings.asr_api_key.trim())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            std::env::var("DASHSCOPE_API_KEY").ok().and_then(|value| {
                let trimmed = value.trim().to_string();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed)
                }
            })
        })
        .or_else(|| {
            saved
                .as_ref()
                .filter(|settings| settings.api_base.contains("dashscope"))
                .map(|settings| settings.api_key.trim())
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        })
        .or_else(|| {
            loaded.as_ref().and_then(|config| {
                if config.api_base.contains("dashscope") && !config.api_key.trim().is_empty() {
                    Some(config.api_key.trim().to_string())
                } else {
                    None
                }
            })
        })
        .ok_or_else(|| {
            "未检测到 ASR API Key，请先在 Aha 智能体设置中填写 ASR API Key，或设置 DASHSCOPE_API_KEY。"
                .to_string()
        })?;

    let saved_websocket_url = saved
        .as_ref()
        .map(|settings| settings.asr_websocket_url.trim())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);

    let api_base = saved
        .as_ref()
        .map(|settings| settings.api_base.trim())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            loaded
                .as_ref()
                .map(|config| config.api_base.trim())
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| "https://dashscope.aliyuncs.com/compatible-mode/v1".to_string());

    Ok(VoiceAsrConfig {
        api_key,
        websocket_url: saved_websocket_url
            .unwrap_or_else(|| resolve_dashscope_websocket_url(&api_base)),
    })
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

fn spawn_session_title_update(
    state: &DispatcherState,
    app: &AppHandle,
    workspace_id: &str,
    fallback_content: &str,
    generation: u64,
) {
    let app = app.clone();
    let db = state.db.clone();
    let workspace_id = workspace_id.to_string();
    let fallback_content = fallback_content.to_string();

    tokio::spawn(async move {
        let title =
            generate_session_title(db.clone(), workspace_id.clone(), fallback_content).await;
        let state = app.state::<DispatcherState>();
        if !state.finish_latest_title_generation(&workspace_id, generation) {
            return;
        }

        let update_workspace_id = workspace_id.clone();
        let update_title = title.clone();
        let update_result = tokio::task::spawn_blocking(move || {
            db.update_session_title(&update_workspace_id, &update_title)
        })
        .await;

        match update_result {
            Ok(Ok(Some(session))) => {
                let _ = app.emit("dispatcher-session-updated", session);
            }
            Ok(Ok(None)) => {}
            Ok(Err(error)) => {
                eprintln!(
                    "failed to update dispatcher session title for {}: {}",
                    workspace_id, error
                );
            }
            Err(error) => {
                eprintln!(
                    "dispatcher session title update task failed for {}: {}",
                    workspace_id, error
                );
            }
        }
    });
}

async fn generate_session_title(
    db: DispatcherDb,
    workspace_id: String,
    fallback_content: String,
) -> String {
    let title_messages_db = db.clone();
    let title_messages_workspace_id = workspace_id.clone();
    let title_messages = tokio::task::spawn_blocking(move || {
        title_messages_db.list_recent_visible_dialogue_messages(
            &title_messages_workspace_id,
            SESSION_TITLE_RECENT_DIALOGUES,
        )
    })
    .await;

    let title_messages = match title_messages {
        Ok(Ok(messages)) => messages,
        Ok(Err(error)) => {
            eprintln!("failed to load dispatcher title dialogue context: {error}");
            Vec::new()
        }
        Err(error) => {
            eprintln!("dispatcher title dialogue context task failed: {error}");
            Vec::new()
        }
    };
    let fallback_source = title_messages
        .iter()
        .rev()
        .find(|message| message.role == "user" && !message.content.trim().is_empty())
        .map(|message| message.content.clone())
        .unwrap_or_else(|| fallback_content.clone());
    let fallback = fallback_session_title(&fallback_source);
    let title_messages = title_messages
        .into_iter()
        .map(|message| SessionTitleMessage {
            role: message.role,
            content: message.content,
        })
        .collect::<Vec<_>>();
    let provider_db = db.clone();
    let provider_config =
        tokio::task::spawn_blocking(move || resolve_title_provider(&provider_db)).await;

    let (provider, summary_model) = match provider_config {
        Ok(Ok(config)) => config,
        Ok(Err(error)) => {
            eprintln!("failed to load dispatcher title summary config: {error}");
            return fallback;
        }
        Err(error) => {
            eprintln!("dispatcher title summary config task failed: {error}");
            return fallback;
        }
    };

    if !provider.is_configured() {
        return fallback;
    }

    let usage_db = db.clone();
    let usage_workspace_id = workspace_id.clone();
    let usage_summary_model = summary_model.clone();
    match summarize_session_title(
        &provider,
        &summary_model,
        &title_messages,
        &fallback_source,
        move |usage| {
            if let Err(error) = usage_db.upsert_session_token_usage(
                &usage_workspace_id,
                &usage_summary_model,
                DispatcherSessionTokenUsageSource::Summary,
                usage,
            ) {
                eprintln!(
                    "failed to persist dispatcher title token usage for workspace {} and model {}: {}",
                    usage_workspace_id, usage_summary_model, error
                );
            }
        }
    )
    .await
    {
        Ok(title) => title,
        Err(error) => {
            eprintln!(
                "failed to summarize dispatcher session title with {}: {}",
                summary_model,
                error.message()
            );
            fallback
        }
    }
}

fn resolve_title_provider(db: &DispatcherDb) -> Result<(OpenAiCompatProvider, String)> {
    let config = DispatcherAgentConfig::load()?;
    let settings = db.get_settings()?;

    let api_key = settings
        .as_ref()
        .map(|item| item.api_key.trim())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| config.api_key.clone());
    let api_base = settings
        .as_ref()
        .map(|item| item.api_base.trim())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| config.api_base.clone());
    let summary_model = settings
        .as_ref()
        .map(|item| item.summary_model.trim())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| config.summary_model.clone());
    let summary_model = normalize_summary_model_name(&summary_model);

    Ok((
        OpenAiCompatProvider::new(
            api_key,
            api_base,
            summary_model.clone(),
            96,
            config.temperature,
        ),
        summary_model,
    ))
}

fn normalize_summary_model_name(model: &str) -> String {
    let trimmed = model.trim();
    if trimmed.is_empty() {
        DEFAULT_SUMMARY_MODEL.to_string()
    } else {
        trimmed.to_string()
    }
}

#[tauri::command]
pub async fn dispatcher_send_message(
    state: tauri::State<'_, DispatcherState>,
    app: AppHandle,
    workspace_id: String,
    project_path: String,
    content: String,
    segments_json: Option<String>,
    mode: Option<String>,
    enable_thinking: Option<bool>,
    on_event: tauri::ipc::Channel<AgentEvent>,
) -> Result<AgentTurn, String> {
    let title_generation = state.begin_title_generation(&workspace_id);
    let mode = DispatcherMode::from_wire(mode.as_deref().unwrap_or("default"))
        .map_err(|error| error.to_string())?;
    state
        .db
        .set_session_mode(&workspace_id, mode)
        .map_err(|error| error.to_string())?;
    if mode == DispatcherMode::Default {
        state
            .db
            .set_plan_interaction(&workspace_id, None)
            .map_err(|error| error.to_string())?;
    }

    let run_handle = state.begin_run(&workspace_id).map_err(|e| e.to_string())?;
    let agent = state.build_run_agent().with_app_handle(app.clone());
    let result = agent
        .run(
            &state.db,
            &workspace_id,
            &project_path,
            &content,
            segments_json,
            enable_thinking.unwrap_or(false),
            on_event,
            run_handle.cancel_rx,
        )
        .await
        .map_err(|error| error.to_string());
    state.finish_run(&workspace_id, run_handle.generation);
    spawn_session_title_update(&state, &app, &workspace_id, &content, title_generation);
    result
}

#[tauri::command]
pub async fn dispatcher_send_plain_chat_message(
    state: tauri::State<'_, DispatcherState>,
    app: AppHandle,
    workspace_id: String,
    content: String,
    segments_json: Option<String>,
    enable_thinking: Option<bool>,
    on_event: tauri::ipc::Channel<AgentEvent>,
) -> Result<AgentTurn, String> {
    let title_generation = state.begin_title_generation(&workspace_id);
    state
        .db
        .set_session_mode(&workspace_id, DispatcherMode::Default)
        .map_err(|error| error.to_string())?;
    state
        .db
        .set_plan_interaction(&workspace_id, None)
        .map_err(|error| error.to_string())?;

    let run_handle = state.begin_run(&workspace_id).map_err(|e| e.to_string())?;
    let agent = state.build_run_agent().with_app_handle(app.clone());
    let result = agent
        .run_plain_chat(
            &state.db,
            &workspace_id,
            &content,
            segments_json,
            enable_thinking.unwrap_or(false),
            on_event,
            run_handle.cancel_rx,
        )
        .await
        .map_err(|error| error.to_string());
    state.finish_run(&workspace_id, run_handle.generation);
    spawn_session_title_update(&state, &app, &workspace_id, &content, title_generation);
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
pub fn dispatcher_get_session_token_usage(
    state: tauri::State<'_, DispatcherState>,
    workspace_id: String,
) -> Result<Vec<DispatcherSessionTokenUsageRecord>, String> {
    state
        .db
        .list_session_token_usage(&workspace_id)
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
pub fn dispatcher_clear_message_context(
    state: tauri::State<'_, DispatcherState>,
    workspace_id: String,
) -> Result<(), String> {
    state
        .db
        .clear_context_messages(&workspace_id)
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
    kind: Option<String>,
) -> Result<Vec<DispatcherSessionRecord>, String> {
    let kind = DispatcherSessionKind::from_wire(kind.as_deref().unwrap_or("project"))
        .map_err(|error| error.to_string())?;
    state
        .db
        .list_sessions(&project_id, kind)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn dispatcher_create_session(
    state: tauri::State<'_, DispatcherState>,
    app: AppHandle,
    project_id: String,
    title: String,
    kind: Option<String>,
    mode: Option<String>,
    active_plan_path: Option<String>,
) -> Result<DispatcherSessionRecord, String> {
    let kind = DispatcherSessionKind::from_wire(kind.as_deref().unwrap_or("project"))
        .map_err(|error| error.to_string())?;
    let mode = DispatcherMode::from_wire(mode.as_deref().unwrap_or("default"))
        .map_err(|error| error.to_string())?;
    let session = state
        .db
        .create_session(&project_id, &title, kind, mode, active_plan_path.as_deref())
        .map_err(|error| error.to_string())?;
    let _ = app.emit("dispatcher-session-updated", session.clone());
    Ok(session)
}

#[tauri::command]
pub fn dispatcher_get_session_runtime_state(
    state: tauri::State<'_, DispatcherState>,
    session_id: String,
) -> Result<DispatcherSessionRuntimeState, String> {
    state
        .db
        .get_session_runtime_state(&session_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn dispatcher_set_session_mode(
    state: tauri::State<'_, DispatcherState>,
    session_id: String,
    mode: String,
) -> Result<DispatcherSessionRuntimeState, String> {
    let mode = DispatcherMode::from_wire(&mode).map_err(|error| error.to_string())?;
    state
        .db
        .set_session_mode(&session_id, mode)
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
    summary_model: String,
    vision_model: String,
    asr_api_key: String,
    asr_websocket_url: String,
    auto_approve_dispatch: bool,
    context_debug: bool,
    image_model_url: String,
    image_model_api_key: String,
    image_model: String,
    image_edit_model: String,
) -> Result<DispatcherSettingsRecord, String> {
    let record = state
        .db
        .save_settings(
            &api_base,
            &api_key,
            &model,
            &summary_model,
            &vision_model,
            &asr_api_key,
            &asr_websocket_url,
            auto_approve_dispatch,
            context_debug,
            &image_model_url,
            &image_model_api_key,
            &image_model,
            &image_edit_model,
        )
        .map_err(|error| error.to_string())?;

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
    Ok(record)
}

#[tauri::command]
pub async fn dispatcher_continue_after_dispatch(
    state: tauri::State<'_, DispatcherState>,
    app: AppHandle,
    workspace_id: String,
    project_path: String,
    dispatch_result: String,
    dispatch_state: String,
    dispatch_id: Option<String>,
    enable_thinking: Option<bool>,
    on_event: tauri::ipc::Channel<AgentEvent>,
) -> Result<AgentTurn, String> {
    let run_handle = state.begin_run(&workspace_id)?;
    let agent = state.build_run_agent().with_app_handle(app);
    let result = agent
        .continue_after_dispatch(
            &state.db,
            &workspace_id,
            &project_path,
            &dispatch_result,
            DispatchFeedbackState::from_wire(&dispatch_state),
            dispatch_id.as_deref(),
            enable_thinking.unwrap_or(false),
            on_event,
            run_handle.cancel_rx,
        )
        .await
        .map_err(|error| error.to_string());
    state.finish_run(&workspace_id, run_handle.generation);
    result
}

#[tauri::command]
pub fn dispatcher_attach_checklist_subprocess(
    state: tauri::State<'_, DispatcherState>,
    session_id: String,
    dispatch_id: String,
    task_id: String,
) -> Result<DispatcherSessionRuntimeState, String> {
    state
        .db
        .attach_checklist_subprocess(&session_id, &dispatch_id, &task_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn dispatcher_clear_checklist_dispatch(
    state: tauri::State<'_, DispatcherState>,
    session_id: String,
    dispatch_id: String,
) -> Result<DispatcherSessionRuntimeState, String> {
    state
        .db
        .clear_checklist_dispatch(&session_id, &dispatch_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn dispatcher_stop_run(
    state: tauri::State<'_, DispatcherState>,
    browser_manager: tauri::State<'_, BrowserManager>,
    workspace_id: String,
) -> Result<bool, String> {
    let stopped = state.stop_run(&workspace_id);
    let _ = browser_manager.stop(&workspace_id).await;
    Ok(stopped)
}

#[tauri::command]
pub fn dispatcher_start_voice_input(
    state: tauri::State<'_, DispatcherState>,
    voice_state: tauri::State<'_, VoiceAsrManager>,
    app: AppHandle,
    workspace_id: String,
) -> Result<(), String> {
    let config = resolve_voice_asr_config(&state)?;
    voice_state.start_session(app, workspace_id, config)
}

#[tauri::command]
pub fn dispatcher_append_voice_audio(
    voice_state: tauri::State<'_, VoiceAsrManager>,
    workspace_id: String,
    audio_base64: String,
) -> Result<(), String> {
    voice_state.append_audio(&workspace_id, audio_base64)
}

#[tauri::command]
pub fn dispatcher_finish_voice_input(
    voice_state: tauri::State<'_, VoiceAsrManager>,
    workspace_id: String,
) -> Result<(), String> {
    voice_state.finish_session(&workspace_id)
}

#[tauri::command]
pub fn dispatcher_cancel_voice_input(
    voice_state: tauri::State<'_, VoiceAsrManager>,
    workspace_id: String,
) {
    voice_state.cancel_session(&workspace_id);
}

#[tauri::command]
pub async fn dispatcher_mark_subprocess_round_completed(
    state: tauri::State<'_, DispatcherState>,
    task_id: String,
) -> Result<(), String> {
    state.mark_subprocess_round_completed(&task_id);
    Ok(())
}

#[tauri::command]
pub async fn dispatcher_mark_subprocess_running(
    state: tauri::State<'_, DispatcherState>,
    task_id: String,
) -> Result<(), String> {
    state.mark_subprocess_running(&task_id);
    Ok(())
}

#[tauri::command]
pub async fn dispatcher_mark_subprocess_stopped(
    state: tauri::State<'_, DispatcherState>,
    task_id: String,
) -> Result<(), String> {
    state.mark_subprocess_stopped(&task_id);
    Ok(())
}

#[tauri::command]
pub async fn dispatcher_mark_subprocess_finished(
    state: tauri::State<'_, DispatcherState>,
    task_id: String,
) -> Result<(), String> {
    state.mark_subprocess_finished(&task_id);
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
    state: tauri::State<'_, DispatcherState>,
    task_id: String,
) -> Result<(), String> {
    let is_codex = is_codex_subprocess(&task_manager, &task_id);

    if is_codex {
        submit_subprocess_line(&task_manager, &task_id, "/exit", true).await?;
    } else {
        submit_subprocess_line(&task_manager, &task_id, "/exit", false).await?;
    }

    state.mark_subprocess_exit_requested(&task_id);

    Ok(())
}

#[tauri::command]
pub fn dispatcher_is_subprocess_exited(
    state: tauri::State<'_, DispatcherState>,
    task_id: String,
) -> bool {
    state.is_subprocess_exit_requested(&task_id)
}
