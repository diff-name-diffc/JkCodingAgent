use std::path::PathBuf;
use base64::Engine;
use serde::Serialize;

/// Result of saving a chat image
#[derive(Serialize)]
pub struct SaveChatImageResult {
    pub image_id: String,
    pub path: String,
}

/// Save a chat image to the file system.
/// Images are stored under `~/.jkcodingagent/chat-images/{session-title-slug}/`.
#[tauri::command]
pub async fn save_chat_image(
    _session_id: String,
    session_title: String,
    image_data_base64: String,
    mime_type: String,
) -> Result<SaveChatImageResult, String> {
    let image_id = uuid::Uuid::new_v4().to_string();
    let ext = match mime_type.as_str() {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        _ => "png",
    };

    let app_dir = app_data_dir()?;
    let slug = slugify(&session_title);
    let images_dir = app_dir.join("chat-images").join(&slug);
    std::fs::create_dir_all(&images_dir).map_err(|e| e.to_string())?;

    let file_path = images_dir.join(format!("{}.{}", image_id, ext));
    let image_bytes = base64::engine::general_purpose::STANDARD
        .decode(&image_data_base64)
        .map_err(|e| e.to_string())?;
    std::fs::write(&file_path, &image_bytes).map_err(|e| e.to_string())?;

    Ok(SaveChatImageResult {
        image_id: image_id.clone(),
        path: file_path.to_string_lossy().to_string(),
    })
}

fn app_data_dir() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| "找不到用户主目录".to_string())?;
    Ok(home.join(".jkcodingagent"))
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
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("test-uuid"));
        assert!(json.contains("/some/path/test-uuid.png"));
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
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap();
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
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png_bytes.into_inner());

        let result = save_chat_image(
            "test-session-id".to_string(),
            "My Test Session".to_string(),
            b64,
            "image/png".to_string(),
        )
        .await
        .unwrap();

        assert!(!result.image_id.is_empty(), "image_id should be a non-empty UUID");
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
        let b64 = base64::engine::general_purpose::STANDARD.encode(&jpg_bytes.into_inner());

        let result = save_chat_image(
            "test-session-id".to_string(),
            "JPEG Session".to_string(),
            b64,
            "image/jpeg".to_string(),
        )
        .await
        .unwrap();

        assert!(result.path.ends_with(".jpg"), "should use .jpg extension for jpeg");

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
        let b64 = base64::engine::general_purpose::STANDARD.encode(&webp_bytes.into_inner());

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

        assert_ne!(r1.image_id, r2.image_id, "each save should produce a unique image_id");

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
        assert_eq!(on_disk, original_bytes, "file content should match original bytes exactly");

        let file_path = std::path::PathBuf::from(&result.path);
        if let Some(parent) = file_path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }
}
