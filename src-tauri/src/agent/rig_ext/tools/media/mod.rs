//! 多媒体工具组（T2.3a）：generate_image / edit_image / analyze_image /
//! fetch_image / browser_*。
//!
//! 迁移自旧自实现工具层（已随迁移删除）的 image_generation / image_edit /
//! fetch_image / analyze_image / browser（+browser/ 子模块）。差异点：
//! - 图片生成/编辑的 DashScope 直连 HTTP 逻辑整体迁入 `image_api`
//!   （旧 `crate::tools::image_generator` 随旧工具层一并退役），产物落盘
//!   仍走唯一入口 `crate::chat_images::save_image`；
//! - 视觉调用由旧 `OpenAiCompatProvider` 改为 `deps.vision_spec` +
//!   `rig_ext::model::completions_model` 构建 rig 模型发 completion 请求；
//! - db 句柄由旧「app_handle → DispatcherState」改为 `deps.db` 直接注入。

mod analyze;
mod browser;
mod fetch;
mod generate;
mod image_api;

use rig::completion::{CompletionModel, Message};
use rig::message::{AssistantContent, DocumentSourceKind, Image, ImageMediaType, UserContent};
use rig::tool::PortableDynamicTool;

use super::super::model::{build_completion_request, completions_model, PurposeModelSpec};
use super::RigToolDeps;

pub(crate) fn media_tools(deps: &RigToolDeps) -> Vec<PortableDynamicTool> {
    let mut tools = vec![
        generate::generate_image_tool(deps),
        generate::edit_image_tool(deps),
        analyze::analyze_image_tool(deps),
        fetch::fetch_image_tool(deps),
    ];
    tools.extend(browser::browser_tools(deps));
    tools
}

/// 协作式取消检查（语义同旧 `agent::common::cancellation_requested`）：
/// 已置位，或发送端已 drop（has_changed 报错 = 通道关闭，按取消处理）。
fn cancellation_requested(cancel_rx: &tokio::sync::watch::Receiver<bool>) -> bool {
    *cancel_rx.borrow() || cancel_rx.has_changed().is_err()
}

/// mime 字符串 → rig `ImageMediaType`（对齐 `rig_ext::message` 私有同名表；
/// 该函数不导出，本层按图片工具实际支持的四种格式维护副本）。
fn image_media_type_for_mime(mime: &str) -> Option<ImageMediaType> {
    match mime.to_ascii_lowercase().as_str() {
        "image/png" => Some(ImageMediaType::PNG),
        "image/jpeg" => Some(ImageMediaType::JPEG),
        "image/gif" => Some(ImageMediaType::GIF),
        "image/webp" => Some(ImageMediaType::WEBP),
        _ => None,
    }
}

/// `data:image/...;base64,...` → rig `Image`（校验口径同 `rig_ext::message`
/// 的 data URL 解析：必须 image 前缀 + base64 段 + 受支持类型）。
fn data_url_to_image(data_url: &str) -> Result<Image, String> {
    let Some(rest) = data_url.strip_prefix("data:image/") else {
        return Err("data URL 必须以 data:image/ 开头".to_string());
    };
    let Some((mime, data)) = rest.split_once(";base64,") else {
        return Err("data URL 缺少 ;base64, 数据段".to_string());
    };
    let media_type = image_media_type_for_mime(&format!("image/{mime}"))
        .ok_or_else(|| format!("不支持的图片类型：image/{mime}"))?;
    Ok(Image {
        data: DocumentSourceKind::Base64(data.to_string()),
        media_type: Some(media_type),
        detail: None,
        additional_params: None,
    })
}

/// 视觉槽位一次性图片问答：系统提示词 + 用户指令 + 单张图片 → 文本结果。
/// analyze_image 与 browser_visual_analyze 共用；错误消息带「错误：」前缀，
/// `failure_label` 区分两个调用方的旧文案（"视觉模型分析" /
/// "视觉模型分析网页截图"）。返回内容为 trim 后的拼接文本（可能为空，
/// 空结果的报错文案由调用方按旧语义自定）。
async fn vision_complete_image(
    spec: &PurposeModelSpec,
    system_prompt: &str,
    user_text: String,
    image: Image,
    timeout: Option<std::time::Duration>,
    failure_label: &str,
) -> Result<String, String> {
    let model = completions_model(spec)
        .map_err(|error| format!("错误：构建视觉模型客户端失败：{error:#}"))?;
    let request = build_completion_request(
        Some(system_prompt.to_string()),
        vec![Message::User {
            content: vec![UserContent::text(user_text), UserContent::Image(image)],
        }],
        Vec::new(),
        spec.max_tokens,
        spec.temperature,
        spec.enable_thinking,
    );
    let response = match timeout {
        Some(limit) => match tokio::time::timeout(limit, model.completion(request)).await {
            Ok(Ok(response)) => response,
            Ok(Err(error)) => return Err(format!("错误：{failure_label}失败：{error}")),
            Err(_) => {
                return Err(format!(
                    "错误：{failure_label}超时（{} 秒）",
                    limit.as_secs()
                ))
            }
        },
        None => match model.completion(request).await {
            Ok(response) => response,
            Err(error) => return Err(format!("错误：{failure_label}失败：{error}")),
        },
    };
    let content = response
        .choice
        .iter()
        .filter_map(|item| match item {
            AssistantContent::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");
    Ok(content.trim().to_string())
}
