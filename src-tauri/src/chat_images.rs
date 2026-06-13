use base64::Engine;
use image::imageops::FilterType;
use image::{GenericImageView, ImageFormat, ImageReader};
use serde::Serialize;
use std::io::Cursor;
use std::path::PathBuf;

/// URI protocol prefix for internally-referenced chat images. The LLM, UI and
/// tool internals all use `chat-image://<image_id>` instead of raw filesystem
/// paths so that image references survive across machines / usernames.
pub const CHAT_IMAGE_PROTOCOL: &str = "chat-image://";

/// Result of saving a chat image
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveChatImageResult {
    pub image_id: String,
    pub path: String,
    /// Actual mime type of the saved file (may differ from original when compression occurred)
    pub mime_type: String,
}

/// Returns the canonical app data directory for chat images: `~/.jkcodingagent/chat-images`.
pub(crate) fn chat_images_dir() -> Result<PathBuf, String> {
    dirs::home_dir()
        .ok_or_else(|| "无法解析用户主目录".to_string())
        .map(|h| h.join(".jkcodingagent").join("chat-images"))
}

/// Resolve a `chat-image://<image_id>` URI or a raw image_id string to its
/// absolute filesystem path by scanning `~/.jkcodingagent/chat-images/`.
///
/// The lookup is O(N) over the number of stored chat images, which is bounded
/// and therefore acceptable. Newly saved/generated images always use
/// `{image_id}.{ext}` as filename, making this resolution deterministic.
///
/// This is shared across Tauri commands and agent tool internals so that
/// `image_edit` can accept both absolute paths and `chat-image://` URIs.
pub(crate) fn resolve_chat_image_id(image_id: &str) -> Result<PathBuf, String> {
    let id = image_id
        .strip_prefix("chat-image://")
        .unwrap_or(image_id)
        .trim();

    if id.is_empty() {
        return Err("image_id 不能为空".to_string());
    }

    let base = chat_images_dir()?;
    if !base.exists() {
        return Err(format!("chat-images 目录不存在: {}", base.display()));
    }

    let scan_dir = base.clone();
    let id_owned = id.to_string();
    // We use spawn_blocking only from async contexts; here we keep this helper
    // synchronous so that callers inside `spawn_blocking` can use it directly.
    scan_chat_images_dir(&scan_dir, &id_owned)
}

fn scan_chat_images_dir(base: &std::path::Path, image_id: &str) -> Result<PathBuf, String> {
    let entries =
        std::fs::read_dir(base).map_err(|e| format!("无法读取 chat-images 目录: {}", e))?;

    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
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

    Err(format!("未找到 image_id={} 对应的图片文件", image_id))
}

/// Async-friendly wrapper around `resolve_chat_image_id` for tool internals.
pub(crate) async fn resolve_chat_image_id_async(image_id: String) -> Result<PathBuf, String> {
    tokio::task::spawn_blocking(move || resolve_chat_image_id(&image_id))
        .await
        .map_err(|e| format!("任务调度失败: {}", e))?
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

/// Return the canonical app data directory: `~/.jkcodingagent`.
pub(crate) fn app_data_dir() -> Result<PathBuf, String> {
    dirs::home_dir()
        .ok_or_else(|| "无法解析用户主目录".to_string())
        .map(|h| h.join(".jkcodingagent"))
}

pub(crate) const COMPRESS_THRESHOLD: usize = 1_500_000;
pub(crate) const MAX_COMPRESS_DIM: u32 = 2048;
const COMPRESS_QUALITY: u8 = 92;

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

/// Save a chat image to the file system.
/// Images exceeding COMPRESS_THRESHOLD are low-loss compressed (JPEG quality 92,
/// resized to fit MAX_COMPRESS_DIM on the longest side) before being stored.
/// Images are stored under `~/.jkcodingagent/chat-images/{session-title-slug}/`.
#[tauri::command]
pub async fn save_chat_image(
    _session_id: String,
    session_title: String,
    image_data_base64: String,
    mime_type: String,
) -> Result<SaveChatImageResult, String> {
    let image_id = uuid::Uuid::new_v4().to_string();
    let raw_bytes = base64::engine::general_purpose::STANDARD
        .decode(&image_data_base64)
        .map_err(|e| e.to_string())?;

    let (ext, image_bytes) = if raw_bytes.len() >= COMPRESS_THRESHOLD {
        let compressed = compress_image_bytes(&raw_bytes, MAX_COMPRESS_DIM);
        if compressed.len() < raw_bytes.len() {
            ("jpg", compressed)
        } else {
            let ext = match mime_type.as_str() {
                "image/png" => "png",
                "image/jpeg" => "jpg",
                "image/webp" => "webp",
                "image/gif" => "gif",
                _ => "png",
            };
            (ext, raw_bytes)
        }
    } else {
        let ext = match mime_type.as_str() {
            "image/png" => "png",
            "image/jpeg" => "jpg",
            "image/webp" => "webp",
            "image/gif" => "gif",
            _ => "png",
        };
        (ext, raw_bytes)
    };

    let saved_mime_type = match ext {
        "jpg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "gif" => "image/gif",
        _ => "image/png",
    }
    .to_string();

    let slug = slugify(&session_title);
    let (id_clone, bytes) = (image_id.clone(), image_bytes);
    let mime_for_result = saved_mime_type.clone();
    tokio::task::spawn_blocking(move || -> Result<SaveChatImageResult, String> {
        let app_dir = app_data_dir()?;
        let images_dir = app_dir.join("chat-images").join(&slug);
        std::fs::create_dir_all(&images_dir).map_err(|e| e.to_string())?;
        let file_path = images_dir.join(format!("{}.{}", id_clone, ext));
        std::fs::write(&file_path, &bytes).map_err(|e| e.to_string())?;
        Ok(SaveChatImageResult {
            image_id: id_clone,
            path: file_path.to_string_lossy().to_string(),
            mime_type: mime_for_result,
        })
    })
    .await
    .map_err(|e| format!("任务调度失败: {}", e))?
}

/// Result of resolving a chat image by its identifier.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveChatImageResult {
    pub image_id: String,
    pub path: String,
    pub mime_type: String,
}

/// Resolve a `chat-image://<image_id>` URI (or raw image_id) to the image's
/// absolute filesystem path. This enables the frontend to render images
/// referenced in markdown content without exposing raw filesystem paths to
/// the user.
#[tauri::command]
pub async fn resolve_chat_image(image_id: String) -> Result<ResolveChatImageResult, String> {
    let id = image_id
        .strip_prefix("chat-image://")
        .unwrap_or(&image_id)
        .trim()
        .to_string();

    let id_clone = id.clone();
    let file_path = tokio::task::spawn_blocking(move || resolve_chat_image_id(&id_clone))
        .await
        .map_err(|e| format!("任务调度失败: {}", e))??;

    let mime_type = file_path
        .extension()
        .and_then(|e| e.to_str())
        .map(|ext| match ext.to_lowercase().as_str() {
            "jpg" | "jpeg" => "image/jpeg",
            "webp" => "image/webp",
            "gif" => "image/gif",
            "bmp" => "image/bmp",
            _ => "image/png",
        })
        .unwrap_or("image/png")
        .to_string();

    Ok(ResolveChatImageResult {
        image_id: id,
        path: file_path.to_string_lossy().to_string(),
        mime_type,
    })
}

fn slugify(s: &str) -> String {
    let s = s.trim();
    if s.is_empty() {
        return "untitled".to_string();
    }
    let slug: String = s
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == ' ' {
                c
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-");
    if slug.is_empty() {
        "untitled".to_string()
    } else {
        slug
    }
}
