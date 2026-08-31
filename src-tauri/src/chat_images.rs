use image::imageops::FilterType;
use image::{GenericImageView, ImageFormat, ImageReader};
use serde::Serialize;
use std::io::Cursor;
use std::path::PathBuf;
use tauri::Manager;

use crate::shared::error::{CommandResult, IntoCommandResult};
use anyhow::Context;

type ChatImageResult<T> = std::result::Result<T, ChatImageError>;

#[derive(Debug, thiserror::Error)]
pub enum ChatImageError {
    #[error("无法解析用户主目录")]
    HomeDirMissing,
    #[error("image_id 不能为空")]
    EmptyImageId,
    #[error("chat-images 目录不存在：{0}")]
    ImagesDirMissing(PathBuf),
    #[error("未找到 image_id={0} 对应的图片文件")]
    ImageNotFound(String),
    #[error("解码图片 base64 失败：{0}")]
    Decode(String),
    #[error("{action} 失败（{path}）：{source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("不支持的图片 mime 类型：{0}")]
    UnsupportedMime(String),
    #[error("非法的会话标识：{0}")]
    InvalidWorkspaceId(String),
    #[error("后台图片任务失败：{0}")]
    Join(#[from] tokio::task::JoinError),
}

fn io_error(
    action: &'static str,
    path: impl Into<PathBuf>,
) -> impl FnOnce(std::io::Error) -> ChatImageError {
    move |source| ChatImageError::Io {
        action,
        path: path.into(),
        source,
    }
}

/// URI protocol prefix for internally-referenced chat images. The UI and tool
/// internals use `chat-image://<image_id>` instead of raw filesystem paths so
/// that image references survive across machines / usernames.
pub const CHAT_IMAGE_PROTOCOL: &str = "chat-image://";

/// Returns the canonical app data directory for chat images: `~/.jkcodingagent/chat-images`.
pub(crate) fn chat_images_dir() -> ChatImageResult<PathBuf> {
    dirs::home_dir()
        .ok_or(ChatImageError::HomeDirMissing)
        .map(|home| home.join(".jkcodingagent").join("chat-images"))
}

/// Resolve a `chat-image://<image_id>` URI or a raw image_id string to its
/// absolute filesystem path.
///
/// Resolution order: ① chat_images DB 索引行（O(1)，仅传入 db 时）；
/// ② `~/.jkcodingagent/chat-images/` 全目录扫描（兜底，覆盖登记失败
/// best-effort 落盘的文件）。文件名恒为 `{image_id}.{ext}`，扫描按
/// file_stem 精确匹配。
///
/// This is shared across the `chat-image` URI scheme handler, LLM request
/// building and agent tool internals so that every consumer resolves image
/// references the same way.
pub(crate) fn resolve_chat_image_by_id(
    db: Option<&crate::agent::db::DispatcherDb>,
    image_id: &str,
) -> ChatImageResult<PathBuf> {
    let id = image_id
        .strip_prefix(CHAT_IMAGE_PROTOCOL)
        .unwrap_or(image_id)
        .trim();

    if id.is_empty() {
        return Err(ChatImageError::EmptyImageId);
    }

    if let Some(db) = db {
        match db.chat_image_path(id) {
            Ok(Some(path)) => return Ok(path),
            Ok(None) => {}
            Err(error) => eprintln!("查询 chat_images 索引失败（{id}），回退扫描：{error:#}"),
        }
    }

    let base = chat_images_dir()?;
    if !base.exists() {
        return Err(ChatImageError::ImagesDirMissing(base));
    }

    scan_chat_images_dir(&base, id)
}

/// 无 DB 句柄时的解析入口（工具内部 / LLM 请求构造）。
pub(crate) fn resolve_chat_image_id(image_id: &str) -> ChatImageResult<PathBuf> {
    resolve_chat_image_by_id(None, image_id)
}

fn scan_chat_images_dir(base: &std::path::Path, image_id: &str) -> ChatImageResult<PathBuf> {
    let entries = std::fs::read_dir(base).map_err(io_error("读取 chat-images 目录", base))?;

    for entry in entries {
        let entry = entry.map_err(io_error("读取 chat-images 目录项", base))?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let sub_entries = match std::fs::read_dir(&path) {
            Ok(it) => it,
            Err(_) => continue,
        };
        for sub in sub_entries {
            let sub = match sub {
                Ok(s) => s,
                Err(_) => continue,
            };
            let file = sub.path();
            if let Some(stem) = file.file_stem().and_then(|s| s.to_str()) {
                if stem == image_id {
                    return Ok(file);
                }
            }
        }
    }

    Err(ChatImageError::ImageNotFound(image_id.to_string()))
}

/// Async-friendly wrapper around `resolve_chat_image_id` for tool internals.
pub(crate) async fn resolve_chat_image_id_async(image_id: String) -> ChatImageResult<PathBuf> {
    tokio::task::spawn_blocking(move || resolve_chat_image_id(&image_id)).await?
}

/// Check whether a path is under the app-managed `~/.jkcodingagent/chat-images/` directory.
pub(crate) fn is_chat_image_path(path: &std::path::Path) -> bool {
    let Ok(dir) = chat_images_dir() else {
        return false;
    };
    match path.canonicalize() {
        Ok(canonical_path) => match dir.canonicalize() {
            Ok(canonical_dir) => canonical_path.starts_with(&canonical_dir),
            Err(_) => false,
        },
        Err(_) => {
            let normalized = path.components().fold(PathBuf::new(), |mut acc, c| {
                match c {
                    std::path::Component::ParentDir => {
                        acc.pop();
                    }
                    std::path::Component::CurDir => {}
                    _ => acc.push(c),
                }
                acc
            });
            normalized.starts_with(&dir)
        }
    }
}

pub(crate) const COMPRESS_THRESHOLD: usize = 1_500_000;
pub(crate) const MAX_COMPRESS_DIM: u32 = 2048;
const COMPRESS_QUALITY: u8 = 92;

/// 统一的 mime → 扩展名映射（严格）。这是全应用唯一的 mime↔ext 出处：
/// 未知 mime 返回 Err，而不是静默落成 .png 坏文件（历史上 svg/bmp 字节流
/// 会被存成打不开的 .png）。
pub(crate) fn ext_for_mime(mime: &str) -> ChatImageResult<&'static str> {
    match mime.trim().to_lowercase().as_str() {
        "image/png" => Ok("png"),
        "image/jpeg" => Ok("jpg"),
        "image/webp" => Ok("webp"),
        "image/gif" => Ok("gif"),
        other => Err(ChatImageError::UnsupportedMime(other.to_string())),
    }
}

/// 统一的扩展名 → mime 映射（不含 bmp/svg：LLM 内联与 scheme 服务均只支持
/// png/jpg/webp/gif 四类，保持与 `ext_for_mime` 严格对齐）。
pub(crate) fn mime_for_ext(ext: &str) -> Option<&'static str> {
    match ext.to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "webp" => Some("image/webp"),
        "gif" => Some("image/gif"),
        _ => None,
    }
}

/// Low-loss image compression shared across modules.
///
/// - Resizes images exceeding `max_dim` on the longest side, preserving aspect ratio.
/// - Returns JPEG-encoded bytes when the result is smaller than the original;
///   otherwise returns the original bytes unchanged.
/// - GIF images are never re-encoded (would destroy animation).
pub(crate) fn compress_image_bytes(raw: &[u8], max_dim: u32) -> Vec<u8> {
    if raw.len() < COMPRESS_THRESHOLD {
        return raw.to_vec();
    }

    let Ok(reader) = ImageReader::new(Cursor::new(raw)).with_guessed_format() else {
        return raw.to_vec();
    };

    if matches!(reader.format(), Some(ImageFormat::Gif)) {
        return raw.to_vec();
    }

    let Ok(img) = reader.decode() else {
        return raw.to_vec();
    };

    let (w, h) = img.dimensions();
    let longest = w.max(h);

    let resized: image::DynamicImage = if longest > max_dim {
        img.resize(max_dim, max_dim, FilterType::Lanczos3)
    } else {
        img
    };

    let rgb = resized.to_rgb8();
    let (w, h) = rgb.dimensions();
    let mut out = Cursor::new(Vec::with_capacity(raw.len() / 2));
    if image::ImageEncoder::write_image(
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, COMPRESS_QUALITY),
        rgb.as_raw(),
        w,
        h,
        image::ExtendedColorType::Rgb8,
    )
    .is_err()
    {
        return raw.to_vec();
    }

    let compressed = out.into_inner();
    if compressed.len() < raw.len() {
        compressed
    } else {
        raw.to_vec()
    }
}

/// 会话图片目录：`~/.jkcodingagent/chat-images/{workspace_id}`。
/// workspace_id 即 dispatcher 会话 id（uuid 形态），不可变且与 chat_images
/// 索引一致——目录与会话同生命周期，标题改名永不迁移。
pub(crate) fn workspace_image_dir(workspace_id: &str) -> ChatImageResult<PathBuf> {
    if !is_valid_workspace_id(workspace_id) {
        return Err(ChatImageError::InvalidWorkspaceId(workspace_id.to_string()));
    }
    Ok(chat_images_dir()?.join(workspace_id))
}

/// workspace_id 字符集白名单：该 id 直接拼进目录路径，是目录穿越的唯一
/// 闸门（`[0-9A-Za-z_-]{1,64}`，与 uuid / slug 化 id 形态兼容）。
pub(crate) fn is_valid_workspace_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// 统一保存参数：用户粘贴上传与 generate_image / edit_image 工具产物共用。
pub(crate) struct SaveChatImageParams<'a> {
    pub workspace_id: &'a str,
    pub bytes: Vec<u8>,
    pub mime_type: &'a str,
    /// 登记来源："user_paste" | "tool_generate"。
    pub source: &'a str,
    pub generation_prompt: Option<&'a str>,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

#[derive(Debug, Clone)]
pub(crate) struct SavedChatImage {
    pub image_id: String,
    pub path: PathBuf,
    pub mime_type: String,
}

/// 唯一的聊天图片落盘入口：超阈值低损压缩、严格 mime 映射，写入
/// `chat-images/{workspace_id}/{image_id}.{ext}`，并最佳努力登记 chat_images
/// 索引行（message_id 为 NULL，消息落库时由 insert_chat_images 绑定）。
/// 登记失败只留痕不回滚——文件是事实源，索引可由消息路径补齐。
pub(crate) async fn save_image(
    db: &crate::agent::db::DispatcherDb,
    params: SaveChatImageParams<'_>,
) -> ChatImageResult<SavedChatImage> {
    let image_id = uuid::Uuid::new_v4().to_string();
    let (ext, image_bytes) = if params.bytes.len() >= COMPRESS_THRESHOLD {
        let compressed = compress_image_bytes(&params.bytes, MAX_COMPRESS_DIM);
        if compressed.len() < params.bytes.len() {
            ("jpg", compressed)
        } else {
            (ext_for_mime(params.mime_type)?, params.bytes)
        }
    } else {
        (ext_for_mime(params.mime_type)?, params.bytes)
    };
    let saved_mime = mime_for_ext(ext)
        .expect("ext_for_mime 与 mime_for_ext 保持对齐")
        .to_string();

    let dir = workspace_image_dir(params.workspace_id)?;
    let file_path = dir.join(format!("{}.{}", image_id, ext));
    let register = ChatImageRegistration {
        image_id: image_id.clone(),
        workspace_id: params.workspace_id.to_string(),
        width: params.width,
        height: params.height,
        mime_type: saved_mime.clone(),
        source: params.source.to_string(),
        generation_prompt: params.generation_prompt.map(str::to_string),
    };
    let db = db.clone();
    let saved_path = file_path.clone();
    tokio::task::spawn_blocking(move || -> ChatImageResult<()> {
        std::fs::create_dir_all(&dir).map_err(io_error("创建会话图片目录", dir.clone()))?;
        std::fs::write(&file_path, &image_bytes)
            .map_err(io_error("写入聊天图片", file_path.clone()))?;
        if let Err(error) = db.register_chat_image(&register, &file_path) {
            eprintln!("登记聊天图片失败（{}）：{error:#}", register.image_id);
        }
        Ok(())
    })
    .await??;

    Ok(SavedChatImage {
        image_id,
        path: saved_path,
        mime_type: saved_mime,
    })
}

/// 保存即登记的一行索引（见 save_image）。
pub(crate) struct ChatImageRegistration {
    pub image_id: String,
    pub workspace_id: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub mime_type: String,
    pub source: String,
    pub generation_prompt: Option<String>,
}

/// Result of saving a chat image.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveChatImageResult {
    pub image_id: String,
    /// Actual mime type of the saved file (may differ from original when compression occurred)
    pub mime_type: String,
}

/// Save a chat image pasted/attached from the frontend. Thin wrapper over the
/// unified `save_image` entry point (compression + strict mime mapping +
/// `chat-images/{workspace_id}/` layout + index registration).
#[tauri::command]
pub async fn save_chat_image(
    workspace_id: String,
    image_data_base64: String,
    mime_type: String,
    state: tauri::State<'_, crate::agent::DispatcherState>,
) -> CommandResult<SaveChatImageResult> {
    save_chat_image_impl(
        state.db().clone(),
        workspace_id,
        image_data_base64,
        mime_type,
    )
    .await
    .context("保存聊天图片失败")
    .into_command_result()
}

async fn save_chat_image_impl(
    db: crate::agent::db::DispatcherDb,
    workspace_id: String,
    image_data_base64: String,
    mime_type: String,
) -> ChatImageResult<SaveChatImageResult> {
    use base64::Engine;

    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&image_data_base64)
        .map_err(|e| ChatImageError::Decode(e.to_string()))?;
    let saved = save_image(
        &db,
        SaveChatImageParams {
            workspace_id: &workspace_id,
            bytes,
            mime_type: &mime_type,
            source: "user_paste",
            generation_prompt: None,
            width: None,
            height: None,
        },
    )
    .await?;
    Ok(SaveChatImageResult {
        image_id: saved.image_id,
        mime_type: saved.mime_type,
    })
}

/// 发送带图消息前的存在性校验（regenerate/edit 在截断消息前调用，避免
/// 截断后发送失败丢消息）。Image 段引用的文件缺失时返回带引用清单的错误。
#[tauri::command]
pub async fn chat_images_validate(
    segments_json: String,
    state: tauri::State<'_, crate::agent::DispatcherState>,
) -> CommandResult<()> {
    state
        .db()
        .validate_chat_image_segments_async(&segments_json)
        .await
        .context("校验聊天图片失败")
        .into_command_result()
}

/// image_id 的 URI 形态校验（`[0-9A-Za-z-]{8,64}`）。它是 `chat-image` scheme
/// handler 唯一的安全闸门：来自 WebView 的任意请求路径都会流经这里，
/// 字符集白名单确保不可能构造出目录穿越或任意文件读取。
fn is_valid_image_reference(id: &str) -> bool {
    (8..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn respond_image_not_found(responder: tauri::UriSchemeResponder) {
    let response = tauri::http::Response::builder()
        .status(tauri::http::StatusCode::NOT_FOUND)
        .body(Vec::new());
    match response {
        Ok(response) => responder.respond(response),
        Err(_) => responder.respond(
            tauri::http::Response::builder()
                .status(tauri::http::StatusCode::NOT_FOUND)
                .body(Vec::new())
                .expect("static 404 response builds"),
        ),
    }
}

/// `chat-image` 自定义协议处理器：`<img src>` 直接引用
/// `chat-image://{image_id}`（经 convertFileSrc 按平台转换为
/// `chat-image://localhost/{id}` 或 `http://chat-image.localhost/{id}`），
/// 由这里查索引、读盘并以正确的 Content-Type 返回——前端不再需要
/// invoke resolve 两阶段渲染。
///
/// 安全与性能约定：
/// - id 字符集白名单（见 is_valid_image_reference）+ 只按 file_stem 匹配，
///   双保险防目录穿越；
/// - image_id 不可变，因此下发 `Cache-Control: immutable`，历史滚动零重复
///   读盘；
/// - handler 内严禁阻塞（会卡 WebView 全部网络）：解析与读盘全部经
///   spawn_blocking，DB 连接即取即放后再读盘。
pub(crate) fn handle_chat_image_scheme(
    ctx: tauri::UriSchemeContext<'_, tauri::Wry>,
    request: tauri::http::Request<Vec<u8>>,
    responder: tauri::UriSchemeResponder,
) {
    use percent_encoding::percent_decode_str;

    let uri = request.uri().clone();
    // 兼容三种形态：macOS `chat-image://localhost/{id}`、`chat-image://{id}`
    // （host 承载 id、path 为空）与 Windows/Linux `http://chat-image.localhost/{id}`。
    let raw_id = uri.path().trim_start_matches('/').to_string();
    let raw_id = if raw_id.is_empty() {
        uri.host().unwrap_or_default().to_string()
    } else {
        raw_id
    };
    let image_id = percent_decode_str(&raw_id).decode_utf8_lossy().to_string();

    if !is_valid_image_reference(&image_id) {
        respond_image_not_found(responder);
        return;
    }

    let app = ctx.app_handle().clone();
    tauri::async_runtime::spawn(async move {
        let db = app.state::<crate::agent::DispatcherState>().db().clone();
        let served = tokio::task::spawn_blocking(move || -> Option<(Vec<u8>, &'static str)> {
            let path = resolve_chat_image_by_id(Some(&db), &image_id).ok()?;
            let ext = path.extension()?.to_str()?;
            let mime = mime_for_ext(ext)?;
            let bytes = std::fs::read(&path).ok()?;
            Some((bytes, mime))
        })
        .await
        .ok()
        .flatten();

        match served {
            Some((bytes, mime)) => {
                let response = tauri::http::Response::builder()
                    .header("Content-Type", mime)
                    .header("Cache-Control", "private, max-age=31536000, immutable")
                    .body(bytes);
                match response {
                    Ok(response) => responder.respond(response),
                    Err(_) => respond_image_not_found(responder),
                }
            }
            None => respond_image_not_found(responder),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ext_for_mime_is_strict() {
        assert_eq!(ext_for_mime("image/png").unwrap(), "png");
        assert_eq!(ext_for_mime("image/jpeg").unwrap(), "jpg");
        assert_eq!(ext_for_mime("image/webp").unwrap(), "webp");
        assert_eq!(ext_for_mime("IMAGE/GIF").unwrap(), "gif");
        assert!(ext_for_mime("image/svg+xml").is_err());
        assert!(ext_for_mime("image/bmp").is_err());
        assert!(ext_for_mime("application/octet-stream").is_err());
        assert!(ext_for_mime("").is_err());
    }

    #[test]
    fn mime_for_ext_round_trips_supported_formats() {
        for mime in ["image/png", "image/jpeg", "image/webp", "image/gif"] {
            let ext = ext_for_mime(mime).expect("supported mime");
            assert_eq!(mime_for_ext(ext), Some(mime));
        }
        assert_eq!(mime_for_ext("jpg"), Some("image/jpeg"));
        assert_eq!(mime_for_ext("jpeg"), Some("image/jpeg"));
        assert_eq!(mime_for_ext("bmp"), None);
        assert_eq!(mime_for_ext("svg"), None);
        assert_eq!(mime_for_ext(""), None);
    }
}
