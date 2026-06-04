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

#[cfg(test)]
mod tests {
    use super::*;

    // ── slugify ──────────────────────────────────────────────────────────────

    #[test]
    fn slugify_simple_text() {
        assert_eq!(slugify("Hello World"), "hello-world");
    }

    #[test]
    fn slugify_single_word() {
        assert_eq!(slugify("Hello"), "hello");
    }

    #[test]
    fn slugify_empty_string() {
        assert_eq!(slugify(""), "untitled");
    }

    #[test]
    fn slugify_whitespace_only() {
        assert_eq!(slugify("   "), "untitled");
    }

    #[test]
    fn slugify_trims_whitespace() {
        assert_eq!(slugify("  hello  "), "hello");
    }

    #[test]
    fn slugify_special_characters_replaced() {
        assert_eq!(slugify("hello@world!test#done"), "hello-world-test-done");
    }

    #[test]
    fn slugify_chinese_characters() {
        // Chinese characters are alphanumeric in Rust (Unicode-aware)
        assert_eq!(slugify("中文测试"), "中文测试");
    }

    #[test]
    fn slugify_mixed_alphanumeric_and_special() {
        // Dots are not alphanumeric, so they become spaces, joined with hyphens
        assert_eq!(slugify("v1.2.3 release"), "v1-2-3-release");
    }

    #[test]
    fn slugify_multiple_spaces_collapse() {
        assert_eq!(slugify("hello    world"), "hello-world");
    }

    #[test]
    fn slugify_only_special_chars() {
        assert_eq!(slugify("@#$%"), "untitled");
    }

    #[test]
    fn slugify_leading_trailing_special_chars() {
        assert_eq!(slugify("---hello---"), "hello");
    }

    #[test]
    fn slugify_path_like_string() {
        assert_eq!(slugify("src/main.rs"), "src-main-rs");
    }

    #[test]
    fn slugify_preserves_hyphens_in_alphanumeric() {
        // Hyphens are not alphanumeric, so they become spaces, then joined with hyphens
        assert_eq!(slugify("my-project"), "my-project");
    }

    #[test]
    fn slugify_preserves_underscores_become_spaces() {
        // Underscores are not alphanumeric, become spaces, joined with hyphens
        assert_eq!(slugify("my_project_name"), "my-project-name");
    }

    #[test]
    fn slugify_long_session_title() {
        assert_eq!(
            slugify("Refactor Authentication Module - Phase 2"),
            "refactor-authentication-module-phase-2"
        );
    }

    // ── SaveChatImageResult struct ───────────────────────────────────────────

    #[test]
    fn save_result_serializes() {
        let result = SaveChatImageResult {
            image_id: "test-uuid".to_string(),
            path: "/some/path/test-uuid.png".to_string(),
            mime_type: "image/png".to_string(),
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("test-uuid"));
        assert!(json.contains("/some/path/test-uuid.png"));
        assert!(json.contains("image/png"));
    }

    // ── mime type to extension mapping ───────────────────────────────────────

    #[test]
    fn mime_to_ext_png() {
        let ext = match "image/png" {
            "image/png" => "png",
            "image/jpeg" => "jpg",
            "image/webp" => "webp",
            "image/gif" => "gif",
            _ => "png",
        };
        assert_eq!(ext, "png");
    }

    #[test]
    fn mime_to_ext_jpeg() {
        let ext = match "image/jpeg" {
            "image/png" => "png",
            "image/jpeg" => "jpg",
            "image/webp" => "webp",
            "image/gif" => "gif",
            _ => "png",
        };
        assert_eq!(ext, "jpg");
    }

    #[test]
    fn mime_to_ext_webp() {
        let ext = match "image/webp" {
            "image/png" => "png",
            "image/jpeg" => "jpg",
            "image/webp" => "webp",
            "image/gif" => "gif",
            _ => "png",
        };
        assert_eq!(ext, "webp");
    }

    #[test]
    fn mime_to_ext_gif() {
        let ext = match "image/gif" {
            "image/png" => "png",
            "image/jpeg" => "jpg",
            "image/webp" => "webp",
            "image/gif" => "gif",
            _ => "png",
        };
        assert_eq!(ext, "gif");
    }

    #[test]
    fn mime_to_ext_unknown_defaults_png() {
        let ext = match "image/svg+xml" {
            "image/png" => "png",
            "image/jpeg" => "jpg",
            "image/webp" => "webp",
            "image/gif" => "gif",
            _ => "png",
        };
        assert_eq!(ext, "png");
    }

    // ── app_data_dir ─────────────────────────────────────────────────────────

    #[test]
    fn app_data_dir_under_home() {
        let dir = app_data_dir().unwrap();
        let home = dirs::home_dir().unwrap();
        assert_eq!(dir, home.join(".jkcodingagent"));
    }

    // ══════════════════════════════════════════════════════════════════════════
    // Integration tests for save_chat_image command
    // ══════════════════════════════════════════════════════════════════════════

    #[tokio::test]
    async fn save_chat_image_creates_png_file() {
        let img = image::RgbImage::from_pixel(1, 1, image::Rgb([255, 0, 0]));
        let mut png_bytes = std::io::Cursor::new(Vec::new());
        image::ImageEncoder::write_image(
            image::codecs::png::PngEncoder::new(&mut png_bytes),
            &img,
            img.width(),
            img.height(),
            image::ExtendedColorType::Rgb8,
        )
        .unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(png_bytes.into_inner());

        let result = save_chat_image(
            "test-session-id".to_string(),
            "My Test Session".to_string(),
            b64,
            "image/png".to_string(),
        )
        .await
        .unwrap();

        assert!(
            !result.image_id.is_empty(),
            "image_id should be a non-empty UUID"
        );
        assert!(
            result.path.contains("my-test-session"),
            "path should contain slugified session title, got: {}",
            result.path
        );
        assert!(result.path.ends_with(".png"), "path should end with .png");

        let file_path = std::path::PathBuf::from(&result.path);
        assert!(file_path.exists(), "image file should exist on disk");
        let metadata = std::fs::metadata(&file_path).unwrap();
        assert!(metadata.len() > 0, "file should not be empty");

        if let Some(parent) = file_path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    #[tokio::test]
    async fn save_chat_image_creates_jpg_file() {
        let img = image::RgbImage::from_pixel(1, 1, image::Rgb([0, 255, 0]));
        let mut jpg_bytes = std::io::Cursor::new(Vec::new());
        image::ImageEncoder::write_image(
            image::codecs::jpeg::JpegEncoder::new(&mut jpg_bytes),
            &img,
            img.width(),
            img.height(),
            image::ExtendedColorType::Rgb8,
        )
        .unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(jpg_bytes.into_inner());

        let result = save_chat_image(
            "test-session-id".to_string(),
            "JPEG Session".to_string(),
            b64,
            "image/jpeg".to_string(),
        )
        .await
        .unwrap();

        assert!(
            result.path.ends_with(".jpg"),
            "should use .jpg extension for jpeg"
        );

        let file_path = std::path::PathBuf::from(&result.path);
        assert!(file_path.exists());
        if let Some(parent) = file_path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    #[tokio::test]
    async fn save_chat_image_creates_webp_file() {
        let img = image::RgbImage::from_pixel(1, 1, image::Rgb([0, 0, 255]));
        let mut webp_bytes = std::io::Cursor::new(Vec::new());
        image::ImageEncoder::write_image(
            image::codecs::webp::WebPEncoder::new_lossless(&mut webp_bytes),
            &img,
            img.width(),
            img.height(),
            image::ExtendedColorType::Rgb8,
        )
        .unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(webp_bytes.into_inner());

        let result = save_chat_image(
            "test-session-id".to_string(),
            "WebP Session".to_string(),
            b64,
            "image/webp".to_string(),
        )
        .await
        .unwrap();

        assert!(result.path.ends_with(".webp"), "should use .webp extension");

        let file_path = std::path::PathBuf::from(&result.path);
        assert!(file_path.exists());
        if let Some(parent) = file_path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    #[tokio::test]
    async fn save_chat_image_unknown_mime_defaults_to_png() {
        let b64 = base64::engine::general_purpose::STANDARD.encode(b"fake-image-data");

        let result = save_chat_image(
            "test-session-id".to_string(),
            "Unknown Mime".to_string(),
            b64,
            "image/svg+xml".to_string(),
        )
        .await
        .unwrap();

        assert!(
            result.path.ends_with(".png"),
            "unknown mime types should default to .png extension"
        );

        let file_path = std::path::PathBuf::from(&result.path);
        assert!(file_path.exists());
        let written = std::fs::read(&file_path).unwrap();
        assert_eq!(written, b"fake-image-data");

        if let Some(parent) = file_path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    #[tokio::test]
    async fn save_chat_image_invalid_base64_returns_error() {
        let result = save_chat_image(
            "test-session-id".to_string(),
            "Bad Base64".to_string(),
            "not valid base64!!!".to_string(),
            "image/png".to_string(),
        )
        .await;

        assert!(result.is_err(), "invalid base64 should return an error");
    }

    #[tokio::test]
    async fn save_chat_image_empty_title_uses_untitled() {
        let b64 = base64::engine::general_purpose::STANDARD.encode(b"tiny-image");

        let result = save_chat_image(
            "test-session-id".to_string(),
            "".to_string(),
            b64,
            "image/png".to_string(),
        )
        .await
        .unwrap();

        assert!(
            result.path.contains("untitled"),
            "empty title should slugify to 'untitled', got path: {}",
            result.path
        );

        let file_path = std::path::PathBuf::from(&result.path);
        if let Some(parent) = file_path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    #[tokio::test]
    async fn save_chat_image_generates_unique_ids() {
        let b64 = base64::engine::general_purpose::STANDARD.encode(b"data");

        let r1 = save_chat_image(
            "s1".to_string(),
            "Unique Test A".to_string(),
            b64.clone(),
            "image/png".to_string(),
        )
        .await
        .unwrap();
        let r2 = save_chat_image(
            "s2".to_string(),
            "Unique Test A".to_string(),
            b64,
            "image/png".to_string(),
        )
        .await
        .unwrap();

        assert_ne!(
            r1.image_id, r2.image_id,
            "each save should produce a unique image_id"
        );

        for path in [&r1.path, &r2.path] {
            let fp = std::path::PathBuf::from(path);
            if let Some(parent) = fp.parent() {
                let _ = std::fs::remove_dir_all(parent);
            }
        }
    }

    #[tokio::test]
    async fn save_chat_image_preserves_file_content_exactly() {
        let original_bytes: Vec<u8> = (0..255).collect();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&original_bytes);

        let result = save_chat_image(
            "test-session-id".to_string(),
            "Roundtrip".to_string(),
            b64,
            "image/png".to_string(),
        )
        .await
        .unwrap();

        let on_disk = std::fs::read(&result.path).unwrap();
        assert_eq!(
            on_disk, original_bytes,
            "file content should match original bytes exactly"
        );

        let file_path = std::path::PathBuf::from(&result.path);
        if let Some(parent) = file_path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    // ── compress_image_bytes ──────────────────────────────────────────────────

    fn make_large_png(width: u32, height: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_fn(width, height, |x, y| {
            image::Rgba([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8, 255])
        });
        let mut buf = std::io::Cursor::new(Vec::new());
        image::ImageEncoder::write_image(
            image::codecs::png::PngEncoder::new(&mut buf),
            img.as_raw(),
            img.width(),
            img.height(),
            image::ExtendedColorType::Rgba8,
        )
        .unwrap();
        buf.into_inner()
    }

    #[test]
    fn compress_image_bytes_small_image_unchanged() {
        let small = b"tiny-image-bytes".to_vec();
        let result = compress_image_bytes(&small, MAX_COMPRESS_DIM);
        assert_eq!(result, small);
    }

    #[test]
    fn compress_image_bytes_below_threshold_unchanged() {
        let data = vec![0u8; COMPRESS_THRESHOLD - 1];
        let result = compress_image_bytes(&data, MAX_COMPRESS_DIM);
        assert_eq!(result, data);
    }

    #[test]
    fn compress_image_bytes_large_png_is_compressed() {
        let png_bytes = make_large_png(3000, 2500);
        assert!(
            png_bytes.len() >= COMPRESS_THRESHOLD,
            "test image must exceed threshold"
        );
        let compressed = compress_image_bytes(&png_bytes, MAX_COMPRESS_DIM);
        assert!(
            compressed.len() < png_bytes.len(),
            "compressed must be smaller"
        );

        let reader = ImageReader::new(std::io::Cursor::new(&compressed))
            .with_guessed_format()
            .unwrap();
        assert_eq!(reader.format(), Some(ImageFormat::Jpeg));
        let decoded = reader.decode().unwrap();
        let (w, h) = decoded.dimensions();
        assert!(w <= MAX_COMPRESS_DIM && h <= MAX_COMPRESS_DIM);
    }

    #[test]
    fn compress_image_bytes_gif_not_reencoded() {
        let gif_data = vec![0x47, 0x49, 0x46, 0x38, 0x39]; // GIF89 magic
        let large_gif = vec![0xFF; COMPRESS_THRESHOLD + 1];
        let mut payload = gif_data;
        payload.extend_from_slice(&large_gif);
        let result = compress_image_bytes(&payload, MAX_COMPRESS_DIM);
        assert_eq!(result, payload, "GIF must pass through unchanged");
    }

    #[test]
    fn compress_image_bytes_invalid_data_passes_through() {
        let bad = vec![0xFF; COMPRESS_THRESHOLD + 1];
        let result = compress_image_bytes(&bad, MAX_COMPRESS_DIM);
        assert_eq!(result, bad);
    }

    #[tokio::test]
    async fn save_chat_image_compresses_large_png_as_jpg() {
        let large_png = make_large_png(2500, 2500);
        assert!(large_png.len() >= COMPRESS_THRESHOLD);
        let b64 = base64::engine::general_purpose::STANDARD.encode(&large_png);

        let result = save_chat_image(
            "s".to_string(),
            "Compress Test".to_string(),
            b64,
            "image/png".to_string(),
        )
        .await
        .unwrap();

        assert!(
            result.path.ends_with(".jpg"),
            "large PNG should be saved as .jpg, got: {}",
            result.path
        );
        assert_eq!(
            result.mime_type, "image/jpeg",
            "compressions should return image/jpeg mime type"
        );

        let saved_bytes = std::fs::read(&result.path).unwrap();
        assert!(
            saved_bytes.len() < large_png.len(),
            "saved file must be smaller than original"
        );

        let file_path = std::path::PathBuf::from(&result.path);
        if let Some(parent) = file_path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    #[tokio::test]
    async fn save_chat_image_small_png_keeps_png_mime_type() {
        let small_png = make_large_png(8, 8);
        assert!(small_png.len() < COMPRESS_THRESHOLD);
        let b64 = base64::engine::general_purpose::STANDARD.encode(&small_png);

        let result = save_chat_image(
            "s".to_string(),
            "Small Test".to_string(),
            b64,
            "image/png".to_string(),
        )
        .await
        .unwrap();

        assert!(
            result.path.ends_with(".png"),
            "small PNG should keep .png, got: {}",
            result.path
        );
        assert_eq!(result.mime_type, "image/png");

        let file_path = std::path::PathBuf::from(&result.path);
        if let Some(parent) = file_path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    // ── is_chat_image_path ────────────────────────────────────────────────────

    #[test]
    fn is_chat_image_path_recognizes_canonical_path() {
        let home = dirs::home_dir().unwrap();
        let path = home
            .join(".jkcodingagent")
            .join("chat-images")
            .join("slug")
            .join("id.png");
        assert!(is_chat_image_path(&path));
    }

    #[test]
    fn is_chat_image_path_rejects_outside() {
        let path = PathBuf::from("/etc/passwd");
        assert!(!is_chat_image_path(&path));
    }

    #[test]
    fn is_chat_image_path_rejects_parent_dir() {
        let home = dirs::home_dir().unwrap();
        let path = home
            .join(".jkcodingagent")
            .join("..")
            .join("etc")
            .join("passwd");
        assert!(!is_chat_image_path(&path));
    }

    // ── resolve_chat_image_id ──────────────────────────────────────────────────

    #[tokio::test]
    async fn resolve_chat_image_id_finds_saved_image() {
        let bytes = make_large_png(8, 8);
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);

        let saved = save_chat_image(
            "s".to_string(),
            "resolve-test".to_string(),
            b64,
            "image/png".to_string(),
        )
        .await
        .unwrap();

        let resolved = resolve_chat_image_id(&saved.image_id).unwrap();
        assert!(
            resolved.exists(),
            "resolved path must exist: {:?}",
            resolved
        );
        assert_eq!(
            resolved.to_string_lossy(),
            saved.path,
            "resolved path must match the saved path"
        );

        let file_path = PathBuf::from(&saved.path);
        if let Some(parent) = file_path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    #[tokio::test]
    async fn resolve_chat_image_id_strips_protocol() {
        let bytes = make_large_png(8, 8);
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);

        let saved = save_chat_image(
            "s".to_string(),
            "resolve-proto-test".to_string(),
            b64,
            "image/png".to_string(),
        )
        .await
        .unwrap();

        let uri = format!("chat-image://{}", saved.image_id);
        let resolved = resolve_chat_image_id(&uri).unwrap();
        assert!(resolved.exists());

        let file_path = PathBuf::from(&saved.path);
        if let Some(parent) = file_path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    #[test]
    fn resolve_chat_image_id_missing_id_errors() {
        let err = resolve_chat_image_id("nonexistent-uuid-12345").unwrap_err();
        assert!(
            err.contains("未找到"),
            "expected '未找到' in error, got: {}",
            err
        );
    }

    #[test]
    fn resolve_chat_image_id_empty_id_errors() {
        let err = resolve_chat_image_id("").unwrap_err();
        assert!(
            err.contains("不能为空"),
            "expected '不能为空' in error, got: {}",
            err
        );
    }
}
