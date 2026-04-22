use anyhow::Result;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::watch;
use tokio::time::{sleep, Duration};

use tauri::AppHandle;
use uuid::Uuid;

use super::cad::{
    CadReviewRunDetail, CadReviewRunRecord, DispatcherAttachmentRecord, DwgDocumentRecord,
    DwgParseCacheRecord, DwgViewerCommandResult, DwgViewerSessionRegistration,
    DwgViewerSessionState, SaveDwgDocumentIndexInput, SaveDwgEntityPayloadsInput,
    SaveDwgParseCacheInput,
};
use super::config::DispatcherAgentConfig;
use super::db::{
    DispatcherDb, DispatcherMessageRecord, DispatcherSessionRecord, DispatcherSettingsRecord,
    DispatcherToolArtifactRecord,
};
use super::dwg::viewer_bridge::DwgViewerBridgeState;
use super::llm;
use super::runtime::{AgentEvent, AgentTurn, DispatchFeedbackState, DispatcherAgent};
use crate::project::mcp::ProjectMcpRegistry;
use crate::shared::TaskManager;

pub struct DispatcherState {
    agent: tokio::sync::Mutex<DispatcherAgent>,
    db: DispatcherDb,
    viewer_bridge: Arc<DwgViewerBridgeState>,
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
            viewer_bridge: DwgViewerBridgeState::new(),
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

fn ensure_absolute_path(path: &str, label: &str) -> Result<PathBuf, String> {
    let value = PathBuf::from(path);
    if !value.is_absolute() {
        return Err(format!("{label} 必须是绝对路径"));
    }
    Ok(value)
}

fn attachment_mime_type(file_name: &str) -> String {
    let lower = file_name.to_ascii_lowercase();
    if lower.ends_with(".md") || lower.ends_with(".markdown") {
        "text/markdown".to_string()
    } else if lower.ends_with(".json") {
        "application/json".to_string()
    } else if lower.ends_with(".yaml") || lower.ends_with(".yml") {
        "application/yaml".to_string()
    } else if lower.ends_with(".dwg") {
        "application/acad".to_string()
    } else if lower.ends_with(".txt") {
        "text/plain".to_string()
    } else {
        "application/octet-stream".to_string()
    }
}

fn copy_attachment_into_workspace(
    project_path: &str,
    workspace_id: &str,
    source_path: &str,
) -> Result<(String, String, u64, String), String> {
    let project_root = ensure_absolute_path(project_path, "projectPath")?;
    let source = ensure_absolute_path(source_path, "sourcePath")?;
    if !source.exists() {
        return Err(format!("附件文件不存在：{}", source.display()));
    }
    if !source.is_file() {
        return Err(format!("附件路径不是文件：{}", source.display()));
    }

    let file_name = source
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "附件文件名无效".to_string())?
        .to_string();
    let size_bytes = source
        .metadata()
        .map_err(|error| format!("读取附件元数据失败：{error}"))?
        .len();
    let attachment_dir = project_root
        .join(".nezha")
        .join("dispatcher-attachments")
        .join(workspace_id);
    std::fs::create_dir_all(&attachment_dir)
        .map_err(|error| format!("创建附件目录失败：{error}"))?;
    let target_path = attachment_dir.join(format!("{}_{}", Uuid::new_v4(), file_name));
    std::fs::copy(&source, &target_path).map_err(|error| format!("复制附件失败：{error}"))?;

    Ok((
        file_name.clone(),
        target_path.to_string_lossy().into_owned(),
        size_bytes,
        attachment_mime_type(&file_name),
    ))
}

#[tauri::command]
pub async fn dispatcher_send_message(
    state: tauri::State<'_, DispatcherState>,
    app: AppHandle,
    workspace_id: String,
    project_path: String,
    content: String,
    attachments: Option<Vec<String>>,
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
            attachments.as_deref().unwrap_or(&[]),
            Some(app),
            Some(state.viewer_bridge.clone()),
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
pub fn dispatcher_upload_attachment(
    state: tauri::State<'_, DispatcherState>,
    workspace_id: String,
    project_path: String,
    source_path: String,
) -> Result<DispatcherAttachmentRecord, String> {
    let (original_name, stored_path, size_bytes, mime_type) =
        copy_attachment_into_workspace(&project_path, &workspace_id, &source_path)?;
    state
        .db
        .create_attachment(
            &workspace_id,
            &original_name,
            &stored_path,
            &mime_type,
            size_bytes,
        )
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn dispatcher_list_pending_attachments(
    state: tauri::State<'_, DispatcherState>,
    workspace_id: String,
) -> Result<Vec<DispatcherAttachmentRecord>, String> {
    state
        .db
        .list_pending_attachments(&workspace_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn dispatcher_delete_pending_attachment(
    state: tauri::State<'_, DispatcherState>,
    workspace_id: String,
    attachment_id: String,
) -> Result<(), String> {
    let record = state
        .db
        .get_attachments_by_ids(&workspace_id, &[attachment_id.clone()])
        .map_err(|error| error.to_string())?
        .into_iter()
        .next();
    state
        .db
        .delete_pending_attachment(&workspace_id, &attachment_id)
        .map_err(|error| error.to_string())?;
    if let Some(record) = record {
        let _ = std::fs::remove_file(&record.stored_path);
    }
    Ok(())
}

#[tauri::command]
pub fn dispatcher_get_dwg_parse_cache(
    state: tauri::State<'_, DispatcherState>,
    project_path: String,
    file_path: String,
    file_size: u64,
    file_mtime: i64,
    parser_version: String,
) -> Result<Option<DwgParseCacheRecord>, String> {
    state
        .db
        .get_dwg_parse_cache(
            &project_path,
            &file_path,
            file_size,
            file_mtime,
            &parser_version,
        )
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn dispatcher_save_dwg_parse_cache(
    state: tauri::State<'_, DispatcherState>,
    payload: SaveDwgParseCacheInput,
) -> Result<DwgParseCacheRecord, String> {
    state
        .db
        .save_dwg_parse_cache(&payload)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn dispatcher_get_dwg_document_record(
    state: tauri::State<'_, DispatcherState>,
    project_path: String,
    file_path: String,
    file_size: u64,
    file_mtime: i64,
    parser_version: String,
) -> Result<Option<DwgDocumentRecord>, String> {
    state
        .db
        .get_dwg_document(
            &project_path,
            &file_path,
            file_size,
            file_mtime,
            &parser_version,
        )
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn dispatcher_upsert_dwg_document_index(
    state: tauri::State<'_, DispatcherState>,
    payload: SaveDwgDocumentIndexInput,
) -> Result<DwgDocumentRecord, String> {
    state
        .db
        .upsert_dwg_document_index(&payload)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn dispatcher_upsert_dwg_entity_payloads(
    state: tauri::State<'_, DispatcherState>,
    payload: SaveDwgEntityPayloadsInput,
) -> Result<(), String> {
    state
        .db
        .upsert_dwg_entity_payloads(&payload)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn dispatcher_list_cad_review_runs(
    state: tauri::State<'_, DispatcherState>,
    workspace_id: String,
    file_path: Option<String>,
) -> Result<Vec<CadReviewRunRecord>, String> {
    state
        .db
        .list_cad_review_runs(&workspace_id, file_path.as_deref())
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn dispatcher_get_cad_review_run_detail(
    state: tauri::State<'_, DispatcherState>,
    workspace_id: String,
    run_id: String,
) -> Result<CadReviewRunDetail, String> {
    state
        .db
        .get_cad_review_run_detail(&workspace_id, &run_id)
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
pub fn dispatcher_register_dwg_viewer_session(
    state: tauri::State<'_, DispatcherState>,
    payload: DwgViewerSessionRegistration,
) -> Result<DwgViewerSessionState, String> {
    Ok(state.viewer_bridge.register_session(payload))
}

#[tauri::command]
pub fn dispatcher_unregister_dwg_viewer_session(
    state: tauri::State<'_, DispatcherState>,
    session_id: String,
) -> Result<(), String> {
    state.viewer_bridge.unregister_session(&session_id);
    Ok(())
}

#[tauri::command]
pub fn dispatcher_update_dwg_viewer_state(
    state: tauri::State<'_, DispatcherState>,
    payload: DwgViewerSessionRegistration,
) -> Result<DwgViewerSessionState, String> {
    Ok(state.viewer_bridge.update_state(payload.into_state()))
}

#[tauri::command]
pub fn dispatcher_resolve_dwg_viewer_command(
    state: tauri::State<'_, DispatcherState>,
    payload: DwgViewerCommandResult,
) -> Result<(), String> {
    state
        .viewer_bridge
        .resolve_command_result(payload)
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::{copy_attachment_into_workspace, ensure_absolute_path};

    fn create_temp_dir(prefix: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("{prefix}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).expect("create temp dir");
        root
    }

    #[test]
    fn ensure_absolute_path_rejects_relative_input() {
        let error = ensure_absolute_path("relative/path.md", "sourcePath").unwrap_err();
        assert!(error.contains("sourcePath 必须是绝对路径"));
    }

    #[test]
    fn copy_attachment_into_workspace_copies_file_under_workspace_storage() {
        let project_root = create_temp_dir("dispatcher-project");
        let source_root = create_temp_dir("dispatcher-source");
        let source_path = source_root.join("rules.md");
        fs::write(&source_path, "# 审查规则\n- 检查尺寸").expect("write source attachment");

        let (original_name, stored_path, size_bytes, mime_type) = copy_attachment_into_workspace(
            &project_root.to_string_lossy(),
            "session-1",
            &source_path.to_string_lossy(),
        )
        .expect("copy attachment into workspace");

        assert_eq!(original_name, "rules.md");
        assert_eq!(mime_type, "text/markdown");
        assert!(size_bytes > 0);
        assert!(stored_path.starts_with(
            project_root
                .join(".nezha")
                .join("dispatcher-attachments")
                .join("session-1")
                .to_string_lossy()
                .as_ref()
        ));
        assert_eq!(
            fs::read_to_string(&stored_path).expect("read copied attachment"),
            "# 审查规则\n- 检查尺寸"
        );

        let _ = fs::remove_dir_all(project_root);
        let _ = fs::remove_dir_all(source_root);
    }
}
