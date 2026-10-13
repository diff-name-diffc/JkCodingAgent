//! 多媒体工具组（T2.3a）：generate_image / edit_image / analyze_image /
//! fetch_image / browser_open_url。
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
/// 图片生成/编辑的 DashScope 直连 HTTP 层。对 `agent::commands`（设置中心
/// 连通性测试）开放，共用端点构建与超时，避免测试与运行时口径漂移。
pub(crate) mod image_api;

use rig::completion::{CompletionModel, Message};
use rig::message::{AssistantContent, Image, UserContent};
use rig::tool::PortableDynamicTool;

use super::super::model::{build_completion_request, completions_model, PurposeModelSpec};
use super::RigToolDeps;
use crate::agent::common::cancellation_requested;

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

/// 视觉槽位一次性图片问答：供 analyze_image 调用，返回纯文本或明确错误。
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
