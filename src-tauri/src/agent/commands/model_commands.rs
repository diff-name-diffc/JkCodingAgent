use super::*;

#[tauri::command]
pub async fn dispatcher_fetch_models(
    api_base: String,
    api_key: String,
) -> Result<Vec<String>, String> {
    llm::fetch_models(&api_base, &api_key)
        .await
        .map_err(format_anyhow_chain)
}

#[tauri::command]
pub async fn dispatcher_test_model(
    kind: String,
    config: DispatcherModelConfig,
) -> Result<String, String> {
    test_dispatcher_model(&kind, config)
        .await
        .map_err(format_anyhow_chain)
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
        anyhow::bail!(
            "{label}（{model_name}）端点返回 HTTP {status}，请求地址：{url}，响应：{body}"
        );
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

#[cfg(test)]
mod tests {
    use super::{embedding_endpoint, latest_qa_pair};
    use crate::agent::db::content::ContentSegment;
    use crate::agent::db::DispatcherMessageRecord;

    fn message(role: &str, content: &str) -> DispatcherMessageRecord {
        let segments_json = serde_json::to_string(&vec![ContentSegment::Text {
            id: "text-segment".to_string(),
            text: content.to_string(),
        }])
        .unwrap();
        DispatcherMessageRecord {
            id: String::new(),
            workspace_id: String::new(),
            role: role.to_string(),
            segments_json,
            thinking_content: None,
            thinking_elapsed_ms: None,
            context_payload: None,
            tool_call_id: None,
            tool_name: None,
            tool_result_mode: None,
            tool_artifacts: Vec::new(),
            tool_calls_json: None,
            usage_stats: None,
            created_at: String::new(),
        }
    }

    #[test]
    fn qa_pair_last_assistant_matches_nearest_preceding_user() {
        let messages = vec![
            message("user", "旧问题"),
            message("assistant", "旧回答"),
            message("user", "新问题"),
            message("assistant", "新回答"),
        ];
        let (user, assistant) = latest_qa_pair(&messages);
        assert_eq!(user.unwrap().plain_text(), "新问题");
        assert_eq!(assistant.unwrap().plain_text(), "新回答");
    }

    #[test]
    fn qa_pair_last_user_only_uses_last_user_content() {
        // 最后一轮只有用户消息（运行中断/失败）：不得与上一轮旧助手回复配对。
        let messages = vec![
            message("user", "旧问题"),
            message("assistant", "旧回答"),
            message("user", "新问题"),
        ];
        let (user, assistant) = latest_qa_pair(&messages);
        assert_eq!(user.unwrap().plain_text(), "新问题");
        assert!(assistant.is_none());
    }

    #[test]
    fn qa_pair_consecutive_user_messages_pick_latest() {
        // 两条连续 user 消息：取最后一条而不是更旧的问题。
        let messages = vec![message("user", "更旧的问题"), message("user", "最新的问题")];
        let (user, assistant) = latest_qa_pair(&messages);
        assert_eq!(user.unwrap().plain_text(), "最新的问题");
        assert!(assistant.is_none());
    }

    #[test]
    fn qa_pair_empty_or_non_dialogue_messages_yield_none() {
        let (user, assistant) = latest_qa_pair(&[]);
        assert!(user.is_none() && assistant.is_none());

        let messages = vec![message("tool", "工具输出")];
        let (user, assistant) = latest_qa_pair(&messages);
        assert!(user.is_none() && assistant.is_none());
    }

    #[test]
    fn embedding_endpoint_appends_suffix() {
        assert_eq!(
            embedding_endpoint("https://api.example.com"),
            "https://api.example.com/v1/embeddings"
        );
        assert_eq!(
            embedding_endpoint("https://api.example.com/v1/"),
            "https://api.example.com/v1/embeddings"
        );
        assert_eq!(
            embedding_endpoint("https://api.example.com/embeddings"),
            "https://api.example.com/embeddings"
        );
    }
}
