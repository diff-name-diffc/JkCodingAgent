//! analyze_image：逐张读取本地 / 远端图片，流式转 base64 后调用设置中配置的
//! 视觉用途模型，返回与输入一一对应的分析结果。
//!
//! 视觉调用走 `deps.vision_spec` + `rig_ext::model::completions_model` 构建的
//! rig 模型发 completion 请求（替代旧 `OpenAiCompatProvider::chat_stream`）。

use std::io::Write;
use std::path::Path;
use std::time::Duration;

use rig::message::Image;
use rig::tool::{PortableDynamicTool, ToolExecutionError, ToolOutput};
use serde_json::{json, Value};

use super::super::common::{
    non_empty_string_array_arg, render_labeled_sections, resolve_path, string_arg,
    with_compression_parameters, DEFAULT_FORCE_COMPRESS_AFTER_CHARS,
};
use super::super::deps::RigToolDeps;
use super::super::super::model::PurposeModelSpec;
use super::{cancellation_requested, image_media_type_for_mime, vision_complete_image};
use crate::chat_images::{resolve_chat_image_id_async, CHAT_IMAGE_PROTOCOL};

/// 单次调用最多分析的图片数量（与参数 schema 的 maxItems 一致）。
const MAX_IMAGES: usize = 8;
/// 单张图片大小上限（与 `rig_ext::message::MAX_INLINE_IMAGE_BYTES` 对齐——
/// 那里是私有常量，故在此声明副本并保持同步）。
const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;
/// 单张图片的视觉模型调用超时（秒）。整体超时由工具自管（runtime 挂载结果
/// 策略时不应为 analyze_image 设置低于「最坏 8 张 × 180s」的统一超时）。
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

pub(super) fn analyze_image_tool(deps: &RigToolDeps) -> PortableDynamicTool {
    let deps = deps.clone();
    PortableDynamicTool::new(
        "analyze_image",
        "按指令分析一张或多张图片的内容。图片来源支持工作区内文件路径（相对或绝对）、\
         http(s) URL、chat-image://uuid（会话图片引用）。用户粘贴到对话中的图片会以\
         「[图片引用：chat-image://uuid]」形式标注在用户消息中，必须原样复制该完整引用，\
         不要改写或编造 image_id。每张图片单独调用视觉模型分析，\
         返回与输入顺序一一对应的分析结果，单张失败不影响其余图片。\
         需要先在设置中配置视觉模型。",
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
            DEFAULT_FORCE_COMPRESS_AFTER_CHARS,
            "分析结果默认保留完整内容；批量分析多张图片且只需要要点时可开启压缩并写明 compress_intent。",
        ),
        move |args: Value| {
            let deps = deps.clone();
            Box::pin(async move { execute_analyze_image(&args, &deps).await })
        },
    )
}

async fn execute_analyze_image(
    args: &Value,
    deps: &RigToolDeps,
) -> Result<ToolOutput, ToolExecutionError> {
    let Some(images) = non_empty_string_array_arg(args, "images") else {
        return Err(ToolExecutionError::invalid_args(
            "错误：缺少必填参数 images，且 images 必须是非空字符串数组",
        ));
    };
    if images.len() > MAX_IMAGES {
        // schema maxItems 已在入口校验，这里防御性兜底。
        return Err(ToolExecutionError::invalid_args(format!(
            "错误：images 最多支持 {MAX_IMAGES} 张，收到 {} 张",
            images.len()
        )));
    }
    let Some(instruction) = string_arg(args, "instruction")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    else {
        return Err(ToolExecutionError::invalid_args(
            "错误：缺少必填参数 instruction，且不能为空",
        ));
    };

    let Some(spec) = deps.vision_spec.clone() else {
        return Err(ToolExecutionError::other(
            "错误：视觉模型未配置，请先在设置中配置视觉模型后再使用 analyze_image",
        ));
    };
    if !spec.is_configured() {
        return Err(ToolExecutionError::other(
            "错误：视觉模型缺少 API Key，请先在设置中配置",
        ));
    }

    // 本次调用内所有下载共用一个客户端（reqwest 内部连接池复用）。
    let download_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(DOWNLOAD_TIMEOUT_SECS))
        .build()
        .map_err(|error| {
            ToolExecutionError::other(format!("错误：创建下载客户端失败：{error}"))
        })?;

    let total = images.len();
    let mut sections: Vec<(String, String)> = Vec::with_capacity(total);
    let mut succeeded = 0usize;

    for (index, raw) in images.iter().enumerate() {
        // 协作式取消：在每张图片之间检查；进行中的调用由单张超时兜底。
        if let Some(cancel_rx) = deps.cancel_rx.as_ref() {
            if cancellation_requested(cancel_rx) {
                for (offset, remaining) in images[index..].iter().enumerate() {
                    let skipped_index = index + offset;
                    let message = "错误：用户已停止，未分析该图片".to_string();
                    sections.push((
                        format!("图片 {}/{}：{remaining}", skipped_index + 1, total),
                        message,
                    ));
                }
                break;
            }
        }

        let outcome =
            analyze_single_image(&spec, &download_client, deps, &instruction, raw).await;
        if outcome.ok {
            succeeded += 1;
        }
        sections.push((format!("图片 {}/{}：{raw}", index + 1, total), outcome.body));
    }

    let display = render_labeled_sections(sections);

    if succeeded == 0 {
        // 全部失败：按旧 read_file 先例返回可恢复错误（runtime 会为未带
        // 「错误：」前缀的文本补前缀，对齐旧 recoverable_error 语义）。
        Err(ToolExecutionError::other(display))
    } else {
        Ok(ToolOutput::text(display))
    }
}

struct ImageOutcome {
    body: String,
    ok: bool,
}

async fn analyze_single_image(
    spec: &PurposeModelSpec,
    download_client: &reqwest::Client,
    deps: &RigToolDeps,
    instruction: &str,
    raw: &str,
) -> ImageOutcome {
    let error_outcome = |message: String| ImageOutcome {
        body: message,
        ok: false,
    };

    let (mime, base64) = match acquire_image_data(deps, download_client, raw).await {
        Ok(data) => data,
        Err(message) => return error_outcome(message),
    };

    let Some(media_type) = image_media_type_for_mime(mime) else {
        return error_outcome(format!("错误：不支持的图片类型：{mime}"));
    };
    let image = Image {
        data: rig::message::DocumentSourceKind::Base64(base64),
        media_type: Some(media_type),
        detail: None,
        additional_params: None,
    };

    let content = match vision_complete_image(
        spec,
        SYSTEM_PROMPT,
        instruction.to_string(),
        image,
        Some(Duration::from_secs(PER_IMAGE_LLM_TIMEOUT_SECS)),
        "视觉模型分析",
    )
    .await
    {
        Ok(content) => content,
        Err(message) => return error_outcome(message),
    };

    if content.is_empty() {
        // 推理类模型可能把输出预算耗在思考链上，给出可行动的提示。
        return error_outcome(
            "错误：视觉模型返回了空分析结果（可能因思考链耗尽输出预算）".to_string(),
        );
    }
    ImageOutcome {
        body: content,
        ok: true,
    }
}

/// 按来源取图并转 base64：`chat-image://` 受管目录引用 / http(s) URL /
/// 工作区内本地路径。返回（mime, base64）；所有错误消息带「错误：」前缀。
async fn acquire_image_data(
    deps: &RigToolDeps,
    download_client: &reqwest::Client,
    raw: &str,
) -> Result<(&'static str, String), String> {
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
        return tokio::task::spawn_blocking(move || stream_local_image_to_base64(&path))
            .await
            .map_err(|e| format!("错误：读取图片任务失败：{e}"))?;
    }

    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return download_image_to_base64(download_client, trimmed).await;
    }

    // 本地路径：工作区沙箱 + 保护路径 + 白名单校验；同步文件系统 I/O
    // 一律移入 spawn_blocking（canonicalize / metadata 等）。
    let workspace = deps.workspace.clone();
    let restrict = deps.restrict_to_workspace;
    let extra_dirs = deps.extra_allowed_dirs.clone();
    let raw_owned = trimmed.to_string();
    tokio::task::spawn_blocking(move || {
        let resolved = resolve_path(&workspace, restrict, &extra_dirs, &raw_owned)?;
        stream_local_image_to_base64(&resolved)
    })
    .await
    .map_err(|e| format!("错误：读取图片任务失败：{e}"))?
}

/// 流式编码本地图片为 base64：BufReader 分块读取 +
/// `base64::write::EncoderWriter` 增量编码，任意时刻内存中只有编码输出与
/// 小块缓冲，避免「原始字节 + 完整 base64」双份峰值。`take()` 做 TOCTOU
/// 硬截断（先 metadata 检查、再基于同一句柄限制读取量）。
/// 同步实现，仅在 spawn_blocking 内运行。
fn stream_local_image_to_base64(path: &Path) -> Result<(&'static str, String), String> {
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
    let mut reader = std::io::BufReader::new(std::io::Read::take(file, MAX_IMAGE_BYTES));

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
    Ok((mime, encoded))
}

/// 单趟流式下载 + 增量 base64 编码：边收分片边编码，原始字节不整体驻留；
/// 按原始字节数做 20MB 硬上限；MIME 优先 content-type（仅接受四种图片类型，
/// 顺带拒绝 HTML 错误页/未知类型），缺失时退回 URL 扩展名。
async fn download_image_to_base64(
    client: &reqwest::Client,
    url: &str,
) -> Result<(&'static str, String), String> {
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
    Ok((mime, encoded))
}

/// 与 `rig_ext::message::local_image_to_base64` 同表（该函数私有，故在工具内
/// 维护副本）。
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
    fn stream_local_image_round_trips_bytes_to_base64() {
        use base64::Engine;

        let dir = std::env::temp_dir().join("analyze_image_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("roundtrip.png");
        let bytes: Vec<u8> = (0..255u8).cycle().take(10_000).collect();
        std::fs::write(&path, &bytes).unwrap();

        let (mime, encoded) = stream_local_image_to_base64(&path).unwrap();

        assert_eq!(mime, "image/png");
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&encoded)
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

        assert!(stream_local_image_to_base64(&svg)
            .unwrap_err()
            .contains("不支持的图片格式"));
        // 目录不是文件
        assert!(stream_local_image_to_base64(&dir)
            .unwrap_err()
            .contains("不是文件"));

        std::fs::remove_dir_all(&dir).ok();
    }
}
