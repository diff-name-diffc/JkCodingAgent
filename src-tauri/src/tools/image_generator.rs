//! 图片生成工具
//! 调用 DashScope 等外部 API 生成图片

use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// 图片生成工具入参
#[derive(Debug, Clone, Deserialize)]
pub struct ImageGenerationInput {
    pub prompt: String,
    pub image_name: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub style: Option<String>,
    pub negative_prompt: Option<String>,
    pub model: Option<String>,
    pub seed: Option<u64>,
}

/// 图片生成工具出参
#[derive(Debug, Clone, Serialize)]
pub struct ImageGenerationOutput {
    pub image_id: String,
    pub path: String,
    pub width: u32,
    pub height: u32,
    pub mime_type: String,
    pub generation_prompt: String,
    pub generation_params: serde_json::Value,
    pub created_at: String,
}

/// 调用 DashScope API 编辑图片，保存到本地并返回结果。
///
/// 端点：`POST /api/v1/services/aigc/multimodal-generation/generation`
///
/// 请求体格式：
/// ```json
/// {
///   "model": "qwen-image-2.0-pro",
///   "input": {
///     "messages": [
///       {
///         "role": "user",
///         "content": [
///           { "image": "data:image/png;base64,..." },
///           { "text": "..." }
///         ]
///       }
///     ]
///   },
///   "parameters": {
///     "n": 1,
///     "size": "1024*1024"
///   }
/// }
/// ```
#[allow(clippy::too_many_arguments)]
pub async fn edit_image(
    image_path: &str,
    prompt: String,
    image_name: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    session_title: String,
    api_key: &str,
    base_url: &str,
    default_model: &str,
) -> anyhow::Result<ImageGenerationOutput> {
    if api_key.is_empty() {
        anyhow::bail!("图片编辑 API Key 未配置");
    }

    // 读取图片并编码为 base64
    let image_bytes = tokio::fs::read(image_path).await
        .with_context(|| format!("无法读取图片文件: {}", image_path))?;
    let mime_type = infer_mime_type(image_path);
    let base64_image = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &image_bytes);
    let data_uri = format!("data:{};base64,{}", mime_type, base64_image);

    // 构造 size 参数
    let size = match (width, height) {
        (Some(w), Some(h)) => format!("{}*{}", w, h),
        _ => "1024*1024".to_string(),
    };

    let request_body = serde_json::json!({
        "model": default_model,
        "input": {
            "messages": [
                {
                    "role": "user",
                    "content": [
                        { "image": &data_uri },
                        { "text": &prompt }
                    ]
                }
            ]
        },
        "parameters": {
            "n": 1,
            "size": size
        }
    });

    let client = reqwest::Client::new();
    let api_base = resolve_image_api_base(base_url);
    let url = format!("{}/services/aigc/multimodal-generation/generation", api_base);

    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&request_body)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("图片编辑 API 请求失败: {} - URL: {} - 请求体: {} - 响应: {}", status, url, request_body, body);
    }

    let response_json: serde_json::Value = response.json().await?;

    let image_url = response_json["output"]["choices"][0]["message"]["content"][0]["image"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("无法从响应中获取图片 URL: {:?}", response_json))?;

    let image_response = client.get(image_url).send().await?;
    let edited_image_bytes = image_response.bytes().await?;

    let (w, h) = extract_dimensions(&response_json);
    save_and_return(edited_image_bytes.to_vec(), session_title,
        ImageGenerationInput {
            prompt: prompt.clone(),
            image_name,
            width: Some(w),
            height: Some(h),
            style: None,
            negative_prompt: None,
            model: Some(default_model.to_string()),
            seed: None,
        },
        default_model,
        w,
        h,
    ).await
}

fn infer_mime_type(path: &str) -> String {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("png")
        .to_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        _ => "image/png",
    }
    .to_string()
}

/// 调用 DashScope API 生成图片，保存到本地并返回结果。
///
/// 端点：`POST /api/v1/services/aigc/multimodal-generation/generation`
///
/// 请求体格式（参考 curl 示例）：
/// ```json
/// {
///   "model": "qwen-image-2.0-pro",
///   "input": {
///     "messages": [
///       {
///         "role": "user",
///         "content": [{ "text": "..." }]
///       }
///     ]
///   },
///   "parameters": {
///     "n": 1,
///     "negative_prompt": "...",
///     "prompt_extend": true,
///     "watermark": false,
///     "size": "1024*1024"
///   }
/// }
/// ```
pub async fn generate_image(
    input: ImageGenerationInput,
    session_title: String,
    api_key: &str,
    base_url: &str,
    default_model: &str,
) -> anyhow::Result<ImageGenerationOutput> {
    if api_key.is_empty() {
        anyhow::bail!("图片生成 API Key 未配置");
    }

    let model = input.model.clone().unwrap_or_else(|| default_model.to_string());

    // 构造 size 参数，格式 "WxH"（如 "1024*1024"）
    let size = match (input.width, input.height) {
        (Some(w), Some(h)) => format!("{}*{}", w, h),
        _ => "1024*1024".to_string(),
    };

    let request_body = json!({
        "model": model,
        "input": {
            "messages": [
                {
                    "role": "user",
                    "content": [
                        { "text": &input.prompt }
                    ]
                }
            ]
        },
        "parameters": {
            "n": 1,
            "negative_prompt": input.negative_prompt.as_deref().unwrap_or(""),
            "prompt_extend": true,
            "watermark": false,
            "size": size
        }
    });

    let client = reqwest::Client::new();
    let api_base = resolve_image_api_base(base_url);
    let url = format!("{}/services/aigc/multimodal-generation/generation", api_base);

    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&request_body)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("图片生成 API 请求失败: {} - URL: {} - 请求体: {} - 响应: {}", status, url, request_body, body);
    }

    let response_json: serde_json::Value = response.json().await?;

    // 同步返回：直接从 choices 中提取图片 URL
    let image_url = response_json["output"]["choices"][0]["message"]["content"][0]["image"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("无法从响应中获取图片 URL: {:?}", response_json))?;

    let image_response = client.get(image_url).send().await?;
    let image_bytes = image_response.bytes().await?;

    let (width, height) = extract_dimensions(&response_json);
    save_and_return(image_bytes.to_vec(), session_title, input, &model, width, height).await
}

/// 规范化图片生成 API 基础地址，确保包含 `/api/v1` 前缀。
fn resolve_image_api_base(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    if trimmed.ends_with("/api/v1") {
        trimmed.to_string()
    } else {
        format!("{}/api/v1", trimmed)
    }
}

/// 从响应 JSON 中提取宽度和高度（从 usage 字段）。
fn extract_dimensions(node: &serde_json::Value) -> (u32, u32) {
    let width = node["usage"]["width"].as_u64().unwrap_or(1024) as u32;
    let height = node["usage"]["height"].as_u64().unwrap_or(1024) as u32;
    (width, height)
}

/// 保存图片到本地并返回 ImageGenerationOutput。
async fn save_and_return(
    image_bytes: Vec<u8>,
    session_title: String,
    input: ImageGenerationInput,
    model: &str,
    width: u32,
    height: u32,
) -> anyhow::Result<ImageGenerationOutput> {
    let image_id = uuid::Uuid::new_v4().to_string();
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("无法解析用户主目录"))?;
    let slug = slugify(&session_title);
    let images_dir = home.join(".jkcodingagent").join("chat-images").join(&slug);
    std::fs::create_dir_all(&images_dir)?;

    let ext = "png";
    let file_name = match &input.image_name {
        Some(name) if !name.is_empty() => {
            let safe = slugify(name);
            format!("{}.{}", safe, ext)
        }
        _ => format!("{}.{}", image_id, ext),
    };
    let file_path = images_dir.join(&file_name);
    std::fs::write(&file_path, &image_bytes)?;

    Ok(ImageGenerationOutput {
        image_id: image_id.clone(),
        path: file_path.to_string_lossy().to_string(),
        width,
        height,
        mime_type: "image/png".to_string(),
        generation_prompt: input.prompt,
        generation_params: json!({
            "width": input.width,
            "height": input.height,
            "style": input.style,
            "negative_prompt": input.negative_prompt,
            "model": model,
            "seed": input.seed
        }),
        created_at: chrono::Utc::now().to_rfc3339(),
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
