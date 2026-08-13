use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::PathBuf;

use super::common::{bounded_dimension_arg, resolve_path, string_arg};
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
                "width": { "type": "integer", "description": "输出图片宽度（可选，支持范围 256-4096）" },
                "height": { "type": "integer", "description": "输出图片高度（可选，支持范围 256-4096）" }
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
                .unwrap_or(&raw_image_path)
                .to_string();
            let ctx = context.clone();
            // is_chat_image_path / resolve_path 内部含同步文件系统 I/O
            // （canonicalize / symlink_metadata），移入 spawn_blocking，
            // 避免阻塞 Tokio 工作线程。
            match tokio::task::spawn_blocking(move || {
                let raw_path_buf = PathBuf::from(&stripped);
                if is_chat_image_path(&raw_path_buf) {
                    // is_chat_image_path 内部会 canonicalize（或词法归一化）后校验路径必须
                    // 位于应用托管的可信目录 ~/.jkcodingagent/chat-images/ 之内，因此这里
                    // 直接使用解析后的路径是安全的——它属于受信任目录白名单，而非绕过校验。
                    return Ok(raw_path_buf);
                }
                resolve_path(&ctx, &stripped)
            })
            .await
            {
                Ok(Ok(p)) => p,
                Ok(Err(e)) => return e,
                Err(e) => return format!("错误：解析图片路径任务失败：{e}"),
            }
        };

        // 存在性检查同样是同步文件系统 I/O，移入 spawn_blocking。
        let image_path_exists = {
            let p = image_path.clone();
            tokio::task::spawn_blocking(move || p.exists())
                .await
                .unwrap_or(false)
        };
        if !image_path_exists {
            return format!("错误：图片文件不存在：{}", image_path.display());
        }

        let image_name = string_arg(args, "image_name");
        // width/height 做范围校验（256-4096）而非 u64→u32 静默截断，与 generate_image 一致。
        let width = match bounded_dimension_arg(args, "width") {
            Ok(value) => value,
            Err(message) => return message,
        };
        let height = match bounded_dimension_arg(args, "height") {
            Ok(value) => value,
            Err(message) => return message,
        };

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

        // 路径必须可表示为 UTF-8 才能交给下层；解析失败时显式报错，
        // 不能回退到用户原始输入（那会绕过 resolve_path 的工作区校验）。
        let Some(image_path_str) = image_path.to_str() else {
            return "错误：图片路径包含非 UTF-8 字符，无法处理".to_string();
        };

        match edit_image(
            image_path_str,
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
            Err(e) => format!("错误：图片编辑失败：{}", e),
        }
    }
}
