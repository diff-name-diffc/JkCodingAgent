use anyhow::{anyhow, Context, Result};
use parking_lot::Mutex;
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::watch;
use tokio::time::{sleep, Duration};

use tauri::{AppHandle, Emitter, Manager};

use super::config::DispatcherAgentConfig;
use super::db::{
    AgentContext, AhaContextConfig, AhaSettingsV2, AhaSharedModels, ChatCategory, ChatSessionRecord,
    DispatcherDb, DispatcherMessageRecord, DispatcherMode, DispatcherModelConfig,
    DispatcherSessionKind, DispatcherSessionRecord, DispatcherSessionRuntimeState,
    DispatcherSessionTokenUsageRecord, DispatcherSessionTokenUsageSource,
    DispatcherSettingsModelConfigs, DispatcherSettingsRecord, DispatcherToolArtifactRecord,
    KeywordAction, ProjectSessionRecord, SessionKeywordRecord, SessionPage, SessionSearchResult,
};
use super::llm::OpenAiCompatProvider;
use super::llm::{self, ChatMessage};
use super::plain_chat::PlainChatAgent;
use super::runtime::{
    AgentEvent, AgentTurn, DispatchFeedbackState, DispatcherAgent, DispatcherSubprocessRegistry,
};
use super::sub_agent::SubAgentManager;
use super::summary::{
    fallback_session_title, parse_keyword_actions, summarize_session_keywords,
    summarize_session_title, SessionTitleMessage,
};
use super::tools::ToolRegistry;
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
    keywords_generations: Mutex<HashMap<String, u64>>,
    next_keywords_generation: AtomicU64,
    sub_agent_manager: Option<Arc<SubAgentManager>>,
    registered_tools: Mutex<Option<Vec<(String, String)>>>,
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherSaveSettingsPayload {
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
    chat_model_config: Option<DispatcherModelConfig>,
    summary_model_config: Option<DispatcherModelConfig>,
    vision_model_config: Option<DispatcherModelConfig>,
    image_model_config: Option<DispatcherModelConfig>,
    image_edit_model_config: Option<DispatcherModelConfig>,
    asr_model_config: Option<DispatcherModelConfig>,
    tts_model_config: Option<DispatcherModelConfig>,
    embedding_model_config: Option<DispatcherModelConfig>,
    chat_model_configs: Option<Vec<DispatcherModelConfig>>,
    summary_model_configs: Option<Vec<DispatcherModelConfig>>,
    vision_model_configs: Option<Vec<DispatcherModelConfig>>,
    image_model_configs: Option<Vec<DispatcherModelConfig>>,
    image_edit_model_configs: Option<Vec<DispatcherModelConfig>>,
    asr_model_configs: Option<Vec<DispatcherModelConfig>>,
    tts_model_configs: Option<Vec<DispatcherModelConfig>>,
    embedding_model_configs: Option<Vec<DispatcherModelConfig>>,
    allowed_tools: Option<Vec<String>>,
}

impl DispatcherState {
    pub fn new(project_mcp_registry: ProjectMcpRegistry) -> Result<Self> {
        let config = DispatcherAgentConfig::load()?;
        let db = DispatcherDb::new(config.db_path.clone())?;
        let subprocesses = Arc::new(DispatcherSubprocessRegistry::default());
        let sub_agent_manager =
            Arc::new(SubAgentManager::new(db.pool()));
        if let Err(e) = sub_agent_manager.load_all() {
            eprintln!("failed to load sub_agent configs: {}", e);
        }

        let initial_tool_registry =
            ToolRegistry::default_tools(project_mcp_registry.clone());
        let initial_tool_names = initial_tool_registry.tool_names_and_descriptions();

        Ok(Self {
            config,
            project_mcp_registry,
            subprocesses,
            db,
            active_runs: Mutex::new(HashMap::new()),
            next_run_generation: AtomicU64::new(1),
            title_generations: Mutex::new(HashMap::new()),
            next_title_generation: AtomicU64::new(1),
            keywords_generations: Mutex::new(HashMap::new()),
            next_keywords_generation: AtomicU64::new(1),
            sub_agent_manager: Some(sub_agent_manager),
            registered_tools: Mutex::new(Some(initial_tool_names)),
        })
    }

    pub fn sub_agent_manager(&self) -> Option<Arc<SubAgentManager>> {
        self.sub_agent_manager.clone()
    }

    pub fn registered_tool_names(&self) -> Option<Vec<(String, String)>> {
        self.registered_tools.lock().clone()
    }

    pub fn set_registered_tools(&self, tools: Vec<(String, String)>) {
        *self.registered_tools.lock() = Some(tools);
    }

    fn build_run_agent(&self) -> DispatcherAgent {
        let mut agent = DispatcherAgent::new(
            self.config.clone(),
            self.project_mcp_registry.clone(),
            Arc::clone(&self.subprocesses),
            self.sub_agent_manager.clone(),
        );

        if let Ok(v2) = self.db.get_settings_v2() {
            agent.apply_settings_v2(&v2, AgentContext::Project);
            agent.set_auto_approve_dispatch(v2.auto_approve_dispatch);
            agent.set_context_debug(v2.context_debug);
        } else if let Ok(Some(settings)) = self.db.get_settings() {
            agent.apply_settings(&settings);
            agent.set_auto_approve_dispatch(settings.auto_approve_dispatch);
            agent.set_context_debug(settings.context_debug);
        }

        if self.registered_tool_names().is_none() {
            self.set_registered_tools(agent.tools_arc().tool_names_and_descriptions());
        }

        agent
    }

    fn build_plain_chat_agent(&self) -> PlainChatAgent {
        let agent = PlainChatAgent::new(self.config.clone(), self.sub_agent_manager.clone());

        if let Ok(v2) = self.db.get_settings_v2() {
            agent.apply_settings_v2(&v2, AgentContext::Chat);
        } else if let Ok(Some(settings)) = self.db.get_settings() {
            agent.apply_settings(&settings);
        }

        agent
    }

    fn begin_run(&self, workspace_id: &str) -> Result<ActiveRunHandle, String> {
        let mut active_runs = self.active_runs.lock();
        if active_runs.contains_key(workspace_id) {
            return Err(format!(
                "会话 {} 已在运行中，请等待当前任务完成",
                workspace_id
            ));
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

    fn begin_keywords_generation(&self, workspace_id: &str) -> u64 {
        let generation = self.next_keywords_generation.fetch_add(1, Ordering::Relaxed);
        self.keywords_generations
            .lock()
            .insert(workspace_id.to_string(), generation);
        generation
    }

    fn finish_latest_keywords_generation(&self, workspace_id: &str, generation: u64) -> bool {
        let mut keywords_generations = self.keywords_generations.lock();
        if keywords_generations
            .get(workspace_id)
            .is_some_and(|current| *current == generation)
        {
            keywords_generations.remove(workspace_id);
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
    context: AgentContext,
    generation: u64,
) {
    let app = app.clone();
    let db = state.db.clone();
    let workspace_id = workspace_id.to_string();
    let fallback_content = fallback_content.to_string();

    tokio::spawn(async move {
        let title =
            generate_session_title(db.clone(), workspace_id.clone(), fallback_content, context)
                .await;
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

fn spawn_session_keywords_update(
    state: &DispatcherState,
    app: &AppHandle,
    workspace_id: &str,
    context: AgentContext,
    generation: u64,
) {
    let app = app.clone();
    let db = state.db.clone();
    let workspace_id = workspace_id.to_string();

    tokio::spawn(async move {
        let actions =
            generate_session_keywords(db.clone(), workspace_id.clone(), context).await;
        let state = app.state::<DispatcherState>();
        if !state.finish_latest_keywords_generation(&workspace_id, generation) {
            return;
        }

        if let Some(actions) = actions {
            let apply_db = db.clone();
            let apply_ws = workspace_id.clone();
            let apply_result = tokio::task::spawn_blocking(move || {
                apply_db.apply_keyword_actions(&apply_ws, &actions)
            })
            .await;

            match apply_result {
                Ok(Ok(())) => {
                    let list_db = db.clone();
                    let list_ws = workspace_id.clone();
                    if let Ok(Ok(keywords)) = tokio::task::spawn_blocking(move || {
                        list_db.list_session_keywords(&list_ws)
                    })
                    .await
                    {
                        let _ = app.emit(
                            "session-keywords-updated",
                            serde_json::json!({
                                "workspaceId": workspace_id,
                                "keywords": keywords,
                            }),
                        );
                    }
                }
                Ok(Err(error)) => {
                    eprintln!(
                        "failed to apply keyword actions for {}: {}",
                        workspace_id, error
                    );
                }
                Err(error) => {
                    eprintln!(
                        "keyword actions apply task failed for {}: {}",
                        workspace_id, error
                    );
                }
            }
        }
    });
}

async fn generate_session_keywords(
    db: DispatcherDb,
    workspace_id: String,
    context: AgentContext,
) -> Option<Vec<KeywordAction>> {
    let messages_db = db.clone();
    let messages_ws = workspace_id.clone();
    let messages = tokio::task::spawn_blocking(move || {
        messages_db.list_recent_visible_dialogue_messages(&messages_ws, 2)
    })
    .await;

    let messages = match messages {
        Ok(Ok(msgs)) => msgs,
        Ok(Err(error)) => {
            eprintln!("failed to load messages for keyword extraction: {error}");
            return None;
        }
        Err(error) => {
            eprintln!("keyword extraction message load task failed: {error}");
            return None;
        }
    };
    if messages.len() < 2 {
        return None;
    }

    let keywords_db = db.clone();
    let keywords_ws = workspace_id.clone();
    let existing = tokio::task::spawn_blocking(move || {
        keywords_db.list_session_keywords(&keywords_ws)
    })
    .await;

    let existing_keywords_json = match existing {
        Ok(Ok(records)) => {
            let kw: Vec<serde_json::Value> = records
                .iter()
                .map(|r| {
                    serde_json::json!({"keyword": r.keyword, "weight": r.weight})
                })
                .collect();
            serde_json::to_string(&kw).unwrap_or_else(|_| "[]".to_string())
        }
        Ok(Err(error)) => {
            eprintln!("failed to load existing keywords: {error}");
            "[]".to_string()
        }
        Err(error) => {
            eprintln!("existing keywords task failed: {error}");
            "[]".to_string()
        }
    };

    let qa_text = {
        let user = messages.iter().find(|m| m.role == "user");
        let assistant = messages.iter().find(|m| m.role == "assistant");
        let mut s = String::new();
        if let Some(u) = user {
            s.push_str("【用户】\n");
            s.push_str(&u.content);
            s.push('\n');
        }
        if let Some(a) = assistant {
            s.push_str("\n【助手】\n");
            let text = if a.content.len() > 2000 {
                format!("{}...", &a.content[..2000])
            } else {
                a.content.clone()
            };
            s.push_str(&text);
            s.push('\n');
        }
        s
    };

    let provider_db = db.clone();
    let provider_config =
        tokio::task::spawn_blocking(move || resolve_summary_provider(&provider_db, context)).await;

    let (provider, summary_model) = match provider_config {
        Ok(Ok(config)) => config,
        Ok(Err(error)) => {
            eprintln!("failed to resolve keywords summary provider: {error}");
            return None;
        }
        Err(error) => {
            eprintln!("keywords summary provider task failed: {error}");
            return None;
        }
    };

    if !provider.is_configured() {
        return None;
    }

    let usage_db = db.clone();
    let usage_ws = workspace_id.clone();
    let usage_model = summary_model.clone();
    match summarize_session_keywords(
        &provider,
        &summary_model,
        &qa_text,
        &existing_keywords_json,
        move |usage| {
            if let Err(error) = usage_db.upsert_session_token_usage(
                &usage_ws,
                &usage_model,
                DispatcherSessionTokenUsageSource::Summary,
                usage,
            ) {
                eprintln!(
                    "failed to persist keywords token usage for workspace {}: {}",
                    usage_ws, error
                );
            }
        },
    )
    .await
    {
        Ok(raw) => {
            let actions = parse_keyword_actions(&raw);
            if actions.is_empty() {
                eprintln!(
                    "no valid keyword actions parsed from raw response (len={})",
                    raw.len()
                );
                None
            } else {
                Some(actions)
            }
        }
        Err(error) => {
            eprintln!(
                "failed to call summarize_session_keywords with {}: {}",
                summary_model,
                error.message()
            );
            None
        }
    }
}

async fn run_dispatcher_db<T, F>(operation: &'static str, f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|error| format!("{operation} task failed: {error}"))?
        .map_err(|error| error.to_string())
}

async fn generate_session_title(
    db: DispatcherDb,
    workspace_id: String,
    fallback_content: String,
    context: AgentContext,
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
        tokio::task::spawn_blocking(move || resolve_summary_provider(&provider_db, context)).await;

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

fn resolve_summary_provider(
    db: &DispatcherDb,
    context: AgentContext,
) -> Result<(OpenAiCompatProvider, String)> {
    let config = DispatcherAgentConfig::load()?;
    let settings_v2 = db.get_settings_v2()?;
    let context_config = match context {
        AgentContext::Project => &settings_v2.project,
        AgentContext::Chat => &settings_v2.chat,
    };
    let summary = context_config
        .summary_model_configs
        .iter()
        .find(|item| item.active)
        .or_else(|| context_config.summary_model_configs.first())
        .ok_or_else(|| anyhow!("未配置 {:?} 摘要模型", context))?;
    let summary_model = summary.model.trim();
    if summary_model.is_empty() {
        return Err(anyhow!("未配置 {:?} 摘要模型名称", context));
    }

    Ok((
        OpenAiCompatProvider::new(
            if summary.api_key.trim().is_empty() {
                config.api_key.clone()
            } else {
                summary.api_key.trim().to_string()
            },
            if summary.url.trim().is_empty() {
                config.api_base.clone()
            } else {
                summary.url.trim().to_string()
            },
            summary_model.to_string(),
            96,
            config.temperature,
        ),
        summary_model.to_string(),
    ))
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
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
    let keywords_generation = state.begin_keywords_generation(&workspace_id);
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
    spawn_session_title_update(
        &state,
        &app,
        &workspace_id,
        &content,
        AgentContext::Project,
        title_generation,
    );
    spawn_session_keywords_update(
        &state,
        &app,
        &workspace_id,
        AgentContext::Project,
        keywords_generation,
    );
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
    let keywords_generation = state.begin_keywords_generation(&workspace_id);
    let run_handle = state.begin_run(&workspace_id).map_err(|e| e.to_string())?;
    let agent = state.build_plain_chat_agent().with_app_handle(app.clone());
    let result = agent
        .run(
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
    spawn_session_title_update(
        &state,
        &app,
        &workspace_id,
        &content,
        AgentContext::Chat,
        title_generation,
    );
    spawn_session_keywords_update(
        &state,
        &app,
        &workspace_id,
        AgentContext::Chat,
        keywords_generation,
    );
    result
}

#[tauri::command]
pub async fn dispatcher_list_messages(
    state: tauri::State<'_, DispatcherState>,
    workspace_id: String,
) -> Result<Vec<DispatcherMessageRecord>, String> {
    let db = state.db.clone();
    run_dispatcher_db("dispatcher_list_messages", move || {
        db.list_visible_messages(&workspace_id)
    })
    .await
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
pub async fn dispatcher_clear_messages(
    state: tauri::State<'_, DispatcherState>,
    workspace_id: String,
) -> Result<(), String> {
    let db = state.db.clone();
    run_dispatcher_db("dispatcher_clear_messages", move || {
        db.clear_messages(&workspace_id)
    })
    .await
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
pub async fn dispatcher_get_tool_artifact(
    state: tauri::State<'_, DispatcherState>,
    workspace_id: String,
    artifact_id: String,
) -> Result<DispatcherToolArtifactRecord, String> {
    let db = state.db.clone();
    run_dispatcher_db("dispatcher_get_tool_artifact", move || {
        db.get_tool_artifact(&workspace_id, &artifact_id)
    })
    .await
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
    category: Option<String>,
) -> Result<DispatcherSessionRecord, String> {
    let kind = DispatcherSessionKind::from_wire(kind.as_deref().unwrap_or("project"))
        .map_err(|error| error.to_string())?;
    let mode = DispatcherMode::from_wire(mode.as_deref().unwrap_or("default"))
        .map_err(|error| error.to_string())?;
    let session = state
        .db
        .create_session(
            &project_id,
            &title,
            kind,
            mode,
            active_plan_path.as_deref(),
            category.as_deref(),
        )
        .map_err(|error| error.to_string())?;
    let _ = app.emit("dispatcher-session-updated", session.clone());
    Ok(session)
}

#[tauri::command]
pub fn dispatcher_get_session_runtime_state(
    state: tauri::State<'_, DispatcherState>,
    workspace_id: String,
) -> Result<DispatcherSessionRuntimeState, String> {
    state
        .db
        .get_session_runtime_state(&workspace_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn dispatcher_set_session_mode(
    state: tauri::State<'_, DispatcherState>,
    workspace_id: String,
    mode: String,
) -> Result<DispatcherSessionRuntimeState, String> {
    let mode = DispatcherMode::from_wire(&mode).map_err(|error| error.to_string())?;
    state
        .db
        .set_session_mode(&workspace_id, mode)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn dispatcher_delete_session(
    state: tauri::State<'_, DispatcherState>,
    workspace_id: String,
) -> Result<(), String> {
    let db = state.db.clone();
    run_dispatcher_db("dispatcher_delete_session", move || {
        db.delete_session(&workspace_id)
    })
    .await
}

#[tauri::command]
pub async fn session_get_keywords(
    state: tauri::State<'_, DispatcherState>,
    workspace_id: String,
) -> Result<Vec<SessionKeywordRecord>, String> {
    let db = state.db.clone();
    run_dispatcher_db("session_get_keywords", move || {
        db.list_session_keywords(&workspace_id)
    })
    .await
}

#[tauri::command]
pub async fn session_search_keywords(
    state: tauri::State<'_, DispatcherState>,
    query: String,
    limit: Option<i64>,
    kind: Option<String>,
    project_id: Option<String>,
) -> Result<Vec<SessionSearchResult>, String> {
    let db = state.db.clone();
    let lim = limit.unwrap_or(20);
    run_dispatcher_db("session_search_keywords", move || {
        db.search_sessions_by_keywords(
            &query,
            lim,
            kind.as_deref(),
            project_id.as_deref(),
        )
    })
    .await
}

// ── v6: Chat Sessions (paginated) ─────────────────────────────

#[tauri::command]
pub async fn chat_list_sessions(
    state: tauri::State<'_, DispatcherState>,
    category: Option<String>,
    cursor: Option<String>,
    page_size: Option<i64>,
) -> Result<SessionPage<ChatSessionRecord>, String> {
    let db = state.db.clone();
    let size = page_size.unwrap_or(30);
    run_dispatcher_db("chat_list_sessions", move || {
        db.list_chat_sessions_paginated(category.as_deref(), cursor.as_deref(), size)
    })
    .await
}

#[tauri::command]
pub async fn chat_create_session(
    state: tauri::State<'_, DispatcherState>,
    app: AppHandle,
    title: String,
    category: Option<String>,
) -> Result<ChatSessionRecord, String> {
    let db = state.db.clone();
    let session = run_dispatcher_db("chat_create_session", move || {
        db.create_chat_session(&title, category.as_deref())
    })
    .await?;
    let _ = app.emit("dispatcher-session-updated", session.clone());
    Ok(session)
}

#[tauri::command]
pub async fn chat_delete_session(
    state: tauri::State<'_, DispatcherState>,
    session_id: String,
) -> Result<(), String> {
    let db = state.db.clone();
    run_dispatcher_db("chat_delete_session", move || {
        db.delete_chat_session(&session_id)
    })
    .await
}

#[tauri::command]
pub async fn chat_update_session_title(
    state: tauri::State<'_, DispatcherState>,
    session_id: String,
    title: String,
) -> Result<Option<ChatSessionRecord>, String> {
    let db = state.db.clone();
    run_dispatcher_db("chat_update_session_title", move || {
        db.update_chat_session_title(&session_id, &title)
    })
    .await
}

#[tauri::command]
pub async fn chat_set_session_category_v6(
    state: tauri::State<'_, DispatcherState>,
    session_id: String,
    category_id: String,
) -> Result<(), String> {
    let db = state.db.clone();
    run_dispatcher_db("chat_set_session_category_v6", move || {
        db.set_chat_session_category(&session_id, &category_id)
    })
    .await
}

// ── v6: Project Sessions (paginated) ──────────────────────────

#[tauri::command]
pub async fn project_list_sessions(
    state: tauri::State<'_, DispatcherState>,
    project_id: String,
    offset: Option<i64>,
    page_size: Option<i64>,
) -> Result<SessionPage<ProjectSessionRecord>, String> {
    let db = state.db.clone();
    let off = offset.unwrap_or(0);
    let size = page_size.unwrap_or(30);
    run_dispatcher_db("project_list_sessions", move || {
        db.list_project_sessions_paginated(&project_id, off, size)
    })
    .await
}

#[tauri::command]
pub async fn project_create_session(
    state: tauri::State<'_, DispatcherState>,
    app: AppHandle,
    project_id: String,
    title: String,
    mode: Option<String>,
    active_plan_path: Option<String>,
) -> Result<ProjectSessionRecord, String> {
    let db = state.db.clone();
    let mode = DispatcherMode::from_wire(mode.as_deref().unwrap_or("default"))
        .map_err(|e| e.to_string())?;
    let session = run_dispatcher_db("project_create_session", move || {
        db.create_project_session(&project_id, &title, mode, active_plan_path.as_deref())
    })
    .await?;
    let _ = app.emit("dispatcher-session-updated", session.clone());
    Ok(session)
}

#[tauri::command]
pub async fn project_delete_session(
    state: tauri::State<'_, DispatcherState>,
    session_id: String,
) -> Result<(), String> {
    let db = state.db.clone();
    run_dispatcher_db("project_delete_session", move || {
        db.delete_project_session(&session_id)
    })
    .await
}

#[tauri::command]
pub async fn chat_list_categories(
    state: tauri::State<'_, DispatcherState>,
) -> Result<Vec<ChatCategory>, String> {
    let db = state.db.clone();
    run_dispatcher_db("chat_list_categories", move || db.list_chat_categories()).await
}

#[tauri::command]
pub async fn chat_create_category(
    state: tauri::State<'_, DispatcherState>,
    name: String,
    icon: Option<String>,
    color: Option<String>,
) -> Result<ChatCategory, String> {
    let db = state.db.clone();
    run_dispatcher_db("chat_create_category", move || {
        db.create_chat_category(
            &name,
            icon.as_deref().unwrap_or(""),
            color.as_deref().unwrap_or(""),
        )
    })
    .await
}

#[tauri::command]
pub async fn chat_update_category(
    state: tauri::State<'_, DispatcherState>,
    category_id: String,
    name: Option<String>,
    icon: Option<String>,
    color: Option<String>,
) -> Result<Option<ChatCategory>, String> {
    let db = state.db.clone();
    run_dispatcher_db("chat_update_category", move || {
        db.update_chat_category(
            &category_id,
            name.as_deref(),
            icon.as_deref(),
            color.as_deref(),
        )
    })
    .await
}

#[tauri::command]
pub async fn chat_delete_category(
    state: tauri::State<'_, DispatcherState>,
    category_id: String,
) -> Result<(), String> {
    let db = state.db.clone();
    run_dispatcher_db("chat_delete_category", move || {
        db.delete_chat_category(&category_id)
    })
    .await
}

#[tauri::command]
pub async fn chat_set_session_category(
    state: tauri::State<'_, DispatcherState>,
    workspace_id: String,
    category_id: String,
) -> Result<(), String> {
    let db = state.db.clone();
    run_dispatcher_db("chat_set_session_category", move || {
        db.set_session_category(&workspace_id, &category_id)
    })
    .await
}

#[tauri::command]
pub async fn chat_reorder_categories(
    state: tauri::State<'_, DispatcherState>,
    ordered_ids: Vec<String>,
) -> Result<(), String> {
    let db = state.db.clone();
    run_dispatcher_db("chat_reorder_categories", move || {
        db.reorder_chat_categories(&ordered_ids)
    })
    .await
}

#[tauri::command]
pub async fn dispatcher_save_settings(
    state: tauri::State<'_, DispatcherState>,
    settings: DispatcherSaveSettingsPayload,
) -> Result<DispatcherSettingsRecord, String> {
    let record = state
        .db
        .save_settings_with_model_configs(
            &settings.api_base,
            &settings.api_key,
            &settings.model,
            &settings.summary_model,
            &settings.vision_model,
            &settings.asr_api_key,
            &settings.asr_websocket_url,
            settings.auto_approve_dispatch,
            settings.context_debug,
            &settings.image_model_url,
            &settings.image_model_api_key,
            &settings.image_model,
            &settings.image_edit_model,
            DispatcherSettingsModelConfigs {
                chat_model_config: settings.chat_model_config,
                summary_model_config: settings.summary_model_config,
                vision_model_config: settings.vision_model_config,
                image_model_config: settings.image_model_config,
                image_edit_model_config: settings.image_edit_model_config,
                asr_model_config: settings.asr_model_config,
                tts_model_config: settings.tts_model_config,
                embedding_model_config: settings.embedding_model_config,
                chat_model_configs: settings.chat_model_configs,
                summary_model_configs: settings.summary_model_configs,
                vision_model_configs: settings.vision_model_configs,
                image_model_configs: settings.image_model_configs,
                image_edit_model_configs: settings.image_edit_model_configs,
                asr_model_configs: settings.asr_model_configs,
                tts_model_configs: settings.tts_model_configs,
                embedding_model_configs: settings.embedding_model_configs,
            },
            settings.allowed_tools.unwrap_or_default(),
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
        .map_err(|error| format_anyhow_chain(error))
}

#[tauri::command]
pub async fn dispatcher_test_model(
    kind: String,
    config: DispatcherModelConfig,
) -> Result<String, String> {
    test_dispatcher_model(&kind, config)
        .await
        .map_err(|error| format_anyhow_chain(error))
}

async fn test_dispatcher_model(kind: &str, config: DispatcherModelConfig) -> Result<String> {
    match kind {
        "chat" => test_chat_compatible_model("聊天主模型", config, false).await,
        "summary" => test_chat_compatible_model("摘要模型", config, false).await,
        "vision" => test_chat_compatible_model("视觉模型", config, true).await,
        "embedding" => test_embedding_model(config).await,
        "asr" => test_required_model_config("ASR 模型", &config)
            .map(|_| "ASR 配置字段完整，未启动真实录音会话。".to_string()),
        "image" => test_endpoint_reachable_model("图片模型", config).await,
        "imageEdit" => test_endpoint_reachable_model("图片编辑模型", config).await,
        "tts" => test_endpoint_reachable_model("TTS 模型", config).await,
        other => Err(anyhow!("未知模型类型：{other}")),
    }
}

async fn test_chat_compatible_model(
    label: &str,
    config: DispatcherModelConfig,
    enable_multimodal: bool,
) -> Result<String> {
    test_required_model_config(label, &config)?;
    let model_name = config.model.trim().to_string();
    let provider = OpenAiCompatProvider::new(config.api_key, config.url, config.model, 64, 0.0);
    let messages = vec![ChatMessage::system("只输出 pong。".to_string())];
    let response = provider
        .chat_stream(&messages, &[], enable_multimodal, |_| {})
        .await
        .with_context(|| format!("{label} 测试请求失败（模型 {model_name}）"))?;
    let content = response.content.trim().to_string();
    if content.is_empty() {
        anyhow::bail!("{label}（{model_name}）返回空内容");
    }
    Ok(format!("{label} ok（{model_name}）：{content}"))
}

async fn test_embedding_model(config: DispatcherModelConfig) -> Result<String> {
    test_required_model_config("文本向量模型", &config)?;
    let model_name = config.model.trim().to_string();
    let endpoint = embedding_endpoint(&config.url);
    let response = Client::new()
        .post(&endpoint)
        .bearer_auth(config.api_key.trim())
        .json(&serde_json::json!({
            "model": config.model.trim(),
            "input": "ping"
        }))
        .send()
        .await
        .with_context(|| format!("文本向量模型请求失败（模型 {model_name}，端点 {endpoint}）"))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!("文本向量模型（{model_name}）测试失败，HTTP {status}：{body}");
    }
    let value: Value = serde_json::from_str(&body).with_context(|| {
        format!(
            "文本向量模型（{model_name}）响应解析失败，响应内容：{}",
            &body[..body.len().min(500)]
        )
    })?;
    let dimension = value
        .get("data")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|item| item.get("embedding"))
        .and_then(Value::as_array)
        .map(Vec::len)
        .ok_or_else(|| {
            let preview = &body[..body.len().min(300)];
            anyhow!(
                "文本向量模型（{model_name}）响应中未找到 data[0].embedding，响应结构：{preview}"
            )
        })?;

    Ok(format!("文本向量模型 ok（{model_name}），维度 {dimension}"))
}

async fn test_endpoint_reachable_model(
    label: &str,
    config: DispatcherModelConfig,
) -> Result<String> {
    test_required_model_config(label, &config)?;
    let model_name = config.model.trim().to_string();
    let url = config.url.trim().to_string();
    let response = Client::new()
        .get(&url)
        .bearer_auth(config.api_key.trim())
        .send()
        .await
        .with_context(|| format!("{label} 端点连通性测试失败（模型 {model_name}，端点 {url}）"))?;
    let status = response.status();
    if status.is_server_error() {
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("{label}（{model_name}）端点返回 HTTP {status}：{body}");
    }
    if status.is_client_error() {
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!(
            "{label}（{model_name}）端点返回 HTTP {status}（请检查 API Key 和 URL），响应：{body}"
        );
    }
    Ok(format!(
        "{label} ok（{model_name}），端点 HTTP {status} 可达"
    ))
}

fn test_required_model_config(label: &str, config: &DispatcherModelConfig) -> Result<()> {
    if config.url.trim().is_empty() {
        anyhow::bail!("{label} URL 未配置（请在 API Base URL 中填入服务商端点地址）");
    }
    if config.api_key.trim().is_empty() {
        anyhow::bail!("{label} API Key 未配置（请在 API Key 中填入服务商提供的密钥）");
    }
    if config.model.trim().is_empty() {
        anyhow::bail!("{label} 模型名称未配置（请在 Model 中填入具体模型 ID，如 gpt-4o）");
    }
    Ok(())
}

fn format_anyhow_chain(error: anyhow::Error) -> String {
    error
        .chain()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("：")
}

fn embedding_endpoint(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches('/');
    if trimmed.ends_with("/embeddings") {
        trimmed.to_string()
    } else if trimmed.ends_with("/v1") {
        format!("{trimmed}/embeddings")
    } else {
        format!("{trimmed}/v1/embeddings")
    }
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
pub async fn aha_get_settings_v2(
    state: tauri::State<'_, DispatcherState>,
) -> Result<AhaSettingsV2, String> {
    state.db.get_settings_v2().map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn aha_save_settings_v2(
    state: tauri::State<'_, DispatcherState>,
    settings: AhaSettingsV2,
) -> Result<AhaSettingsV2, String> {
    state
        .db
        .save_settings_v2(&settings)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn aha_get_context_config(
    state: tauri::State<'_, DispatcherState>,
    context: String,
) -> Result<AhaContextConfig, String> {
    let ctx = AgentContext::from_wire(&context).map_err(|e| e.to_string())?;
    state
        .db
        .get_settings_for_context(ctx)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn aha_get_shared_models(
    state: tauri::State<'_, DispatcherState>,
) -> Result<AhaSharedModels, String> {
    state
        .db
        .get_shared_models()
        .map_err(|error| error.to_string())
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
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
    workspace_id: String,
    dispatch_id: String,
    task_id: String,
) -> Result<DispatcherSessionRuntimeState, String> {
    state
        .db
        .attach_checklist_subprocess(&workspace_id, &dispatch_id, &task_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn dispatcher_clear_checklist_dispatch(
    state: tauri::State<'_, DispatcherState>,
    workspace_id: String,
    dispatch_id: String,
) -> Result<DispatcherSessionRuntimeState, String> {
    state
        .db
        .clear_checklist_dispatch(&workspace_id, &dispatch_id)
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
    task_manager
        .set_task_termination_intent(&task_id, crate::shared::TaskTerminationIntent::Stopped);
    task_manager.write_to_pty(&task_id, b"\x03", true)?;

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    if has_live_subprocess(&task_manager, &task_id) {
        task_manager
            .set_task_termination_intent(&task_id, crate::shared::TaskTerminationIntent::Stopped);
        let _ = task_manager.kill_child(&task_id);
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
