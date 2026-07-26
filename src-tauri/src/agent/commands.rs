use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use serde_json::Value;
use tokio::time::{sleep, Duration};

use super::agents::DispatcherContinueAfterDispatchRequest;
use super::config::DispatcherAgentConfig;
use super::db::content::{segments_to_plain_text, try_parse_segments_json, ContentSegment};
use super::db::{
    AgentContext, AhaContextConfig, AhaSettingsV2, ChatCategory, ChatCategoryAgentConfig,
    ChatSessionRecord, DispatcherDb, DispatcherMessageRecord, DispatcherModelConfig,
    DispatcherSessionKind, DispatcherSessionTokenUsageRecord, DispatcherSessionTokenUsageSource,
    DispatcherToolArtifactRecord, KeywordAction, ProjectSessionRecord, SessionPage,
    SessionSearchResult,
};
use super::llm::OpenAiCompatProvider;
use super::llm::{self, ChatMessage};
use super::llm::{ChatMessageContentPart, ChatMessageImageSource};
use super::run_loop::{
    run_agent_turn, AgentEvent, AgentRunRequest, AgentTurn, DispatchFeedbackState, RuntimeAgentKind,
};
use super::state::DispatcherState;
use super::sub_agent::db::ToolInfo;
use super::summary::{
    fallback_session_title, parse_keyword_actions, summarize_session_keywords,
    summarize_session_title, SessionTitleMessage,
};
use super::voice::{resolve_dashscope_websocket_url, VoiceAsrConfig, VoiceAsrManager};
use crate::browser::BrowserManager;
use crate::shared::TaskManager;
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, Manager};

const SESSION_TITLE_RECENT_DIALOGUES: usize = 3;

fn has_live_subprocess(task_manager: &TaskManager, task_id: &str) -> bool {
    task_manager.child_handles.lock().contains_key(task_id)
}

fn resolve_voice_asr_config(state: &DispatcherState) -> Result<VoiceAsrConfig, String> {
    let saved = state
        .db()
        .get_settings_v2()
        .map_err(|error| error.to_string())?;
    let loaded = DispatcherAgentConfig::load().ok();
    let asr_config = saved
        .shared
        .asr_model_configs
        .iter()
        .find(|config| config.active)
        .or_else(|| saved.shared.asr_model_configs.first());

    let api_key = asr_config
        .map(|config| config.api_key.trim())
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
            asr_config
                .filter(|config| config.url.contains("dashscope"))
                .map(|config| config.api_key.trim())
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

    let saved_websocket_url = asr_config
        .map(|config| config.url.trim())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);

    let api_base = saved_websocket_url.clone().or_else(|| {
        loaded
            .as_ref()
            .map(|config| config.api_base.trim())
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    });

    Ok(VoiceAsrConfig {
        api_key,
        websocket_url: saved_websocket_url.unwrap_or_else(|| {
            resolve_dashscope_websocket_url(
                api_base
                    .as_deref()
                    .unwrap_or("https://dashscope.aliyuncs.com/compatible-mode/v1"),
            )
        }),
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
    segments_json: &str,
    context: AgentContext,
    generation: u64,
) {
    let app = app.clone();
    let db = state.db().clone();
    let workspace_id = workspace_id.to_string();
    let segments_json = segments_json.to_string();

    tokio::spawn(async move {
        let title =
            generate_session_title(db.clone(), workspace_id.clone(), segments_json, context).await;
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
    let db = state.db().clone();
    let workspace_id = workspace_id.to_string();

    tokio::spawn(async move {
        let actions = generate_session_keywords(db.clone(), workspace_id.clone(), context).await;
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
                    if let Ok(Ok(keywords)) =
                        tokio::task::spawn_blocking(move || list_db.list_session_keywords(&list_ws))
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
    let existing =
        tokio::task::spawn_blocking(move || keywords_db.list_session_keywords(&keywords_ws)).await;

    let existing_keywords_json = match existing {
        Ok(Ok(records)) => {
            let kw: Vec<serde_json::Value> = records
                .iter()
                .map(|r| serde_json::json!({"keyword": r.keyword, "weight": r.weight}))
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
                // Find the nearest char boundary at or before byte 2000
                let boundary = a.content.floor_char_boundary(2000);
                format!("{}...", &a.content[..boundary])
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
    segments_json: String,
    context: AgentContext,
) -> String {
    let current_user_segments = try_parse_segments_json(&segments_json).unwrap_or_else(|error| {
        eprintln!("failed to parse current user segments for title generation: {error}");
        Vec::new()
    });
    let current_user_content = segments_to_plain_text(&current_user_segments);
    let current_user_parts = title_content_parts_from_segments(&current_user_segments);

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
        .unwrap_or_else(|| current_user_content.clone());
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
        &current_user_parts,
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

fn title_content_parts_from_segments(segments: &[ContentSegment]) -> Vec<ChatMessageContentPart> {
    segments
        .iter()
        .filter_map(|segment| match segment {
            ContentSegment::Text { text, .. } if !text.trim().is_empty() => {
                Some(ChatMessageContentPart::Text { text: text.clone() })
            }
            ContentSegment::Image { image_id, .. } => Some(ChatMessageContentPart::Image {
                source: ChatMessageImageSource::ChatImage {
                    image_id: image_id.clone(),
                },
            }),
            ContentSegment::Text { .. } | ContentSegment::File { .. } => None,
        })
        .collect()
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
            // 关键字摘要输出 JSON 数组（最多 15 项）需要较大预算；也兼容仍会思考的摘要
            // 模型（思考 token 计入上限）。非思考模型输出完即停，此处仅作上限保护。
            2048,
            config.temperature,
        ),
        summary_model.to_string(),
    ))
}

#[tauri::command]
pub async fn dispatcher_send_project_agent_message(
    state: tauri::State<'_, DispatcherState>,
    app: AppHandle,
    workspace_id: String,
    project_path: String,
    segments_json: String,
    on_event: Channel<AgentEvent>,
) -> Result<AgentTurn, String> {
    let title_segments_json = segments_json.clone();
    let agent = state.build_run_agent().with_app_handle(app.clone());
    let run_handle = state.begin_run(&workspace_id).map_err(|e| e.to_string())?;
    let title_generation_index = state.begin_title_generation(&workspace_id);
    let keywords_generation_index = state.begin_keywords_generation(&workspace_id);
    let result = run_agent_turn(
        &agent,
        AgentRunRequest {
            kind: RuntimeAgentKind::Project,
            db: state.db(),
            workspace_id: &workspace_id,
            workspace_path: Some(&project_path),
            user_segments_json: segments_json,
            on_event,
            cancel_rx: run_handle.cancel_rx,
        },
    )
    .await
    .map_err(|error| error.to_string());
    state.finish_run(&workspace_id);
    spawn_session_title_update(
        &state,
        &app,
        &workspace_id,
        &title_segments_json,
        AgentContext::Project,
        title_generation_index,
    );
    spawn_session_keywords_update(
        &state,
        &app,
        &workspace_id,
        AgentContext::Project,
        keywords_generation_index,
    );
    result
}

#[tauri::command]
pub async fn dispatcher_send_chat_agent_message(
    state: tauri::State<'_, DispatcherState>,
    app: AppHandle,
    workspace_id: String,
    segments_json: String,
    on_event: Channel<AgentEvent>,
) -> Result<AgentTurn, String> {
    let title_segments_json = segments_json.clone();
    let agent = state
        .build_plain_chat_agent(&workspace_id)?
        .with_app_handle(app.clone());
    let run_handle = state.begin_run(&workspace_id).map_err(|e| e.to_string())?;
    let title_generation_index = state.begin_title_generation(&workspace_id);
    let keywords_generation_index = state.begin_keywords_generation(&workspace_id);
    let result = run_agent_turn(
        &agent,
        AgentRunRequest {
            kind: RuntimeAgentKind::PlainChat,
            db: state.db(),
            workspace_id: &workspace_id,
            workspace_path: None,
            user_segments_json: segments_json,
            on_event,
            cancel_rx: run_handle.cancel_rx,
        },
    )
    .await
    .map_err(|error| error.to_string());
    state.finish_run(&workspace_id);
    spawn_session_title_update(
        &state,
        &app,
        &workspace_id,
        &title_segments_json,
        AgentContext::Chat,
        title_generation_index,
    );
    spawn_session_keywords_update(
        &state,
        &app,
        &workspace_id,
        AgentContext::Chat,
        keywords_generation_index,
    );
    result
}

#[tauri::command]
pub async fn dispatcher_list_messages(
    state: tauri::State<'_, DispatcherState>,
    workspace_id: String,
) -> Result<Vec<DispatcherMessageRecord>, String> {
    let db = state.db().clone();
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
        .db()
        .list_session_token_usage(&workspace_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn dispatcher_clear_messages(
    state: tauri::State<'_, DispatcherState>,
    workspace_id: String,
) -> Result<(), String> {
    let db = state.db().clone();
    run_dispatcher_db("dispatcher_clear_messages", move || {
        db.clear_messages(&workspace_id)
    })
    .await
}

#[tauri::command]
pub async fn dispatcher_truncate_messages_from(
    state: tauri::State<'_, DispatcherState>,
    workspace_id: String,
    message_id: String,
) -> Result<u64, String> {
    let db = state.db().clone();
    run_dispatcher_db("dispatcher_truncate_messages_from", move || {
        db.truncate_messages_from(&workspace_id, &message_id)
    })
    .await
}

#[tauri::command]
pub async fn dispatcher_get_tool_artifact(
    state: tauri::State<'_, DispatcherState>,
    workspace_id: String,
    artifact_id: String,
) -> Result<DispatcherToolArtifactRecord, String> {
    let db = state.db().clone();
    run_dispatcher_db("dispatcher_get_tool_artifact", move || {
        db.get_tool_artifact(&workspace_id, &artifact_id)
    })
    .await
}

#[tauri::command]
pub async fn dispatcher_delete_session(
    state: tauri::State<'_, DispatcherState>,
    workspace_id: String,
) -> Result<(), String> {
    let db = state.db().clone();
    run_dispatcher_db("dispatcher_delete_session", move || {
        db.delete_session(&workspace_id)
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
    let db = state.db().clone();
    let size = page_size.unwrap_or(30).clamp(1, 100);
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
    let db = state.db().clone();
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
    let db = state.db().clone();
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
    let db = state.db().clone();
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
    let db = state.db().clone();
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
    let db = state.db().clone();
    let off = offset.unwrap_or(0).max(0);
    let size = page_size.unwrap_or(30).clamp(1, 100);
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
) -> Result<ProjectSessionRecord, String> {
    let db = state.db().clone();
    let session = run_dispatcher_db("project_create_session", move || {
        db.create_project_session(&project_id, &title)
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
    let db = state.db().clone();
    run_dispatcher_db("project_delete_session", move || {
        db.delete_project_session(&session_id)
    })
    .await
}

#[tauri::command]
pub async fn session_search_keywords(
    state: tauri::State<'_, DispatcherState>,
    query: String,
    kind: DispatcherSessionKind,
    project_id: Option<String>,
    limit: Option<i64>,
) -> Result<Vec<SessionSearchResult>, String> {
    let db = state.db().clone();
    run_dispatcher_db("session_search_keywords", move || {
        db.search_sessions(&query, kind, project_id.as_deref(), limit.unwrap_or(20))
    })
    .await
}

#[tauri::command]
pub async fn chat_list_categories(
    state: tauri::State<'_, DispatcherState>,
) -> Result<Vec<ChatCategory>, String> {
    let db = state.db().clone();
    run_dispatcher_db("chat_list_categories", move || db.list_chat_categories()).await
}

#[tauri::command]
pub async fn chat_create_category(
    state: tauri::State<'_, DispatcherState>,
    name: String,
    icon: Option<String>,
    color: Option<String>,
    allowed_tools: Option<Vec<String>>,
    system_prompt: Option<String>,
) -> Result<ChatCategory, String> {
    let db = state.db().clone();
    run_dispatcher_db("chat_create_category", move || {
        db.create_chat_category(
            &name,
            icon.as_deref().unwrap_or(""),
            color.as_deref().unwrap_or(""),
            allowed_tools,
            system_prompt.as_deref(),
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
    let db = state.db().clone();
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
    let db = state.db().clone();
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
    let db = state.db().clone();
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
    let db = state.db().clone();
    run_dispatcher_db("chat_reorder_categories", move || {
        db.reorder_chat_categories(&ordered_ids)
    })
    .await
}

#[tauri::command]
pub async fn aha_get_chat_category_agent_configs(
    state: tauri::State<'_, DispatcherState>,
) -> Result<Vec<ChatCategoryAgentConfig>, String> {
    let db = state.db().clone();
    run_dispatcher_db("aha_get_chat_category_agent_configs", move || {
        db.list_chat_category_agent_configs()
    })
    .await
}

#[tauri::command]
pub async fn aha_save_chat_category_agent_configs(
    state: tauri::State<'_, DispatcherState>,
    configs: Vec<ChatCategoryAgentConfig>,
) -> Result<Vec<ChatCategoryAgentConfig>, String> {
    let db = state.db().clone();
    run_dispatcher_db("aha_save_chat_category_agent_configs", move || {
        db.save_chat_category_agent_configs(&configs)
    })
    .await
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
        "review" => test_chat_compatible_model("审查模型", config, false).await,
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
    let messages = build_test_messages(enable_multimodal);
    let response = provider
        .chat_stream(&messages, &[], enable_multimodal, |_| {})
        .await
        .with_context(|| format!("{label} 测试请求失败（模型 {model_name}）"))?;
    let content = response.content.trim().to_string();
    if content.is_empty() {
        anyhow::bail!("{label}（{model_name}）返回空内容");
    }
    if enable_multimodal {
        Ok(format!(
            "{label} ok（{model_name}，含图片多模态调用）：{content}"
        ))
    } else {
        Ok(format!("{label} ok（{model_name}）：{content}"))
    }
}

/// Build the test message list. For vision-capable models the user message
/// embeds a small inline PNG so the multimodal `image_url` path is actually
/// exercised — a text-only model misconfigured as the vision model will then
/// fail here (HTTP 400 / `unknown variant image_url`) instead of silently
/// passing and crashing `browser_visual_analyze` at runtime.
fn build_test_messages(enable_multimodal: bool) -> Vec<ChatMessage> {
    if !enable_multimodal {
        return vec![ChatMessage::system("只输出 pong。".to_string())];
    }
    // 64x64 red PNG. Kept small to minimize request size, but every side
    // exceeds the minimum image dimension enforced by some providers
    // (e.g. Aliyun DashScope rejects images with width/height <= 10px).
    const TEST_PNG_DATA_URL: &str =
        "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAEAAAABACAIAAAAlC+aJAAAAb0lEQVR4nO3PAQkAAAyEwO9feoshgnABdLep8QUNyPEFDcjxBQ3I8QUNyPEFDcjxBQ3I8QUNyPEFDcjxBQ3I8QUNyPEFDcjxBQ3I8QUNyPEFDcjxBQ3I8QUNyPEFDcjxBQ3I8QUNyPEFDcjxBQ3IPanc8OLDQitxAAAAAElFTkSuQmCC";
    vec![
        ChatMessage::system("你是模型连通性测试器，只对图片中的颜色做最简短回答。".to_string()),
        ChatMessage {
            role: "user".to_string(),
            content: "这是一张测试图片，请用一个词描述其中主要的颜色。".to_string(),
            content_parts: vec![
                ChatMessageContentPart::Text {
                    text: "这是一张测试图片，请用一个词描述其中主要的颜色。".to_string(),
                },
                ChatMessageContentPart::Image {
                    source: ChatMessageImageSource::DataUrl {
                        data_url: TEST_PNG_DATA_URL.to_string(),
                    },
                },
            ],
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        },
    ]
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
        anyhow::bail!("{label}（{model_name}）端点返回 HTTP {status}，请求地址：{url}，响应：{body}");
    }
    if status.is_client_error() {
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!(
            "{label}（{model_name}）端点返回 HTTP {status}（请检查 API Key 和 URL），请求地址：{url}，响应：{body}"
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
pub async fn aha_get_settings_v2(
    state: tauri::State<'_, DispatcherState>,
) -> Result<AhaSettingsV2, String> {
    state
        .db()
        .get_settings_v2()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn aha_save_settings_v2(
    state: tauri::State<'_, DispatcherState>,
    settings: AhaSettingsV2,
) -> Result<AhaSettingsV2, String> {
    state
        .db()
        .save_settings_v2(&settings)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn aha_set_active_chat_model(
    state: tauri::State<'_, DispatcherState>,
    model_index: usize,
) -> Result<Vec<DispatcherModelConfig>, String> {
    let mut settings = state
        .db()
        .get_settings_v2()
        .map_err(|error| error.to_string())?;

    if model_index >= settings.chat.chat_model_configs.len() {
        return Err(format!("聊天主模型索引越界：{model_index}"));
    }

    for (index, config) in settings.chat.chat_model_configs.iter_mut().enumerate() {
        config.active = index == model_index;
    }

    let saved = state
        .db()
        .save_settings_v2(&settings)
        .map_err(|error| error.to_string())?;
    Ok(saved.chat.chat_model_configs)
}

#[tauri::command]
pub async fn aha_get_context_config(
    state: tauri::State<'_, DispatcherState>,
    context: String,
) -> Result<AhaContextConfig, String> {
    let ctx = AgentContext::from_wire(&context).map_err(|e| e.to_string())?;
    state
        .db()
        .get_settings_for_context(ctx)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn aha_list_agent_tools(
    state: tauri::State<'_, DispatcherState>,
    context: String,
    project_path: Option<String>,
) -> Result<Vec<ToolInfo>, String> {
    let ctx = AgentContext::from_wire(&context).map_err(|e| e.to_string())?;
    state.list_agent_tools(ctx, project_path).await
}

#[tauri::command]
pub async fn aha_resolve_ssh_workspace(
    state: tauri::State<'_, DispatcherState>,
    context: String,
    project_path: Option<String>,
) -> Result<String, String> {
    let ctx = AgentContext::from_wire(&context).map_err(|e| e.to_string())?;
    state
        .ssh_workspace_for_context(ctx, project_path)
        .await
        .map(|path| path.to_string_lossy().into_owned())
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
    on_event: tauri::ipc::Channel<AgentEvent>,
) -> Result<AgentTurn, String> {
    let run_handle = state.begin_run(&workspace_id)?;
    let agent = state.build_run_agent().with_app_handle(app);
    let result = agent
        .continue_after_dispatch(DispatcherContinueAfterDispatchRequest {
            db: state.db(),
            workspace_id: &workspace_id,
            workspace_path: &project_path,
            dispatch_result: &dispatch_result,
            dispatch_state: DispatchFeedbackState::from_wire(&dispatch_state),
            dispatch_id: dispatch_id.as_deref(),
            on_event,
            cancel_rx: run_handle.cancel_rx,
        })
        .await
        .map_err(|error| error.to_string());
    state.finish_run(&workspace_id);
    result
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
