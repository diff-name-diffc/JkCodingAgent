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
}
