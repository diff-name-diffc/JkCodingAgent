use async_trait::async_trait;
use serde_json::{json, Value};

use super::common::{bounded_dimension_arg, string_arg, u64_arg};
use crate::agent::tools::context::ToolContext;
use crate::agent::tools::registry::AgentTool;
use crate::agent::tools::ToolResult;
use crate::tools::image_generator::{generate_image, ImageGenerationInput};

pub(super) fn generate_image_tool() -> Box<dyn AgentTool> {
    Box::new(GenerateImageTool)
}

struct GenerateImageTool;

#[async_trait]
impl AgentTool for GenerateImageTool {
    fn name(&self) -> &'static str {
        "generate_image"
    }

    fn description(&self) -> &'static str {
        "根据文本描述生成图片。支持指定尺寸、风格等参数。调用外部图片生成模型（如 qwen-image-2.0-pro）生成图片，保存到本地后返回路径。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "prompt": { "type": "string", "description": "图片描述文本，详细描述要生成的图片内容" },
                "width": { "type": "integer", "description": "图片宽度（可选，支持范围 256-4096）" },
                "height": { "type": "integer", "description": "图片高度（可选，支持范围 256-4096）" },
                "style": { "type": "string", "description": "图片风格（可选）" },
                "negative_prompt": { "type": "string", "description": "负面提示词，指定不希望在图片中出现的内容（可选）" },
                "model": { "type": "string", "description": "使用的图片生成模型名称（可选，默认使用配置中的模型）" },
                "seed": { "type": "integer", "description": "随机种子（可选）" }
            },
            "required": ["prompt"]
        })
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> ToolResult {
        ToolResult::from_text(execute_image_generation(args, context).await)
    }
}

async fn execute_image_generation(args: &Value, context: &ToolContext) -> String {
    let Some(prompt) = string_arg(args, "prompt") else {
        return "错误：缺少必填参数 prompt".to_string();
    };

    // width/height 做范围校验（256-4096）而非 u64→u32 静默截断，
    // 非法值直接报「错误：」，避免把超大/零尺寸原样传给外部模型。
    let width = match bounded_dimension_arg(args, "width") {
        Ok(value) => value,
        Err(message) => return message,
    };
    let height = match bounded_dimension_arg(args, "height") {
        Ok(value) => value,
        Err(message) => return message,
    };
    let style = string_arg(args, "style");
    let negative_prompt = string_arg(args, "negative_prompt");
    let model = string_arg(args, "model");
    let seed = u64_arg(args, "seed");

    // image_name 参数已移除：底层落盘文件名固定为 {uuid}.png（用于
    // chat-image://uuid 反查），image_name 从未被使用，保留会误导调用方。
    let input = ImageGenerationInput {
        prompt,
        image_name: None,
        width,
        height,
        style,
        negative_prompt,
        model,
        seed,
    };

    let api_key = &context.image_model_api_key;
    let base_url = &context.image_model_url;
    let default_model = &context.image_model;

    if api_key.is_empty() {
        return "错误：图片生成 API Key 未配置，请先在设置中配置".to_string();
    }

    match generate_image(
        input,
        context.session_title.clone(),
        api_key,
        base_url,
        default_model,
    )
    .await
    {
        Ok(output) => {
            let ref_uri = format!("chat-image://{}", output.image_id);
            format!(
                    "图片生成成功！尺寸 {}x{}，提示词：{}\n\n如需在回答中展示该图片，请使用：\n![图片描述]({})",
                    output.width, output.height, output.generation_prompt, ref_uri
                )
        }
        Err(e) => format!("错误：图片生成失败：{}", e),
    }
}
