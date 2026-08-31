//! 图片分析工具：逐张读取本地 / 远端图片，流式转 base64 后调用设置中配置的
//! 视觉用途模型，返回与输入一一对应的分析结果。
//!
//! 与 `browser_visual_analyze` 不同：这里使用 `ToolContext.vision_provider`
//! 携带的完整视觉凭据（可指向独立网关/密钥），而不是把视觉模型名拼到聊天
//! provider 上。

use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::time::timeout;

use super::common::{
    non_empty_string_array_arg, render_labeled_sections, resolve_path, string_arg,
    with_compression_parameters,
};
use crate::agent::llm::{
    ChatMessage, ChatMessageContentPart, ChatMessageImageSource, OpenAiCompatProvider,
};
use crate::agent::tools::context::ToolContext;
use crate::agent::tools::registry::AgentTool;
use crate::agent::tools::ToolResult;
use crate::chat_images::{resolve_chat_image_id_async, CHAT_IMAGE_PROTOCOL};

/// 单次调用最多分析的图片数量（与参数 schema 的 maxItems 一致）。
const MAX_IMAGES: usize = 8;
/// 单张图片大小上限（与 `agent/llm/request.rs` 的 `MAX_INLINE_IMAGE_BYTES`
/// 对齐——那里是私有常量，故在此声明副本并保持同步）。
const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;
/// 单张图片的视觉模型调用超时（秒）。整体超时由工具自管（策略表
/// `SELF_MANAGED`）：最坏 8 张 × 180s，远超任何合理的统一超时。
const PER_IMAGE_LLM_TIMEOUT_SECS: u64 = 180;
/// 远端图片下载超时（秒）。
const DOWNLOAD_TIMEOUT_SECS: u64 = 60;

/// 预设系统提示词：约束模型只陈述可见事实、控制输出体积
/// （单张默认不超过 ~500 字，避免多图结果轻易超过内联上限）。
const SYSTEM_PROMPT: &str =
    "你是一个严谨的图片分析助手。请严格按照用户给出的分析指令描述图片内容：\n\
- 只陈述图片中可见、可确认的事实，不要臆测或编造；\n\
- 无法确定的内容明确说明不确定或缺失；\n\
- 回答使用与分析指令相同的语言；\n\
- 除非指令明确要求详细展开，否则保持简洁（单张图片一般不超过 500 字）。";

pub(super) fn analyze_image_tool() -> Box<dyn AgentTool> {
    Box::new(AnalyzeImageTool)
}

struct AnalyzeImageTool;

#[async_trait]
impl AgentTool for AnalyzeImageTool {
    fn name(&self) -> &'static str {
        "analyze_image"
    }

    fn description(&self) -> &'static str {
        "按指令分析一张或多张图片的内容。图片来源支持工作区内文件路径（相对或绝对）、\
         http(s) URL、chat-image://uuid（会话图片引用）。用户粘贴到对话中的图片会以\
         「[图片引用：chat-image://uuid]」形式标注在用户消息中，必须原样复制该完整引用，\
         不要改写或编造 image_id。每张图片单独调用视觉模型分析，\
         返回与输入顺序一一对应的分析结果，单张失败不影响其余图片。\
         需要先在设置中配置视觉模型。"
    }

    fn parameters(&self) -> Value {
        with_compression_parameters(
            json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "images": {
                        "type": "array",
                        "description": "要分析的图片地址列表。支持：工作区内文件路径（相对或绝对）、\
                                        http(s) URL、chat-image://uuid（会话图片引用）。\
                                        即使只分析一张图片，也必须传单元素数组。",
                        "minItems": 1,
                        "maxItems": MAX_IMAGES,
                        "items": { "type": "string", "minLength": 1, "maxLength": 4096 }
                    },
                    "instruction": {
                        "type": "string",
                        "minLength": 1,
                        "description": "分析指令：说明需要从图片中识别、提取或判断的内容。"
                    }
                },
                "required": ["images", "instruction"]
            }),
            false,
            "分析结果默认保留完整内容；批量分析多张图片且只需要要点时可开启压缩并写明 compress_intent。",
        )
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> ToolResult {
        let Some(images) = non_empty_string_array_arg(args, "images") else {
            return ToolResult::recoverable_error(
                "错误：缺少必填参数 images，且 images 必须是非空字符串数组",
            );
        };
        if images.len() > MAX_IMAGES {
            // schema maxItems 已在入口校验，这里防御性兜底。
            return ToolResult::recoverable_error(format!(
                "错误：images 最多支持 {MAX_IMAGES} 张，收到 {} 张",
                images.len()
            ));
        }
        let Some(instruction) = string_arg(args, "instruction")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
        else {
            return ToolResult::recoverable_error("错误：缺少必填参数 instruction，且不能为空");
        };

        let Some(provider) = context.vision_provider.clone() else {
            return ToolResult::recoverable_error(
                "错误：视觉模型未配置，请先在设置中配置视觉模型后再使用 analyze_image",
            );
        };
        if !provider.is_configured() {
            return ToolResult::recoverable_error("错误：视觉模型缺少 API Key，请先在设置中配置");
        }

        // 本次调用内所有下载共用一个客户端（reqwest 内部连接池复用）。
        let download_client = match reqwest::Client::builder()
            .timeout(Duration::from_secs(DOWNLOAD_TIMEOUT_SECS))
            .build()
        {
            Ok(client) => client,
            Err(error) => {
                return ToolResult::recoverable_error(format!("错误：创建下载客户端失败：{error}"))
            }
        };

        let total = images.len();
        let mut results: Vec<Value> = Vec::with_capacity(total);
        let mut sections: Vec<(String, String)> = Vec::with_capacity(total);
        let mut succeeded = 0usize;

        'outer: for (index, raw) in images.iter().enumerate() {
            // 协作式取消：在每张图片之间检查；进行中的调用由单张超时兜底。
            if let Some(cancel_rx) = context.cancel_rx.as_ref() {
                if crate::agent::common::cancellation_requested(cancel_rx) {
                    for (offset, remaining) in images[index..].iter().enumerate() {
                        let skipped_index = index + offset;
                        let message = "错误：用户已停止，未分析该图片".to_string();
                        results.push(json!({
                            "image": remaining,
                            "index": skipped_index,
                            "status": "error",
                            "error": message,
                        }));
                        sections.push((
                            format!("图片 {}/{}：{remaining}", skipped_index + 1, total),
                            message,
                        ));
                    }
                    break 'outer;
                }
            }

            let outcome = analyze_single_image(
                &provider,
                &download_client,
                context,
                &instruction,
                index,
                raw,
            )
            .await;
            if outcome.ok {
                succeeded += 1;
            }
            results.push(outcome.data);
            sections.push((format!("图片 {}/{}：{raw}", index + 1, total), outcome.body));
        }

        let data = json!({
            "results": results,
            "total": total,
            "succeeded": succeeded,
            "failed": total - succeeded,
            "model": provider.model(),
        });
        let display = render_labeled_sections(sections);

        if succeeded == 0 {
            // 全部失败：按 read_file 先例返回可恢复错误并保留结构化数据。
            ToolResult::recoverable_error(display).with_data(data)
        } else {
            ToolResult::success_data(data, display.clone(), display)
        }
    }
}

struct ImageOutcome {
    data: Value,
    body: String,
    ok: bool,
}

async fn analyze_single_image(
    provider: &OpenAiCompatProvider,
    download_client: &reqwest::Client,
    context: &ToolContext,
    instruction: &str,
    index: usize,
    raw: &str,
) -> ImageOutcome {
    let error_outcome = |message: String| ImageOutcome {
        data: json!({
            "image": raw,
            "index": index,
            "status": "error",
            "error": message,
        }),
        body: message.clone(),
        ok: false,
    };

    let data_url = match acquire_data_url(context, download_client, raw).await {
        Ok(data_url) => data_url,
        Err(message) => return error_outcome(message),
    };

    let messages = [
        ChatMessage::system(SYSTEM_PROMPT.to_string()),
        ChatMessage {
            role: "user".to_string(),
            content: instruction.to_string(),
            content_parts: vec![
                ChatMessageContentPart::Text {
                    text: instruction.to_string(),
                },
                ChatMessageContentPart::Image {
                    source: ChatMessageImageSource::DataUrl { data_url },
                },
            ],
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        },
    ];

    let response = match timeout(
        Duration::from_secs(PER_IMAGE_LLM_TIMEOUT_SECS),
        provider.chat_stream(&messages, &[], true, |_| {}),
    )
    .await
    {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => return error_outcome(format!("错误：视觉模型分析失败：{error:#}")),
        Err(_) => {
            return error_outcome(format!(
                "错误：视觉模型分析超时（{PER_IMAGE_LLM_TIMEOUT_SECS} 秒）"
            ))
        }
    };

    let content = response.content.trim().to_string();
    if content.is_empty() {
        // 推理类模型可能把输出预算耗在思考链上，给出可行动的提示。
        return error_outcome(
            "错误：视觉模型返回了空分析结果（可能因思考链耗尽输出预算）".to_string(),
        );
    }
    ImageOutcome {
        data: json!({
            "image": raw,
            "index": index,
            "status": "success",
            "analysis": content,
        }),
        body: content,
        ok: true,
    }
}

/// 按来源取图并转 data URL：`chat-image://` 受管目录引用 / http(s) URL /
/// 工作区内本地路径。所有错误消息带「错误：」前缀。
async fn acquire_data_url(
    context: &ToolContext,
    download_client: &reqwest::Client,
    raw: &str,
) -> Result<String, String> {
    let trimmed = raw.trim();

    if trimmed.starts_with(CHAT_IMAGE_PROTOCOL) {
        // 应用受管目录（~/.jkcodingagent/chat-images/）内的可信引用，
        // 解析函数内部按 image_id 扫描受信目录，不走工作区 resolve_path。
        let path = resolve_chat_image_id_async(trimmed.to_string())
            .await
            .map_err(|e| {
                format!(
                    "错误：无法解析 chat-image 引用 `{raw}`：{e}。\
                     提示：图片引用必须原样复制用户消息中标注的 [图片引用：chat-image://uuid]，\
                     不能使用图片内容里的文本或自行编造 id"
                )
            })?;
        return tokio::task::spawn_blocking(move || stream_local_image_to_data_url(&path))
            .await
            .map_err(|e| format!("错误：读取图片任务失败：{e}"))?;
    }

    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return download_image_to_data_url(download_client, trimmed).await;
    }

    // 本地路径：工作区沙箱 + 保护路径 + 白名单校验；同步文件系统 I/O
    // 一律移入 spawn_blocking（canonicalize / metadata 等）。
    let context = context.clone();
    let raw_owned = trimmed.to_string();
    tokio::task::spawn_blocking(move || {
        let resolved = resolve_path(&context, &raw_owned)?;
        stream_local_image_to_data_url(&resolved)
    })
    .await
    .map_err(|e| format!("错误：读取图片任务失败：{e}"))?
}

/// 流式编码本地图片为 data URL：BufReader 分块读取 +
/// `base64::write::EncoderWriter` 增量编码，任意时刻内存中只有编码输出与
/// 小块缓冲，避免「原始字节 + 完整 base64」双份峰值。`take()` 做 TOCTOU
/// 硬截断（先 metadata 检查、再基于同一句柄限制读取量）。
/// 同步实现，仅在 spawn_blocking 内运行。
fn stream_local_image_to_data_url(path: &Path) -> Result<String, String> {
    let metadata = std::fs::metadata(path).map_err(|e| format!("错误：读取图片元数据失败：{e}"))?;
    if !metadata.is_file() {
        return Err(format!("错误：图片路径不是文件：{}", path.display()));
    }
    if metadata.len() > MAX_IMAGE_BYTES {
        return Err(format!(
            "错误：图片文件过大（{} 字节），超过 {} MB 限制",
            metadata.len(),
            MAX_IMAGE_BYTES / 1024 / 1024
        ));
    }
    let mime = image_mime_from_path(path)?;

    let file = std::fs::File::open(path).map_err(|e| format!("错误：打开图片失败：{e}"))?;
    let mut reader = std::io::BufReader::new(file.take(MAX_IMAGE_BYTES));

    let mut encoded: Vec<u8> = Vec::new();
    {
        let mut encoder = base64::write::EncoderWriter::new(
            &mut encoded,
            &base64::engine::general_purpose::STANDARD,
        );
        std::io::copy(&mut reader, &mut encoder).map_err(|e| format!("错误：读取图片失败：{e}"))?;
        encoder
            .finish()
            .map_err(|e| format!("错误：编码图片失败：{e}"))?;
    }
    let encoded =
        String::from_utf8(encoded).map_err(|_| "错误：base64 编码结果非法".to_string())?; // base64 输出恒为 ASCII
    Ok(format!("data:{mime};base64,{encoded}"))
}

/// 单趟流式下载 + 增量 base64 编码：边收分片边编码，原始字节不整体驻留；
/// 按原始字节数做 20MB 硬上限；MIME 优先 content-type（仅接受四种图片类型，
/// 顺带拒绝 HTML 错误页/未知类型），缺失时退回 URL 扩展名。
async fn download_image_to_data_url(client: &reqwest::Client, url: &str) -> Result<String, String> {
    use futures::StreamExt;

    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("错误：下载图片失败：{e}"))?
        .error_for_status()
        .map_err(|e| format!("错误：下载图片失败：{e}"))?;

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap_or("").trim().to_string());

    let mime = match content_type.as_deref() {
        Some(ct) if !ct.is_empty() => mime_from_content_type(ct).ok_or_else(|| {
            format!("错误：URL 返回了不支持的图片类型：{ct}（仅支持 png/jpg/jpeg/webp/gif）")
        })?,
        _ => image_mime_from_path(Path::new(url.split(['?', '#']).next().unwrap_or(url))).map_err(
            |_| "错误：无法确定图片类型（响应无 content-type 且 URL 无图片扩展名）".to_string(),
        )?,
    };

    let mut encoded: Vec<u8> = Vec::new();
    let mut received: u64 = 0;
    let mut stream = response.bytes_stream();
    {
        let mut encoder = base64::write::EncoderWriter::new(
            &mut encoded,
            &base64::engine::general_purpose::STANDARD,
        );
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| format!("错误：下载图片失败：{e}"))?;
            received = received.saturating_add(chunk.len() as u64);
            if received > MAX_IMAGE_BYTES {
                return Err(format!(
                    "错误：图片超过 {} MB 限制，已终止下载",
                    MAX_IMAGE_BYTES / 1024 / 1024
                ));
            }
            encoder
                .write_all(&chunk)
                .map_err(|e| format!("错误：编码图片失败：{e}"))?;
        }
        encoder
            .finish()
            .map_err(|e| format!("错误：编码图片失败：{e}"))?;
    }
    if received == 0 {
        return Err("错误：下载到的图片内容为空".to_string());
    }
    let encoded =
        String::from_utf8(encoded).map_err(|_| "错误：base64 编码结果非法".to_string())?;
    Ok(format!("data:{mime};base64,{encoded}"))
}

/// 与 `agent/llm/request.rs` 私有 `image_mime_from_path` 同表（该函数私有且
/// 其宿主整文件读入内存，不满足流式要求，故在工具内维护副本）。
fn image_mime_from_path(path: &Path) -> Result<&'static str, String> {
    let ext = path
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match ext.as_str() {
        "png" => Ok("image/png"),
        "jpg" | "jpeg" => Ok("image/jpeg"),
        "webp" => Ok("image/webp"),
        "gif" => Ok("image/gif"),
        _ => Err(format!(
            "错误：不支持的图片格式（仅支持 png/jpg/jpeg/webp/gif）：{}",
            path.display()
        )),
    }
}

fn mime_from_content_type(ct: &str) -> Option<&'static str> {
    match ct {
        "image/png" => Some("image/png"),
        "image/jpeg" => Some("image/jpeg"),
        "image/webp" => Some("image/webp"),
        "image/gif" => Some("image/gif"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_from_extension_covers_supported_types_and_rejects_others() {
        assert_eq!(
            image_mime_from_path(Path::new("a.PNG")).unwrap(),
            "image/png"
        );
        assert_eq!(
            image_mime_from_path(Path::new("a.jpg")).unwrap(),
            "image/jpeg"
        );
        assert_eq!(
            image_mime_from_path(Path::new("a.jpeg")).unwrap(),
            "image/jpeg"
        );
        assert_eq!(
            image_mime_from_path(Path::new("a.webp")).unwrap(),
            "image/webp"
        );
        assert_eq!(
            image_mime_from_path(Path::new("a.gif")).unwrap(),
            "image/gif"
        );
        assert!(image_mime_from_path(Path::new("a.svg")).is_err());
        assert!(image_mime_from_path(Path::new("a")).is_err());
    }

    #[test]
    fn mime_from_content_type_allow_list() {
        assert_eq!(mime_from_content_type("image/png"), Some("image/png"));
        assert_eq!(mime_from_content_type("image/jpeg"), Some("image/jpeg"));
        assert_eq!(mime_from_content_type("text/html"), None);
        assert_eq!(mime_from_content_type("image/svg+xml"), None);
    }

    #[test]
    fn stream_local_image_round_trips_bytes_to_data_url() {
        use base64::Engine;

        let dir = std::env::temp_dir().join("analyze_image_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("roundtrip.png");
        let bytes: Vec<u8> = (0..255u8).cycle().take(10_000).collect();
        std::fs::write(&path, &bytes).unwrap();

        let data_url = stream_local_image_to_data_url(&path).unwrap();

        let (head, encoded) = data_url
            .split_once(";base64,")
            .expect("data URL 缺少 base64 分隔");
        assert_eq!(head, "data:image/png");
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap();
        assert_eq!(decoded, bytes);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn stream_local_image_rejects_unsupported_extension_and_dirs() {
        let dir = std::env::temp_dir().join("analyze_image_test_reject");
        std::fs::create_dir_all(&dir).unwrap();
        let svg = dir.join("bad.svg");
        std::fs::write(&svg, b"<svg/>").unwrap();

        assert!(stream_local_image_to_data_url(&svg)
            .unwrap_err()
            .contains("不支持的图片格式"));
        // 目录不是文件
        assert!(stream_local_image_to_data_url(&dir)
            .unwrap_err()
            .contains("不是文件"));

        std::fs::remove_dir_all(&dir).ok();
    }
}
