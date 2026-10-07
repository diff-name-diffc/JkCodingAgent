use super::*;
use crate::agent::rig_ext::model::{build_completion_request, completions_model, PurposeModelSpec};
use crate::agent::rig_ext::tools::deps::ToolTimeoutDefaults;
use crate::agent::rig_ext::tools::media::image_api;
use rig::completion::CompletionModel;

#[tauri::command]
pub async fn dispatcher_fetch_models(
    api_base: String,
    api_key: String,
) -> Result<Vec<String>, String> {
    crate::agent::rig_ext::models::fetch_models(&api_base, &api_key)
        .await
        .map_err(|error| format_anyhow_error(&error))
}

#[tauri::command]
pub async fn dispatcher_test_model(
    state: tauri::State<'_, DispatcherState>,
    kind: String,
    config: DispatcherModelConfig,
) -> Result<String, String> {
    // 图片类测试的 HTTP 预算与运行时同源（spec::effective_timeout_secs）：
    // 用户为慢模型调大默认超时后，测试不再按表默认提前掐死。设置读取失败
    // 直接报错（设置库不可读时掩饰成表默认会让误报无从排查）。
    let tool_timeouts = state
        .db()
        .get_settings_v2()
        .map(|settings| ToolTimeoutDefaults::from(&settings.tool_timeouts))
        .map_err(|error| error.to_string())?;
    test_dispatcher_model(&kind, config, &tool_timeouts)
        .await
        .map_err(|error| format_anyhow_error(&error))
}

async fn test_dispatcher_model(
    kind: &str,
    config: DispatcherModelConfig,
    tool_timeouts: &ToolTimeoutDefaults,
) -> Result<String> {
    match kind {
        "chat" => test_chat_compatible_model("聊天主模型", config, false).await,
        "summary" => test_chat_compatible_model("摘要模型", config, false).await,
        "review" => test_chat_compatible_model("审查模型", config, false).await,
        "vision" => test_chat_compatible_model("视觉模型", config, true).await,
        "embedding" => test_embedding_model(config).await,
        "asr" => test_required_model_config("ASR 模型", &config)
            .map(|_| "ASR 配置字段完整，未启动真实录音会话。".to_string()),
        "image" => {
            test_dashscope_image_model("图片模型", config, false, tool_timeouts.generate_image)
                .await
        }
        "imageEdit" => {
            test_dashscope_image_model("图片编辑模型", config, true, tool_timeouts.edit_image).await
        }
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
    let spec = PurposeModelSpec {
        api_key: config.api_key,
        api_base: config.url,
        model: config.model,
        // 连通性测试只要一个词的答案：给足小预算但保留上限保护。
        max_tokens: Some(64),
        context_window: None,
        temperature: 0.0,
        // 测试只验证链路可用：关闭思考链，避免「只想说 pong」的任务被思考耗尽预算。
        enable_thinking: false,
    };
    let model = completions_model(&spec)
        .with_context(|| format!("{label} 测试模型初始化失败（模型 {model_name}）"))?;
    let (preamble, messages) = build_test_messages(enable_multimodal);
    let request = build_completion_request(
        preamble,
        messages,
        Vec::new(),
        spec.max_tokens,
        spec.temperature,
        spec.enable_thinking,
    );
    let response = model
        .completion(request)
        .await
        .with_context(|| format!("{label} 测试请求失败（模型 {model_name}）"))?;
    let content = response
        .choice
        .iter()
        .filter_map(|item| match item {
            rig::message::AssistantContent::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
        .trim()
        .to_string();
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
fn build_test_messages(enable_multimodal: bool) -> (Option<String>, Vec<rig::completion::Message>) {
    if !enable_multimodal {
        return (
            Some("只输出 pong。".to_string()),
            vec![rig::completion::Message::user("ping".to_string())],
        );
    }
    (
        Some("你是模型连通性测试器，只对图片中的颜色做最简短回答。".to_string()),
        vec![rig::completion::Message::User {
            content: vec![
                rig::message::UserContent::text("这是一张测试图片，请用一个词描述其中主要的颜色。"),
                rig::message::UserContent::Image(rig::message::Image {
                    data: rig::message::DocumentSourceKind::Url(TEST_PNG_DATA_URL.to_string()),
                    media_type: Some(rig::message::ImageMediaType::PNG),
                    detail: None,
                    additional_params: None,
                }),
            ],
        }],
    )
}

/// 64x64 red PNG (data URL). Kept small to minimize request size, but every
/// side exceeds the minimum image dimension enforced by some providers (e.g.
/// Aliyun DashScope rejects images with width/height <= 10px).
const TEST_PNG_DATA_URL: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAEAAAABACAIAAAAlC+aJAAAAb0lEQVR4nO3PAQkAAAyEwO9feoshgnABdLep8QUNyPEFDcjxBQ3I8QUNyPEFDcjxBQ3I8QUNyPEFDcjxBQ3I8QUNyPEFDcjxBQ3I8QUNyPEFDcjxBQ3I8QUNyPEFDcjxBQ3I8QUNyPEFDcjxBQ3I8QUNyPEFDcjxBQ3I8QUNyIPanc8OLDQitxAAAAAElFTkSuQmCC";

/// 512x512 red PNG (data URL): 图片编辑模型测试的输入图。编辑类模型常要求
/// 输入分辨率 ≥256px（高于视觉问答的 10px 下限），64px 会被拒参数。
const EDIT_TEST_PNG_DATA_URL: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAgAAAAIACAIAAAB7GkOtAAAFl0lEQVR42u3VMQ0AAAjAsPk3DR54aVIFe9YUAA9JAGAAABgAAAYAgAEAYAAAGAAABgCAAQBgAAAYAAAGAIABAGAAABgAAAYAgAEAYAAAGAAABgCAAQBgAAAYAAAGAGAAABgAAAYAgAEAYAAAGAAABgCAAQBgAAAYAAAGAIABAGAAABgAAAYAgAEAYAAAGAAABgCAAQBgAAAYAAAGAGAAEgAYAAAGAIABAGAAABgAAAYAgAEAYAAAGAAABgCAAQBgAAAYAAAGAIABAGAAABgAAAYAgAEAYAAAGAAABgCAAQAYAAAGAIABAGAAABgAAAYAgAEAYAAAGAAABgCAAQBgAAAYAAAGAIABAGAAABgAAAYAgAEAYAAAGAAABgCAAQBgAAAYAAAGAIABAGAAABgAAAYAgAEAYAAAGAAABgCAAQBgAAAGAIABAGAAABgAAAYAgAEAYAAAGAAABgCAAQBgAAAYAAAGAIABAGAAABgAAAYAAASABgAAAYAgAEAYAAAGAAABgCAAQBgAAAYAAAGAIABAGAAABgAAAYAgAEAYAAAGAAABgCAAQBgAAAYAAAGAIABABgAAAYAgAEAYAAAGAAABgCAAQBgAAAYAAAGAIABAGAAABgAAAYAgAEAYAAAGAAABgCAAQBgAAAYAAAGAIABACABgAEAYAAAGAAABgCAAQBgAAAYAAAGAIABAGAAABgAAAYAgAEAYAAAGAAABgCAAQBgAAAYAAAGAIABAGAAABgAgAEAYAAAGAAABgCAAQBgAAAYAAAGAIABAGAAABgAAAYAgAEAYAAAGAAABgCAAQBgAAAYAAAGAIABAGAAABgAgAFIAGAAABgAAAYAgAEAYAAAGAAABgCAAQBgAAAYAAAGAIABAGAAABgAAAYAgAEAYAAAGAAABgCAAQBgAAAYAAAGAGAAABgAAAYAgAEAYAAAGAAABgCAAQBgAAAYAAAGAIABAGAAABgAAAYAgAEAYAAAGAAABgCAAQBgAAAYAAAGAGAAABgAAAYAgAEAYAAAGAAABgCAAQBgAAAYAAAGAIABAGAAABgAAAYAgAEAYAAAGAAABgCAAQBgAAAYAAAGAIABABgAAAYAgAEAYAAAGAAABgCAAQBgAAAYAAAGAIABAGAAABgAAAYAgAEAYAAAGAAABgCAAQBgAAAYAAAGAIABABgAAAYAgAEAYAAAGAAABgCAAQBgAAAYAAAGAIABAGAAABgAAAYAgAEAYAAAGAAABgCAAQBgAAAYAAAGAIABAGAAAAYAgAEAYAAAGAAABgCAAQBgAAAYAAAGAIABAGAAABgAAAYAgAEAYAAAGAAABgCAAQBgAAAYAAAGAIABAGAAAAYAgAEAYAAAGAAABgCAAQBgAAAYAAAGAIABAGAAABgAAAYAgAEAYAAAGAAABgCAAQBgAAAYAAAGAIABAGAAAEgAYAAAGAAABgCAAQBgAAAYAAAGAIABAGAAABgAAAYAgAEAYAAAGAAABgCAAQBgAAAYAAAGAIABAGAAABgAAAYAYAAAGAAABgCAAQBgAAAYAAAGAIABAGAAABgAAAYAgAEAYAAAGAAABgCAAQBgAAAYAABHCwofO8XPBui2AAAAAElFTkSuQmCC";

/// 按字节上限截断响应预览，回退到最近字符边界：服务端错误响应常含多字节
/// UTF-8（中文报错说明），固定字节切片落在字符中间会直接 panic，把最需要
/// 可读诊断的失败路径变成崩溃。
fn preview_utf8(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// DashScope 多模态生成端点真实调用测试：与 generate_image / edit_image
/// 工具完全同构（POST `{base}/api/v1/services/aigc/multimodal-generation/
/// generation` + Bearer），`with_image` 为 true 时走编辑形态（内嵌测试图）。
/// 原实现对配置 URL 原样发 GET，而该端点只接受 POST，网关必返 404，测试
/// 结果与真实链路无关。成功判据为响应携带生成图片 URL（真实跑通一次
/// 生成，会产生一次模型计费），不下载产物。
async fn test_dashscope_image_model(
    label: &str,
    config: DispatcherModelConfig,
    with_image: bool,
    user_default: Option<u64>,
) -> Result<String> {
    test_required_model_config(label, &config)?;
    let model_name = config.model.trim().to_string();
    let endpoint = image_api::multimodal_generation_endpoint(&config.url);
    let content = if with_image {
        vec![
            serde_json::json!({ "image": EDIT_TEST_PNG_DATA_URL }),
            serde_json::json!({ "text": "连通性测试：输出一张纯色图片即可。" }),
        ]
    } else {
        vec![serde_json::json!({ "text": "连通性测试：输出一张纯色背景的极简小图。" })]
    };
    // 生成形态与用户侧 curl 请求体一致（仅 prompt_extend）；编辑形态与
    // 运行时 edit_image 的参数面一致（n=1 + size；运行时未指定尺寸时恒发
    // "1024*1024"，并非交给服务端缺省）。
    let parameters = if with_image {
        serde_json::json!({ "n": 1, "size": "1024*1024" })
    } else {
        serde_json::json!({ "prompt_extend": true })
    };
    let request_body = serde_json::json!({
        "model": model_name,
        "input": { "messages": [ { "role": "user", "content": content } ] },
        "parameters": parameters
    });
    // HTTP 预算与运行时同源（spec::effective_timeout_secs）：未声明 → 用户
    // 配置默认 → 表默认。用户为慢模型调大默认时，测试不再按表默认提前掐死。
    let tool = if with_image {
        "edit_image"
    } else {
        "generate_image"
    };
    let timeout_secs =
        crate::agent::rig_ext::tools::spec::effective_timeout_secs(tool, None, user_default);
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .build()
        .context("构建 HTTP 客户端失败")?;
    let response = client
        .post(&endpoint)
        .bearer_auth(config.api_key.trim())
        .json(&request_body)
        .send()
        .await
        .with_context(|| format!("{label} 测试请求失败（模型 {model_name}，端点 {endpoint}）"))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .with_context(|| format!("{label}（{model_name}）读取响应体失败（HTTP {status}）"))?;
    if !status.is_success() {
        anyhow::bail!(
            "{label}（{model_name}）测试失败，HTTP {status}，请求地址：{endpoint}，响应：{body}"
        );
    }
    let value: Value = serde_json::from_str(&body).with_context(|| {
        format!(
            "{label}（{model_name}）响应解析失败，响应内容：{}",
            preview_utf8(&body, 500)
        )
    })?;
    if !value["output"]["choices"][0]["message"]["content"][0]["image"].is_string() {
        anyhow::bail!(
            "{label}（{model_name}）响应中未找到生成图片 URL，响应结构：{}",
            preview_utf8(&body, 300)
        );
    }
    let (width, height) = (
        value["usage"]["width"].as_u64(),
        value["usage"]["height"].as_u64(),
    );
    match (width, height) {
        (Some(width), Some(height)) => Ok(format!(
            "{label} ok（{model_name}），已生成测试图（{width}x{height}）"
        )),
        _ => Ok(format!("{label} ok（{model_name}），已生成测试图")),
    }
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
    let body = response
        .text()
        .await
        .with_context(|| format!("文本向量模型（{model_name}）读取响应体失败（HTTP {status}）"))?;
    if !status.is_success() {
        anyhow::bail!("文本向量模型（{model_name}）测试失败，HTTP {status}：{body}");
    }
    let value: Value = serde_json::from_str(&body).with_context(|| {
        format!(
            "文本向量模型（{model_name}）响应解析失败，响应内容：{}",
            preview_utf8(&body, 500)
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
            let preview = preview_utf8(&body, 300);
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
        let body = response.text().await.with_context(|| {
            format!("{label}（{model_name}）读取错误响应体失败（HTTP {status}）")
        })?;
        anyhow::bail!(
            "{label}（{model_name}）端点返回 HTTP {status}，请求地址：{url}，响应：{body}"
        );
    }
    if status.is_client_error() {
        let body = response.text().await.with_context(|| {
            format!("{label}（{model_name}）读取错误响应体失败（HTTP {status}）")
        })?;
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
            tool_task_id: None,
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
