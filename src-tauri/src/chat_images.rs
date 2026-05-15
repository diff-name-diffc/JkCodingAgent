use std::path::PathBuf;
use base64::Engine;
use serde::Serialize;

/// Result of saving a chat image
#[derive(Serialize)]
pub struct SaveChatImageResult {
    pub image_id: String,
    pub path: String,
}

/// Result of reading a chat image
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadChatImageResult {
    pub data_url: String,
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

/// Read a chat image file and return as base64 data URL.
#[tauri::command]
pub async fn read_chat_image_file(path: String) -> Result<ReadChatImageResult, String> {
    // Validate the path is within the chat-images directory
    let app_dir = app_data_dir()?;
    let chat_images_dir = app_dir.join("chat-images");
    let target = std::path::Path::new(&path);

    // Basic path validation: must be absolute and under chat-images
    if !target.is_absolute() {
        return Err("Path must be absolute".to_string());
    }

    let canonical_target = target.canonicalize().map_err(|e| e.to_string())?;
    let canonical_chat_images = chat_images_dir.canonicalize().map_err(|e| e.to_string())?;

    if !canonical_target.starts_with(&canonical_chat_images) {
        return Err("Path is outside the allowed chat-images directory".to_string());
    }

    // Read file and encode as base64
    let bytes = std::fs::read(&canonical_target).map_err(|e| e.to_string())?;

    // Detect mime type from extension
    let mime_type = canonical_target
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| match ext.to_lowercase().as_str() {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "webp" => "image/webp",
            "gif" => "image/gif",
            _ => "image/png",
        })
        .unwrap_or("image/png");

    let base64_data = base64::engine::general_purpose::STANDARD.encode(&bytes);

    Ok(ReadChatImageResult {
        data_url: format!("data:{};base64,{}", mime_type, base64_data),
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
