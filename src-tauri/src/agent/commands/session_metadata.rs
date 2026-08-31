use super::*;

const SESSION_TITLE_RECENT_DIALOGUES: usize = 3;

pub(super) fn spawn_session_title_update(
    state: &DispatcherState,
    app: &AppHandle,
    workspace_id: &str,
    segments_json: &str,
    context: AgentContext,
    generation: GenerationGuard,
) {
    let app = app.clone();
    let db = state.db().clone();
    let workspace_id = workspace_id.to_string();
    let segments_json = segments_json.to_string();

    tokio::spawn(async move {
        let title =
            generate_session_title(db.clone(), workspace_id.clone(), segments_json, context).await;
        // G11-13：代际守卫在统一锁内校验「最新代胜出」；即使本任务提前
        // return/abort，守卫 Drop 也会结算 active 条目，不会泄漏。
        if !generation.finish() {
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

pub(super) fn spawn_session_keywords_update(
    state: &DispatcherState,
    app: &AppHandle,
    workspace_id: &str,
    context: AgentContext,
    generation: GenerationGuard,
) {
    let app = app.clone();
    let db = state.db().clone();
    let workspace_id = workspace_id.to_string();

    tokio::spawn(async move {
        let actions = generate_session_keywords(db.clone(), workspace_id.clone(), context).await;
        // G11-13：代际守卫在统一锁内校验「最新代胜出」；Drop 兜底清理。
        if !generation.finish() {
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
                                "sessionId": workspace_id,
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

/// 关键字提取的问答配对（G11-14）：以最后一条消息的角色决定配对——
/// 仅当最后一条是 assistant 时才向前查找最近的 user，组成同一轮问答；
/// 最后一条是 user（运行中断/失败等场景）时只使用最后一条 user 内容，
/// 避免新问题与上一轮旧助手回复错配、或两条连续 user 消息取到更旧的问题。
pub(super) fn latest_qa_pair(
    messages: &[DispatcherMessageRecord],
) -> (
    Option<&DispatcherMessageRecord>,
    Option<&DispatcherMessageRecord>,
) {
    let Some(last) = messages.last() else {
        return (None, None);
    };
    match last.role.as_str() {
        "assistant" => {
            let user = messages[..messages.len() - 1]
                .iter()
                .rev()
                .find(|message| message.role == "user");
            (user, Some(last))
        }
        "user" => (Some(last), None),
        _ => (None, None),
    }
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

    let (user, assistant) = latest_qa_pair(&messages);
    if user.is_none() && assistant.is_none() {
        return None;
    }
    let qa_text = {
        let mut s = String::new();
        if let Some(u) = user {
            s.push_str("【用户】\n");
            s.push_str(&u.plain_text());
            s.push('\n');
        }
        if let Some(a) = assistant {
            s.push_str("\n【助手】\n");
            let content = a.plain_text();
            let text = if content.len() > 2000 {
                // Find the nearest char boundary at or before byte 2000
                let boundary = content.floor_char_boundary(2000);
                format!("{}...", &content[..boundary])
            } else {
                content
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
        .find_map(|message| {
            (message.role == "user")
                .then(|| message.plain_text())
                .filter(|text| !text.trim().is_empty())
        })
        .unwrap_or_else(|| current_user_content.clone());
    let fallback = fallback_session_title(&fallback_source);
    let title_messages = title_messages
        .into_iter()
        .map(|message| {
            let content = message.plain_text();
            SessionTitleMessage {
                role: message.role,
                content,
            }
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

    // 凭据回退：摘要槽位 api_key 或 url 任一缺失时，整组回退到同一 context 的
    // 主对话模型槽位（chat_model_configs 的 active 条目，已由模型库回填凭据）。
    // 不做逐字段回退：「摘要的 key + 对话的 url」会把 A 厂商的凭据发往 B 厂商
    // 端点，产生晦涩的鉴权失败。
    let chat_fallback = context_config
        .chat_model_configs
        .iter()
        .find(|item| item.active)
        .or_else(|| context_config.chat_model_configs.first());
    let summary_complete = !summary.api_key.trim().is_empty() && !summary.url.trim().is_empty();
    let (api_key, url) = if summary_complete {
        (
            summary.api_key.trim().to_string(),
            summary.url.trim().to_string(),
        )
    } else {
        (
            chat_fallback
                .map(|item| item.api_key.trim().to_string())
                .unwrap_or_default(),
            chat_fallback
                .map(|item| item.url.trim().to_string())
                .unwrap_or_default(),
        )
    };

    Ok((
        OpenAiCompatProvider::new(
            api_key,
            url,
            summary_model.to_string(),
            // 关键字摘要输出 JSON 数组（最多 15 项）需要较大预算；也兼容仍会思考的摘要
            // 模型（思考 token 计入上限）。非思考模型输出完即停，此处仅作上限保护。
            2048,
            // 摘要是低创造性任务，固定低温度（沿用历史 config.temperature 默认 0.1）。
            0.1,
        ),
        summary_model.to_string(),
    ))
}
