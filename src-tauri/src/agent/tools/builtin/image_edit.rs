use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::PathBuf;

use super::common::{resolve_path, string_arg};
use crate::agent::tools::context::ToolContext;
use crate::agent::tools::registry::AgentTool;
use crate::chat_images::{is_chat_image_path, resolve_chat_image_id_async};
use crate::tools::image_generator::edit_image;

pub(super) fn edit_image_tool() -> Box<dyn AgentTool> {
    Box::new(EditImageTool)
}

struct EditImageTool;

#[async_trait]
impl AgentTool for EditImageTool {
    fn name(&self) -> &'static str {
        "edit_image"
    }

    fn description(&self) -> &'static str {
        "根据用户提供的图片引用和编辑描述，对图片进行编辑（如修改风格、添加元素、调整细节等）。支持 chat-image://uuid 协议引用、本地绝对路径或相对路径。支持指定输出尺寸。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "image_path": { "type": "string", "description": "要编辑的图片引用。支持：chat-image://uuid（对话中图片引用）、本地绝对路径、相对工作区路径" },
                "prompt": { "type": "string", "description": "编辑描述文本，详细描述要进行的修改" },
                "image_name": { "type": "string", "description": "输出图片文件名（可选，不含扩展名）。用于生成可读的文件名" },
                "width": { "type": "integer", "description": "输出图片宽度（可选）" },
                "height": { "type": "integer", "description": "输出图片高度（可选）" }
            },
            "required": ["image_path", "prompt"]
        })
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
        let Some(raw_image_path) = string_arg(args, "image_path") else {
            return "错误：缺少必填参数 image_path".to_string();
        };

        let Some(prompt) = string_arg(args, "prompt") else {
            return "错误：缺少必填参数 prompt".to_string();
        };

        let image_path = if raw_image_path.starts_with("chat-image://") {
            match resolve_chat_image_id_async(raw_image_path.clone()).await {
                Ok(p) => p,
                Err(e) => return format!("错误：无法解析 chat-image 协议引用：{}", e),
            }
        } else {
            let stripped = raw_image_path
                .strip_prefix("file://")
                .unwrap_or(&raw_image_path);

            let raw_path_buf = PathBuf::from(stripped);
            if is_chat_image_path(&raw_path_buf) {
                raw_path_buf
            } else {
                match resolve_path(context, stripped) {
                    Ok(p) => p,
                    Err(e) => return e,
                }
            }
        };

        if !image_path.exists() {
            return format!("错误：图片文件不存在：{}", image_path.display());
        }

        let image_name = string_arg(args, "image_name");
        let width = args.get("width").and_then(|v| v.as_u64().map(|v| v as u32));
        let height = args
            .get("height")
            .and_then(|v| v.as_u64().map(|v| v as u32));

        let api_key = &context.image_model_api_key;
        let base_url = &context.image_model_url;
        let default_model = if context.image_edit_model.trim().is_empty() {
            &context.image_model
        } else {
            &context.image_edit_model
        };

        if api_key.is_empty() {
            return "错误：图片编辑 API Key 未配置，请先在设置中配置".to_string();
        }

        match edit_image(
            image_path.to_str().unwrap_or(&raw_image_path),
            prompt,
            image_name,
            width,
            height,
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
                    "图片编辑成功！尺寸 {}x{}，编辑描述：{}\n\n如需在回答中展示该图片，请使用：\n![图片描述]({})",
                    output.width, output.height, output.generation_prompt, ref_uri
                )
            }
            Err(e) => format!("图片编辑失败：{}", e),
        }
    }
}
