//! fetch_image：下载图片 URL（含局域网地址）到会话图片库并返回
//! `chat-image://` 引用。让 MCP 等工具结果中的图片链接获得与粘贴图/
//! 生成图完全同权的生命周期（渲染、清理、跨轮次引用），并通过
//! `attach_turn_tool_images` 附加为本轮视觉输入。

use async_trait::async_trait;
use serde_json::{json, Value};
use std::io::Cursor;
use std::io::Write;
use tauri::Manager;

use super::common::string_arg;
use crate::agent::tools::context::ToolContext;
use crate::agent::tools::registry::AgentTool;
use crate::agent::tools::ToolResult;

const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;
const DOWNLOAD_TIMEOUT_SECS: u64 = 60;

pub(super) fn fetch_image_tool() -> Box<dyn AgentTool> {
    Box::new(FetchImageTool)
}

struct FetchImageTool;

#[async_trait]
impl AgentTool for FetchImageTool {
    fn name(&self) -> &'static str {
        "fetch_image"
    }

    fn description(&self) -> &'static str {
        "下载图片 URL（含局域网/内网地址）到会话图片库，返回 chat-image:// 引用。当工具结果（如 MCP 工具）中出现图片链接时调用本工具入库；入库图片会自动作为视觉输入附加到当前轮次，也可以在回答中用 ![描述](chat-image://...) 展示给用户。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": { "type": "string", "description": "图片的完整 URL（http/https，支持局域网地址）" }
            },
            "required": ["url"]
        })
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> ToolResult {
        ToolResult::from_text(execute_fetch_image(args, context).await)
    }
}

async fn execute_fetch_image(args: &Value, context: &ToolContext) -> String {
    let Some(url) = string_arg(args, "url") else {
        return "错误：缺少必填参数 url".to_string();
    };
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return "错误：仅支持 http/https 图片 URL".to_string();
    }

    let Some(app) = context.app_handle.clone() else {
        return "错误：应用句柄不可用，无法保存图片".to_string();
    };
    let db = app.state::<crate::agent::DispatcherState>().db().clone();

    match fetch_and_save(&db, context.workspace_id.clone(), url).await {
        Ok(fetched) => {
            let reference = format!("chat-image://{}", fetched.image_id);
            format!(
                "图片已下载入库（{}，{} 字节）：{reference}\n\n如需在回答中展示该图片，请使用：\n![图片描述]({reference})\n该图片会自动作为视觉输入附加到当前轮次；需要更细致的分析时可调用 analyze_image。",
                fetched.mime_type, fetched.byte_len
            )
        }
        Err(message) => message,
    }
}

struct FetchedImage {
    image_id: String,
    mime_type: String,
    byte_len: u64,
}

/// 流式下载（按原始字节数硬上限，超限即中止）→ 内容魔数校验 → 统一入口
/// `chat_images::save_image` 落盘登记。下载发生在本机，因此局域网地址可达。
async fn fetch_and_save(
    db: &crate::agent::db::DispatcherDb,
    workspace_id: String,
    url: String,
) -> Result<FetchedImage, String> {
    use futures::StreamExt;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(DOWNLOAD_TIMEOUT_SECS))
        .build()
        .map_err(|e| format!("错误：构建 HTTP 客户端失败：{e}"))?;
    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("错误：下载图片失败：{e}"))?
        .error_for_status()
        .map_err(|e| format!("错误：下载图片失败：{e}"))?;

    let mut bytes: Vec<u8> = Vec::new();
    let mut received: u64 = 0;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("错误：下载图片失败：{e}"))?;
        received = received.saturating_add(chunk.len() as u64);
        if received > MAX_IMAGE_BYTES {
            return Err(format!(
                "错误：图片超过 {} MB 限制，已终止下载",
                MAX_IMAGE_BYTES / 1024 / 1024
            ));
        }
        bytes
            .write_all(&chunk)
            .map_err(|e| format!("错误：缓冲图片失败：{e}"))?;
    }
    if received == 0 {
        return Err("错误：下载到的图片内容为空".to_string());
    }

    // 以内容魔数为准（不信任 content-type / URL 扩展名）：非图片响应
    // （HTML 错误页、被诱导探测的内网端点）在此被拒绝。
    let mime = sniff_image_mime(&bytes).ok_or_else(|| {
        format!(
            "错误：下载内容不是有效图片（魔数校验失败，前 8 字节为 {}，仅支持 png/jpg/webp/gif）",
            magic_prefix_preview(&bytes)
        )
    })?;

    let saved = crate::chat_images::save_image(
        db,
        crate::chat_images::SaveChatImageParams {
            workspace_id: &workspace_id,
            bytes,
            mime_type: mime,
            source: "tool_fetch",
            generation_prompt: None,
            width: None,
            height: None,
        },
    )
    .await
    .map_err(|e| format!("错误：保存图片失败：{e}"))?;

    Ok(FetchedImage {
        image_id: saved.image_id,
        mime_type: saved.mime_type,
        byte_len: received,
    })
}

/// 按内容魔数嗅探图片 mime（`with_guessed_format` 只读文件头不解码全图）。
/// 非四类受支持格式返回 None。
fn sniff_image_mime(bytes: &[u8]) -> Option<&'static str> {
    let reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    match reader.format()? {
        image::ImageFormat::Png => Some("image/png"),
        image::ImageFormat::Jpeg => Some("image/jpeg"),
        image::ImageFormat::WebP => Some("image/webp"),
        image::ImageFormat::Gif => Some("image/gif"),
        _ => None,
    }
}

/// 魔数校验失败时附在错误信息里的字节前缀预览：前 8 字节十六进制大写，
/// 不足 8 字节则全部展示（空内容为空串）。
fn magic_prefix_preview(bytes: &[u8]) -> String {
    bytes
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniff_rejects_non_image_bytes() {
        assert_eq!(sniff_image_mime(b"<html>not an image</html>"), None);
        assert_eq!(sniff_image_mime(b""), None);
        assert_eq!(sniff_image_mime("{\"error\":404}".as_bytes()), None);
        // BMP/TIFF 虽是图片魔数，但聊天图片库不支持存储，同样拒绝
        assert_eq!(sniff_image_mime(b"BM\x36\x00\x00\x00"), None);
    }

    #[test]
    fn magic_prefix_preview_caps_at_eight_bytes_and_pads_hex() {
        assert_eq!(magic_prefix_preview(b""), "");
        assert_eq!(magic_prefix_preview(&[0xFF, 0xD8]), "FF D8");
        assert_eq!(
            magic_prefix_preview(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, b'x']),
            "89 50 4E 47 0D 0A 1A 0A"
        );
    }

    #[test]
    fn sniff_detects_magic_bytes_of_supported_formats() {
        let png = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        assert_eq!(sniff_image_mime(&png), Some("image/png"));
        let jpeg = [0xFF, 0xD8, 0xFF, 0xE0];
        assert_eq!(sniff_image_mime(&jpeg), Some("image/jpeg"));
        // GIF89a
        let gif = b"GIF89a".to_vec();
        assert_eq!(sniff_image_mime(&gif), Some("image/gif"));
    }
}
