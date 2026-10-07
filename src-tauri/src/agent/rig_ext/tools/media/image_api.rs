//! 图片生成/编辑的 DashScope 直连 HTTP 层。
//!
//! 迁移自旧 `crate::tools::image_generator`（该模块随旧工具层退役）：
//! 端点、请求体、尺寸/尺寸缺省、错误文案全部保留；产物落盘仍走唯一入口
//! `crate::chat_images::save_image`（source = tool_generate）。

use anyhow::Context;
use serde_json::json;
use std::time::Duration;
use tokio::sync::watch;

use crate::agent::common::wait_for_optional_cancellation as cancellation;
use crate::chat_images::compress_image_bytes;

const MAX_FILE_READ_BYTES: usize = 50_000_000;

/// DashScope 多模态生成端点路径（文生图与图片编辑共用）。
const MULTIMODAL_GENERATION_PATH: &str = "/services/aigc/multimodal-generation/generation";

/// 图片生成工具入参
#[derive(Debug, Clone)]
pub(super) struct ImageGenerationInput {
    pub prompt: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// 图片风格（可选）：声明时透传到请求体 `parameters.style`。
    pub style: Option<String>,
    pub negative_prompt: Option<String>,
    pub model: Option<String>,
    /// 随机种子（可选）：声明时透传到请求体 `parameters.seed`。
    pub seed: Option<u64>,
}

/// 图片生成工具出参
#[derive(Debug, Clone)]
pub(super) struct ImageGenerationOutput {
    pub image_id: String,
    pub width: u32,
    pub height: u32,
    pub generation_prompt: String,
}

fn make_client(timeout_secs: u64) -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .build()
}

/// 取消文案：服务端可能已完成生成并计费（HTTP 请求无法撤回），取消仅停止
/// 本地等待；由调用方拼上下文（生成/编辑）。
const CANCELLED_MESSAGE: &str = "已取消（服务端可能仍在处理并计费，结果不再等待）";

/// 调用 DashScope API 编辑图片，保存到本地并返回结果。
///
/// 端点：`POST /api/v1/services/aigc/multimodal-generation/generation`。
/// `timeout_secs` 为单请求 HTTP 超时（由调用方经 spec::effective_timeout_secs
/// 解析，与策略层 deadline 同源）；POST（含响应体读取）与产物下载均感知
/// `cancel_rx`。
#[allow(clippy::too_many_arguments)]
pub(super) async fn edit_image(
    image_path: &str,
    prompt: String,
    width: Option<u32>,
    height: Option<u32>,
    db: crate::agent::db::DispatcherDb,
    workspace_id: String,
    api_key: &str,
    base_url: &str,
    default_model: &str,
    timeout_secs: u64,
    cancel_rx: Option<watch::Receiver<bool>>,
) -> anyhow::Result<ImageGenerationOutput> {
    if api_key.is_empty() {
        anyhow::bail!("图片编辑 API Key 未配置");
    }

    let image_bytes = tokio::fs::read(image_path)
        .await
        .with_context(|| format!("无法读取图片文件: {}", image_path))?;

    if image_bytes.len() > MAX_FILE_READ_BYTES {
        anyhow::bail!(
            "图片文件过大: {} MB (最大支持 {} MB)",
            image_bytes.len() / 1_000_000,
            MAX_FILE_READ_BYTES / 1_000_000
        );
    }

    let payload_bytes = if image_bytes.len() >= crate::chat_images::COMPRESS_THRESHOLD {
        compress_image_bytes(&image_bytes, crate::chat_images::MAX_COMPRESS_DIM)
    } else {
        image_bytes
    };

    let input_ext = std::path::Path::new(image_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default();
    let mime_type = crate::chat_images::mime_for_ext(input_ext).unwrap_or("image/png");
    let base64_image =
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &payload_bytes);
    let data_uri = format!("data:{};base64,{}", mime_type, base64_image);

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

    let client = make_client(timeout_secs).context("构建 HTTP 客户端失败")?;
    let url = multimodal_generation_endpoint(base_url);

    let response = tokio::select! {
        biased;
        _ = cancellation(cancel_rx.clone()) => anyhow::bail!("图片编辑 {CANCELLED_MESSAGE}"),
        result = client
            .post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send() => result.context("图片编辑 API 请求失败（网络层）")?,
    };

    if !response.status().is_success() {
        let status = response.status();
        let body = tokio::select! {
            biased;
            _ = cancellation(cancel_rx.clone()) => anyhow::bail!("图片编辑 {CANCELLED_MESSAGE}"),
            result = response.text() => result
                .unwrap_or_else(|error| format!("（错误响应体读取失败：{error}）")),
        };
        anyhow::bail!(
            "图片编辑 API 请求失败: {} (model={}) - 响应: {}",
            status,
            default_model,
            body
        );
    }

    let response_json: serde_json::Value = tokio::select! {
        biased;
        _ = cancellation(cancel_rx.clone()) => anyhow::bail!("图片编辑 {CANCELLED_MESSAGE}"),
        result = response.json() => result?,
    };

    let image_url = response_json["output"]["choices"][0]["message"]["content"][0]["image"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("无法从响应中获取图片 URL: {:?}", response_json))?;

    let image_response = tokio::select! {
        biased;
        _ = cancellation(cancel_rx.clone()) => anyhow::bail!("图片编辑产物下载 {CANCELLED_MESSAGE}"),
        result = client.get(image_url).send() => result?,
    };
    let edited_image_bytes = tokio::select! {
        biased;
        _ = cancellation(cancel_rx.clone()) => anyhow::bail!("图片编辑产物下载 {CANCELLED_MESSAGE}"),
        result = image_response.bytes() => result?,
    };

    let (w, h) = extract_dimensions(&response_json);
    let saved = persist_generated_image(
        db,
        workspace_id,
        edited_image_bytes.to_vec(),
        &prompt,
        Some(w),
        Some(h),
        cancel_rx,
    )
    .await?;

    Ok(ImageGenerationOutput {
        image_id: saved.image_id,
        width: w,
        height: h,
        generation_prompt: prompt,
    })
}

/// 调用 DashScope API 生成图片，保存到本地并返回结果。
///
/// 端点：`POST /api/v1/services/aigc/multimodal-generation/generation`。
/// `timeout_secs` 为单请求 HTTP 超时（由调用方经 spec::effective_timeout_secs
/// 解析，与策略层 deadline 同源）；POST（含响应体读取）与产物下载均感知
/// `cancel_rx`。
#[allow(clippy::too_many_arguments)]
pub(super) async fn generate_image(
    input: ImageGenerationInput,
    db: crate::agent::db::DispatcherDb,
    workspace_id: String,
    api_key: &str,
    base_url: &str,
    default_model: &str,
    timeout_secs: u64,
    cancel_rx: Option<watch::Receiver<bool>>,
) -> anyhow::Result<ImageGenerationOutput> {
    if api_key.is_empty() {
        anyhow::bail!("图片生成 API Key 未配置");
    }

    let model = input
        .model
        .clone()
        .unwrap_or_else(|| default_model.to_string());

    // 构造 size 参数，格式 "WxH"（如 "1024*1024"）
    let size = match (input.width, input.height) {
        (Some(w), Some(h)) => format!("{}*{}", w, h),
        _ => "1024*1024".to_string(),
    };

    // 可选参数按「声明才发送」透传：未声明时请求体与不带这些键的老请求体
    // 逐字节一致，避免向服务端发送显式 null。
    let mut parameters = serde_json::Map::new();
    parameters.insert("n".to_string(), json!(1));
    parameters.insert(
        "negative_prompt".to_string(),
        json!(input.negative_prompt.as_deref().unwrap_or("")),
    );
    parameters.insert("prompt_extend".to_string(), json!(true));
    parameters.insert("watermark".to_string(), json!(false));
    parameters.insert("size".to_string(), json!(size));
    if let Some(seed) = input.seed {
        parameters.insert("seed".to_string(), json!(seed));
    }
    if let Some(style) = input
        .style
        .as_deref()
        .filter(|style| !style.trim().is_empty())
    {
        parameters.insert("style".to_string(), json!(style));
    }

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
        "parameters": parameters
    });

    let client = make_client(timeout_secs).context("构建 HTTP 客户端失败")?;
    let url = multimodal_generation_endpoint(base_url);

    let response = tokio::select! {
        biased;
        _ = cancellation(cancel_rx.clone()) => anyhow::bail!("图片生成 {CANCELLED_MESSAGE}"),
        result = client
            .post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send() => result.context("图片生成 API 请求失败（网络层）")?,
    };

    if !response.status().is_success() {
        let status = response.status();
        let body = tokio::select! {
            biased;
            _ = cancellation(cancel_rx.clone()) => anyhow::bail!("图片生成 {CANCELLED_MESSAGE}"),
            result = response.text() => result
                .unwrap_or_else(|error| format!("（错误响应体读取失败：{error}）")),
        };
        anyhow::bail!(
            "图片生成 API 请求失败: {} (model={}) - 响应: {}",
            status,
            model,
            body
        );
    }

    let response_json: serde_json::Value = tokio::select! {
        biased;
        _ = cancellation(cancel_rx.clone()) => anyhow::bail!("图片生成 {CANCELLED_MESSAGE}"),
        result = response.json() => result?,
    };

    // 同步返回：直接从 choices 中提取图片 URL
    let image_url = response_json["output"]["choices"][0]["message"]["content"][0]["image"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("无法从响应中获取图片 URL: {:?}", response_json))?;

    let image_response = tokio::select! {
        biased;
        _ = cancellation(cancel_rx.clone()) => anyhow::bail!("图片生成产物下载 {CANCELLED_MESSAGE}"),
        result = client.get(image_url).send() => result?,
    };
    let image_bytes = tokio::select! {
        biased;
        _ = cancellation(cancel_rx.clone()) => anyhow::bail!("图片生成产物下载 {CANCELLED_MESSAGE}"),
        result = image_response.bytes() => result?,
    };

    let (width, height) = extract_dimensions(&response_json);
    let prompt = input.prompt;
    let saved = persist_generated_image(
        db,
        workspace_id,
        image_bytes.to_vec(),
        &prompt,
        Some(width),
        Some(height),
        cancel_rx,
    )
    .await?;

    Ok(ImageGenerationOutput {
        image_id: saved.image_id,
        width,
        height,
        generation_prompt: prompt,
    })
}

/// 规范化图片生成 API 基础地址：
/// - 先剥离 query/fragment（curl 完整 URL 常带 `?api-key=…` 等，不剥离会让
///   后缀匹配失配、拼出带 query 的非法路径）；
/// - 已含完整端点路径（用户把 curl 里的完整 URL 粘进配置）时再剥离路径部分；
/// - 确保包含 `/api/v1` 前缀。
fn resolve_image_api_base(base_url: &str) -> String {
    // 先剥离 query/fragment 再去尾斜杠：`…/api/v1/?key=x` 需要先去掉 query
    // 才能暴露出待去除的尾斜杠。
    let trimmed = base_url.split(['?', '#']).next().unwrap_or(base_url);
    let trimmed = trimmed.trim_end_matches('/');
    let base = trimmed
        .strip_suffix(MULTIMODAL_GENERATION_PATH)
        .map(|base| base.trim_end_matches('/'))
        .unwrap_or(trimmed);
    if base.ends_with("/api/v1") {
        base.to_string()
    } else {
        format!("{base}/api/v1")
    }
}

/// 生成/编辑/连通性测试共用的完整端点 URL（单一构建出处，测试与运行时
/// 不允许各自拼路径导致「测试通过、运行 404」或反之）。
pub(crate) fn multimodal_generation_endpoint(base_url: &str) -> String {
    format!(
        "{base}{MULTIMODAL_GENERATION_PATH}",
        base = resolve_image_api_base(base_url)
    )
}

/// 从响应 JSON 中提取宽度和高度（从 usage 字段）。
fn extract_dimensions(node: &serde_json::Value) -> (u32, u32) {
    let width = node["usage"]["width"].as_u64().unwrap_or(1024) as u32;
    let height = node["usage"]["height"].as_u64().unwrap_or(1024) as u32;
    (width, height)
}

/// 落盘并登记生成图（统一入口 chat_images::save_image，与用户上传共用
/// 压缩/mime 映射/目录布局/索引登记；source = tool_generate）。取消信号由
/// 调用方（工具边界）显式下传，与 HTTP 阶段同一来源，不在此重读 task-local。
async fn persist_generated_image(
    db: crate::agent::db::DispatcherDb,
    workspace_id: String,
    image_bytes: Vec<u8>,
    prompt: &str,
    width: Option<u32>,
    height: Option<u32>,
    cancel_rx: Option<watch::Receiver<bool>>,
) -> anyhow::Result<crate::chat_images::SavedChatImage> {
    crate::chat_images::save_image(
        &db,
        crate::chat_images::SaveChatImageParams {
            workspace_id: &workspace_id,
            bytes: image_bytes,
            mime_type: "image/png",
            source: "tool_generate",
            generation_prompt: Some(prompt),
            width,
            height,
        },
        cancel_rx,
    )
    .await
    .map_err(|e| anyhow::anyhow!("{e}"))
}

#[cfg(test)]
mod tests {
    use super::{
        extract_dimensions, multimodal_generation_endpoint, resolve_image_api_base,
        MULTIMODAL_GENERATION_PATH,
    };

    #[test]
    fn resolve_image_api_base_ensures_api_v1_prefix() {
        assert_eq!(
            resolve_image_api_base("https://dashscope.aliyuncs.com"),
            "https://dashscope.aliyuncs.com/api/v1"
        );
        assert_eq!(
            resolve_image_api_base("https://dashscope.aliyuncs.com/api/v1/"),
            "https://dashscope.aliyuncs.com/api/v1"
        );
        assert_eq!(
            resolve_image_api_base("https://dashscope.aliyuncs.com/api/v1"),
            "https://dashscope.aliyuncs.com/api/v1"
        );
    }

    #[test]
    fn resolve_image_api_base_strips_full_endpoint_path() {
        // 用户直接粘贴 curl 里的完整端点 URL 时，先剥离路径部分再补 /api/v1，
        // 避免二次拼接出「…/generation/api/v1/services/…」的非法路径。
        let full = format!(
            "https://llm-x.cn-beijing.maas.aliyuncs.com/api/v1{MULTIMODAL_GENERATION_PATH}"
        );
        assert_eq!(
            resolve_image_api_base(&full),
            "https://llm-x.cn-beijing.maas.aliyuncs.com/api/v1"
        );
        // 带尾斜杠的完整端点同样归一。
        assert_eq!(
            resolve_image_api_base(&format!("{full}/")),
            "https://llm-x.cn-beijing.maas.aliyuncs.com/api/v1"
        );
    }

    #[test]
    fn resolve_image_api_base_strips_query_and_fragment() {
        // curl 复制的端点常带 query（如 ?api-key=…）或 fragment：先剥离再匹配，
        // 否则后缀失配会拼出「…generation?api-key=…/api/v1/services/…」。
        assert_eq!(
            resolve_image_api_base("https://host.example.com/api/v1/?api-key=k"),
            "https://host.example.com/api/v1"
        );
        assert_eq!(
            resolve_image_api_base("https://host.example.com?api-key=k"),
            "https://host.example.com/api/v1"
        );
        assert_eq!(
            resolve_image_api_base(&format!(
                "https://host.example.com/api/v1{MULTIMODAL_GENERATION_PATH}?api-key=k#frag"
            )),
            "https://host.example.com/api/v1"
        );
    }

    #[test]
    fn multimodal_generation_endpoint_accepts_base_and_full_url() {
        const EXPECTED: &str = "https://llm-x.cn-beijing.maas.aliyuncs.com/api/v1/services/aigc/multimodal-generation/generation";
        assert_eq!(
            multimodal_generation_endpoint("https://llm-x.cn-beijing.maas.aliyuncs.com"),
            EXPECTED
        );
        assert_eq!(
            multimodal_generation_endpoint("https://llm-x.cn-beijing.maas.aliyuncs.com/api/v1"),
            EXPECTED
        );
        assert_eq!(multimodal_generation_endpoint(EXPECTED), EXPECTED);
    }

    #[test]
    fn extract_dimensions_defaults_to_1024() {
        let node = serde_json::json!({});
        assert_eq!(extract_dimensions(&node), (1024, 1024));
        let node = serde_json::json!({ "usage": { "width": 1328, "height": 1328 } });
        assert_eq!(extract_dimensions(&node), (1328, 1328));
    }
}
