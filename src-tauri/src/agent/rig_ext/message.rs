//! 消息桥：会话历史 → rig `Message`。
//!
//! 入口是 `chat_history_to_rig`——DB 侧 `load_llm_history_async` 已完成记录
//! 解析（segments_json / context_payload / tool_calls_json / thinking 过滤），
//! 本模块负责 `ChatMessage` → rig `Message` 的契约转换与图片解析。
//!
//! 图片段（`chat-image://{image_id}`）在此读盘转 base64 的
//! `UserContent::Image`；丢失/超限按旧语义降级为文本占位，绝不中断 run。
//! image_id 暂存在 `Image.additional_params["chatImageId"]`（纯内存标记，
//! openai 线格式转换忽略该字段），供 `attach_turn_tool_images` 跨迭代去重。

use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context, Result};
use base64::Engine;
use rig::completion::Message;
use rig::message::{
    AdditionalParams, AssistantContent, DocumentSourceKind, Image, ImageMediaType, ProviderCallId,
    Text, ToolCall, ToolCallId, ToolFunction, ToolResult, ToolResultContent, UserContent,
};

use crate::agent::db::{
    ChatMessage, ChatMessageContentPart, ChatMessageImageSource, DispatcherDb,
    MAX_TURN_TOOL_IMAGE_ATTACHMENTS,
};

/// `Image.additional_params` 中暂存 image_id 的键（见模块文档）。
const CHAT_IMAGE_ID_PARAM: &str = "chatImageId";

/// 与旧客户端层一致的内联图片体积上限（20 MB）。
const MAX_INLINE_IMAGE_BYTES: u64 = 20 * 1024 * 1024;

/// DB 历史（`DispatcherDb::load_llm_history_async` 的产物）→ rig 消息序列。
/// 运行时循环的上下文起点（记录解析与过滤已在 DB 侧完成）。
pub async fn chat_history_to_rig(history: Vec<ChatMessage>) -> Vec<Message> {
    chat_history_to_rig_with_ids(history).await.0
}

/// 与 `chat_history_to_rig` 相同，并带回每条 rig 消息对应的落库 id。
/// 空 assistant 被丢弃时，id 一并丢掉。摘要和配对占位没有 id。
pub async fn chat_history_to_rig_with_ids(
    history: Vec<ChatMessage>,
) -> (Vec<Message>, Vec<Option<String>>) {
    let mut messages = Vec::with_capacity(history.len());
    let mut ids = Vec::with_capacity(history.len());
    for message in history {
        let id = message.source_id.clone();
        if let Some(message) = chat_message_to_rig(message).await {
            messages.push(message);
            ids.push(id);
        }
    }
    (messages, ids)
}

/// 锚点及更早的消息已写进滚动摘要，装配时不再把原文送进模型。
/// 锚点不在本次窗口里（已经滑出最近 5 轮）时原样保留。
pub(crate) fn omit_messages_through_anchor(history: &mut Vec<ChatMessage>, anchor_id: &str) {
    let Some(index) = history
        .iter()
        .position(|message| message.source_id.as_deref() == Some(anchor_id))
    else {
        return;
    };
    history.drain(..=index);
}

/// 装配运行起点的历史视图：在 DB 窗口历史前插入已持久化的滚动摘要
/// （`dispatcher_session_summaries`，阶段 2 历史级压缩的跨 run 延续）。
/// 锚点消息已被截断/删除时摘要失效——DB 侧读取即删，不回插脏摘要。
/// 锚点仍在窗口内时，锚点及更早的原文删掉，避免摘要和原文叠在一起。
/// 摘要以普通 user 文本消息插入（`HISTORY_SUMMARY_MARKER` 前缀），
/// 进入运行循环 `compact_history` 的滚动合并链路。
pub async fn apply_stored_session_summary(
    db: &DispatcherDb,
    workspace_id: &str,
    mut history: Vec<ChatMessage>,
) -> Result<Vec<ChatMessage>> {
    if let Some(summary) = db.valid_session_summary_async(workspace_id).await? {
        omit_messages_through_anchor(&mut history, &summary.covered_through_message_id);
        history.insert(0, ChatMessage::user(summary.summary));
    }
    Ok(history)
}

/// `ChatMessage` → rig `Message` 的契约转换。
///
/// 角色映射：system → `System`；user → `User`（多模态 parts）；
/// assistant → `Assistant`（正文 → `Text`、tool_calls → `ToolCall`，
/// reasoning 不回灌、空消息丢弃）；tool → `User` 内的
/// `ToolResult`（openai 线格式即 role=tool 消息）。
async fn chat_message_to_rig(message: ChatMessage) -> Option<Message> {
    match message.role.as_str() {
        "system" => Some(Message::System {
            content: message.content,
        }),
        "user" => user_message_to_rig(&message).await,
        "runtime" => Some(Message::user(message.content)),
        "assistant" => {
            let mut content: Vec<AssistantContent> = Vec::new();
            // 历史思考链不回灌（reasoning_content 只用于 UI 展示）：rig 的
            // openai 线格式会把 Reasoning 序列化进请求体，DeepSeek 等服务商
            // 明确要求历史不携带 reasoning_content；回灌只浪费上下文预算。
            if !message.content.is_empty() {
                content.push(AssistantContent::Text(Text::new(message.content)));
            }
            for call in message.tool_calls.unwrap_or_default() {
                // 旧 DB 契约里 arguments 是 JSON 字符串；rig 侧为 Value。
                // 解析失败保留原始字符串（Value::String），不因脏数据丢消息。
                let arguments = serde_json::from_str(&call.function.arguments)
                    .unwrap_or(serde_json::Value::String(call.function.arguments.clone()));
                content.push(AssistantContent::ToolCall(ToolCall::from_wire(
                    call.id,
                    ToolFunction::new(call.function.name, arguments),
                )));
            }
            // 全空 assistant（无正文/思考/工具调用）对模型无信息量，且 rig 请求
            // 校验拒绝空 content 列表——直接丢弃。
            (!content.is_empty()).then_some(Message::Assistant { id: None, content })
        }
        "tool" => {
            let provider = message.tool_call_id.and_then(ProviderCallId::new);
            Some(Message::User {
                content: vec![UserContent::ToolResult(ToolResult {
                    call: ToolCallId::for_provider(provider.as_ref()),
                    provider,
                    name: message.name.unwrap_or_default(),
                    content: vec![ToolResultContent::text(message.content)],
                })],
            })
        }
        other => {
            eprintln!("chat_message_to_rig: 未知角色 {other}，消息已跳过");
            None
        }
    }
}

/// user 消息转换：对齐旧 `build_api_message_content` 语义——无图片时整体
/// 作为单文本；有图片（含丢失占位）时按 parts 组装，且当所有文本 part 均为
/// 空白而 content 非空时把 content 补为首个文本 part（避免用户指令被丢弃）。
async fn user_message_to_rig(message: &ChatMessage) -> Option<Message> {
    if message.content_parts.is_empty() {
        return Some(Message::User {
            content: vec![UserContent::text(message.content.clone())],
        });
    }

    let mut contents: Vec<UserContent> = Vec::new();
    let mut has_image = false;
    let mut has_source_text = false;

    for part in &message.content_parts {
        match part {
            ChatMessageContentPart::Text { text } => {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    has_source_text = true;
                    contents.push(UserContent::text(trimmed));
                }
            }
            ChatMessageContentPart::Image { source } => {
                has_image = true;
                match resolve_image_source(source).await {
                    Ok(image) => contents.push(UserContent::Image(image)),
                    Err(reason) => {
                        contents.push(UserContent::text(format!("[图片已丢失：{reason}，已跳过]")))
                    }
                }
            }
        }
    }

    if has_image && !has_source_text && !message.content.trim().is_empty() {
        contents.insert(0, UserContent::text(message.content.trim().to_string()));
    }
    if contents.is_empty() {
        // parts 全部为空（如纯空白文本段）：回退整体 content，不产生空 user 消息。
        contents.push(UserContent::text(message.content.clone()));
    }
    Some(Message::User { content: contents })
}

/// 单个图片源 → rig `Image`（base64 + media_type + image_id 暂存）。
/// Err 为面向模型的丢失原因文本（调用方包装为占位文本）。
async fn resolve_image_source(
    source: &ChatMessageImageSource,
) -> std::result::Result<Image, String> {
    match source {
        ChatMessageImageSource::DataUrl { data_url } => data_url_to_image(data_url),
        ChatMessageImageSource::ChatImage { image_id } => {
            let reference = format!("chat-image://{image_id}");
            let resolved = crate::chat_images::resolve_chat_image_id_async(image_id.clone())
                .await
                .map_err(|error| format!("{reference}（{error}）"))?;
            let id_for_stash = image_id.clone();
            match tokio::task::spawn_blocking(move || local_image_to_base64(&resolved)).await {
                Ok(Ok((data, media_type))) => Ok(chat_image(data, media_type, &id_for_stash)),
                Ok(Err(error)) => Err(format!("{reference}（{error:#}）")),
                Err(error) => Err(format!("{reference}（后台读取任务失败：{error}）")),
            }
        }
    }
}

/// 构造带 image_id 暂存的 `Image`（供 attach 去重；线格式不携带该标记）。
fn chat_image(data: String, media_type: ImageMediaType, image_id: &str) -> Image {
    Image {
        data: DocumentSourceKind::Base64(data),
        media_type: Some(media_type),
        detail: None,
        additional_params: AdditionalParams::from_entries(Some((
            CHAT_IMAGE_ID_PARAM,
            serde_json::Value::String(image_id.to_string()),
        ))),
    }
}

/// `data:image/...;base64,...` → rig `Image`。非 image data URL 视为丢失
/// （与旧 `image_source_for_api` 的 DataUrl 校验文案一致）。
fn data_url_to_image(data_url: &str) -> std::result::Result<Image, String> {
    let Some(rest) = data_url.strip_prefix("data:image/") else {
        return Err("data URL 必须以 data:image/ 开头".to_string());
    };
    let Some((mime, data)) = rest.split_once(";base64,") else {
        return Err("data URL 缺少 ;base64, 数据段".to_string());
    };
    let media_type = image_media_type_for_mime(&format!("image/{mime}"))
        .ok_or_else(|| format!("不支持的图片类型：image/{mime}"))?;
    Ok(Image {
        data: DocumentSourceKind::Base64(data.to_string()),
        media_type: Some(media_type),
        detail: None,
        additional_params: None,
    })
}

/// 读盘 + 体积校验 + base64（对齐旧 `local_image_to_data_url` 的防御：
/// 非文件/超 20MB/未知扩展名一律拒绝）。
fn local_image_to_base64(path: &Path) -> Result<(String, ImageMediaType)> {
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("读取图片元数据失败：{}", path.display()))?;
    if !metadata.is_file() {
        anyhow::bail!("图片路径不是文件：{}", path.display());
    }
    if metadata.len() > MAX_INLINE_IMAGE_BYTES {
        anyhow::bail!(
            "图片文件过大：{}，当前限制为 {} MB",
            path.display(),
            MAX_INLINE_IMAGE_BYTES / 1024 / 1024
        );
    }
    let ext = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    let mime = crate::chat_images::mime_for_ext(ext)
        .ok_or_else(|| anyhow::anyhow!("不支持的图片格式：{}", path.display()))?;
    let media_type = image_media_type_for_mime(mime)
        .ok_or_else(|| anyhow::anyhow!("不支持的图片类型：{mime}"))?;
    let bytes = std::fs::read(path).with_context(|| format!("读取图片失败：{}", path.display()))?;
    Ok((
        base64::engine::general_purpose::STANDARD.encode(bytes),
        media_type,
    ))
}

fn image_media_type_for_mime(mime: &str) -> Option<ImageMediaType> {
    match mime.to_ascii_lowercase().as_str() {
        "image/png" => Some(ImageMediaType::PNG),
        "image/jpeg" => Some(ImageMediaType::JPEG),
        "image/gif" => Some(ImageMediaType::GIF),
        "image/webp" => Some(ImageMediaType::WEBP),
        "image/svg+xml" => Some(ImageMediaType::SVG),
        "image/heic" => Some(ImageMediaType::HEIC),
        "image/heif" => Some(ImageMediaType::HEIF),
        _ => None,
    }
}

/// 从 rig `Image` 读回暂存的 image_id（见 `CHAT_IMAGE_ID_PARAM`）。
fn attached_image_id(image: &Image) -> Option<&str> {
    image
        .additional_params
        .as_ref()?
        .get(CHAT_IMAGE_ID_PARAM)?
        .as_str()
}

/// 工具图片说明的稳定标记。锚点上这一段会整段换掉，避免多轮工具各自追加。
const TOOL_IMAGE_NOTE_MARK: &str = "张图片由本轮工具调用";

/// 把「本轮用户消息之后的 assistant/tool 文本里引用的
/// `chat-image://{image_id}`」就地附加进该用户消息（内存视图直接持有
/// base64，后续迭代零磁盘重读/零重编码）。锚点上最多保留 3 张、越新的引用优先；
/// 已经解码过的图片直接复用。丢失图片按占位文案降级，绝不中断 run。
///
/// 锚点是最后一条含非工具结果内容的 user 消息。工具结果在 rig 里也是
/// `Message::User`，若把锚点取成「任意最后一条 User」，刚落库的工具结果
/// 会把扫描区间挤空，`generate_image` / `fetch_image` 的引用就进不了视觉输入。
pub async fn attach_turn_tool_images(messages: &mut Vec<Message>) {
    let anchor = last_turn_anchor_index(messages);
    attach_turn_tool_images_at(messages, anchor).await;
}

/// 运行时 observation 同样映射为 User，因此调用方应传入真实请求的来源锚点。
pub(crate) async fn attach_turn_tool_images_at(messages: &mut Vec<Message>, anchor: Option<usize>) {
    let Some(last_user_index) = anchor else {
        return;
    };
    let Message::User { content } = &messages[last_user_index] else {
        return;
    };
    let prefix_len = tool_image_suffix_start(content).unwrap_or(content.len());
    let wanted = collect_turn_tool_image_ids(messages, last_user_index, prefix_len);
    let current = resident_tool_image_ids(content);
    if wanted == current {
        return;
    }

    let Message::User { content } = &mut messages[last_user_index] else {
        return;
    };
    let (prefix, mut resident) = split_tool_image_suffix(std::mem::take(content));
    let mut rebuilt = prefix;
    if !wanted.is_empty() {
        rebuilt.push(UserContent::text(format!(
            "[以下 {} 张图片由本轮工具调用（fetch_image / generate_image / edit_image 等）产生的 \
             chat-image:// 引用附加为视觉输入]",
            wanted.len()
        )));
        for image_id in wanted {
            if let Some(index) = resident.iter().position(|(id, _)| id == &image_id) {
                let (_, image) = resident.swap_remove(index);
                rebuilt.push(UserContent::Image(image));
                continue;
            }
            let source = ChatMessageImageSource::ChatImage {
                image_id: image_id.clone(),
            };
            match resolve_image_source(&source).await {
                Ok(image) => rebuilt.push(UserContent::Image(image)),
                Err(reason) => {
                    rebuilt.push(UserContent::text(format!("[图片已丢失：{reason}，已跳过]")))
                }
            }
        }
    }
    *content = rebuilt;
}

/// 上下文超限重试前卸掉驻留图片。文本预算减半缩不掉 base64；保留
/// `chat-image://` 引用，模型仍知道图在哪，只是这一轮不再看像素。
pub(crate) fn degrade_resident_images(messages: &mut [Message]) {
    for message in messages {
        let Message::User { content } = message else {
            continue;
        };
        for item in content.iter_mut() {
            let UserContent::Image(image) = item else {
                continue;
            };
            let note = match attached_image_id(image) {
                Some(id) => format!("chat-image://{id}（上下文超限，本轮不再附带图像数据）"),
                None => "（上下文超限，本轮不再附带图像数据）".to_string(),
            };
            *item = UserContent::text(note);
        }
    }
}

/// 最后一条「人类/摘要」user 消息的下标。只含 `ToolResult` 的 user 消息是
/// 工具回灌，不能当作本轮锚点。
fn last_turn_anchor_index(messages: &[Message]) -> Option<usize> {
    messages.iter().rposition(|message| {
        let Message::User { content } = message else {
            return false;
        };
        content
            .iter()
            .any(|item| !matches!(item, UserContent::ToolResult(_)))
    })
}

/// 锚点消息里工具图片说明的起点。没有说明时，整段都是用户自己的内容。
fn tool_image_suffix_start(content: &[UserContent]) -> Option<usize> {
    content.iter().position(
        |item| matches!(item, UserContent::Text(text) if text.text.contains(TOOL_IMAGE_NOTE_MARK)),
    )
}

/// 当前锚点后缀里、按附加顺序排列的工具图片 id。
fn resident_tool_image_ids(content: &[UserContent]) -> Vec<String> {
    let Some(start) = tool_image_suffix_start(content) else {
        return Vec::new();
    };
    content[start + 1..]
        .iter()
        .filter_map(|item| match item {
            UserContent::Image(image) => attached_image_id(image).map(str::to_string),
            _ => None,
        })
        .collect()
}

/// 拆掉锚点上的工具图片后缀，返回用户原文和已解码图片（供原样复用）。
fn split_tool_image_suffix(
    mut content: Vec<UserContent>,
) -> (Vec<UserContent>, Vec<(String, Image)>) {
    let Some(start) = tool_image_suffix_start(&content) else {
        return (content, Vec::new());
    };
    let suffix = content.split_off(start);
    let mut resident = Vec::new();
    for item in suffix.into_iter().skip(1) {
        let UserContent::Image(image) = item else {
            continue;
        };
        let Some(id) = attached_image_id(&image).map(str::to_string) else {
            continue;
        };
        resident.push((id, image));
    }
    (content, resident)
}

/// 纯函数：收集锚点之后应保留的工具图片 id（从新到旧，最多 3 张）。
///
/// `prefix_len` 是锚点里用户原文的长度。原文和更早用户消息里的图片不再附加；
/// 锚点后缀里已经挂上的图片不参与去重，这样它们还能和新引用一起竞争这 3 个名额，
/// 落选的会被换掉，而不是一轮轮叠上去。
fn collect_turn_tool_image_ids(
    messages: &[Message],
    last_user_index: usize,
    prefix_len: usize,
) -> Vec<String> {
    let mut skip: HashSet<&str> = HashSet::new();
    for (index, message) in messages.iter().enumerate() {
        if index > last_user_index {
            break;
        }
        let Message::User { content } = message else {
            continue;
        };
        let parts = if index == last_user_index {
            &content[..prefix_len.min(content.len())]
        } else {
            content.as_slice()
        };
        for item in parts {
            if let UserContent::Image(image) = item {
                if let Some(id) = attached_image_id(image) {
                    skip.insert(id);
                }
            }
        }
    }

    let mut ids: Vec<String> = Vec::new();
    for message in messages[last_user_index + 1..].iter().rev() {
        for text in message_visible_texts(message).into_iter().rev() {
            for image_id in extract_chat_image_references(text).into_iter().rev() {
                if !skip.contains(image_id.as_str()) && !ids.contains(&image_id) {
                    ids.push(image_id);
                }
            }
        }
    }
    ids.truncate(MAX_TURN_TOOL_IMAGE_ATTACHMENTS);
    ids
}

/// 消息中可供引用扫描的可见文本（assistant 正文 / user 文本 / 工具结果文本）。
/// 不扫描 reasoning——与旧实现只扫 `message.content` 的口径一致。
fn message_visible_texts(message: &Message) -> Vec<&str> {
    match message {
        Message::Assistant { content, .. } => content
            .iter()
            .filter_map(|item| match item {
                AssistantContent::Text(text) => Some(text.text.as_str()),
                _ => None,
            })
            .collect(),
        Message::User { content } => content
            .iter()
            .flat_map(|item| match item {
                UserContent::Text(text) => vec![text.text.as_str()],
                UserContent::ToolResult(result) => result
                    .content
                    .iter()
                    .filter_map(|block| block.as_text())
                    .collect(),
                _ => Vec::new(),
            })
            .collect(),
        Message::System { .. } => Vec::new(),
    }
}

/// 从纯文本里按出现顺序抽取 `chat-image://{id}` 引用。id 形态与
/// `chat-image` scheme handler 的白名单一致（`[0-9A-Za-z-]{8,64}`），
/// 模型改写/编造的引用天然不匹配。与旧客户端的 `extract_chat_image_references`
/// 同一实现（私有函数不可复用，随 Phase 5 合并归一）。
fn extract_chat_image_references(text: &str) -> Vec<String> {
    const PROTOCOL: &str = "chat-image://";
    let mut references = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(PROTOCOL) {
        let after = &rest[start + PROTOCOL.len()..];
        // 前缀全部为 ASCII 白名单字节，字节索引切片安全。
        let id_len = after
            .bytes()
            .position(|b| !(b.is_ascii_alphanumeric() || b == b'-'))
            .unwrap_or(after.len());
        let candidate = &after[..id_len];
        if (8..=64).contains(&candidate.len()) {
            references.push(candidate.to_string());
        }
        // 前进量至少一个字符（id 为空时跳过首个非白名单字符，UTF-8 安全）。
        let skip = if id_len > 0 {
            id_len
        } else {
            after.chars().next().map_or(after.len(), char::len_utf8)
        };
        rest = &after[skip..];
    }
    references
}

#[cfg(test)]
mod tests;
