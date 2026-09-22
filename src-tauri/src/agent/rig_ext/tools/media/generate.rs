//! generate_image / edit_image 工具（DashScope 直连 HTTP，落盘走
//! `chat_images::save_image`；产物以 `chat-image://{image_id}` 引用回显）。

use std::path::PathBuf;

use rig::tool::{PortableDynamicTool, ToolExecutionError, ToolOutput};
use serde_json::{json, Value};

use super::super::common::{bounded_dimension_arg, resolve_path, string_arg, u64_arg};
use super::super::deps::RigToolDeps;
use super::image_api::{self, ImageGenerationInput};
use crate::chat_images::{is_chat_image_path, resolve_chat_image_id_async};

pub(super) fn generate_image_tool(deps: &RigToolDeps) -> PortableDynamicTool {
    let deps = deps.clone();
    PortableDynamicTool::new(
        "generate_image",
        "根据文本描述生成图片。支持指定尺寸、风格等参数。调用外部图片生成模型（如 qwen-image-2.0-pro）生成图片，保存到本地后返回路径。",
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
        }),
        move |args: Value| {
            let deps = deps.clone();
            Box::pin(async move { execute_image_generation(&args, &deps).await })
        },
    )
}

async fn execute_image_generation(
    args: &Value,
    deps: &RigToolDeps,
) -> Result<ToolOutput, ToolExecutionError> {
    let Some(prompt) = string_arg(args, "prompt") else {
        return Err(ToolExecutionError::invalid_args(
            "错误：缺少必填参数 prompt",
        ));
    };

    // width/height 做范围校验（256-4096）而非 u64→u32 静默截断，
    // 非法值直接报「错误：」，避免把超大/零尺寸原样传给外部模型。
    let width = bounded_dimension_arg(args, "width").map_err(ToolExecutionError::invalid_args)?;
    let height = bounded_dimension_arg(args, "height").map_err(ToolExecutionError::invalid_args)?;
    let style = string_arg(args, "style");
    let negative_prompt = string_arg(args, "negative_prompt");
    let model = string_arg(args, "model");
    let seed = u64_arg(args, "seed");

    // image_name 参数已移除：底层落盘文件名固定为 {uuid}.png（用于
    // chat-image://uuid 反查），image_name 从未被使用，保留会误导调用方。
    let input = ImageGenerationInput {
        prompt,
        width,
        height,
        style,
        negative_prompt,
        model,
        seed,
    };

    let config = &deps.image;
    if config.api_key.is_empty() {
        return Err(ToolExecutionError::other(
            "错误：图片生成 API Key 未配置，请先在设置中配置",
        ));
    }

    match image_api::generate_image(
        input,
        deps.db.clone(),
        deps.workspace_id.clone(),
        &config.api_key,
        &config.url,
        &config.model,
    )
    .await
    {
        Ok(output) => {
            let ref_uri = format!("chat-image://{}", output.image_id);
            Ok(ToolOutput::text(format!(
                "图片生成成功！尺寸 {}x{}，提示词：{}\n\n如需在回答中展示该图片，请使用：\n![图片描述]({})",
                output.width, output.height, output.generation_prompt, ref_uri
            )))
        }
        Err(e) => Err(ToolExecutionError::other(format!(
            "错误：图片生成失败：{e}"
        ))),
    }
}

pub(super) fn edit_image_tool(deps: &RigToolDeps) -> PortableDynamicTool {
    let deps = deps.clone();
    PortableDynamicTool::new(
        "edit_image",
        "根据用户提供的图片引用和编辑描述，对图片进行编辑（如修改风格、添加元素、调整细节等）。支持 chat-image://uuid 协议引用、本地绝对路径或相对路径。支持指定输出尺寸。",
        json!({
            "type": "object",
            "properties": {
                "image_path": { "type": "string", "description": "要编辑的图片引用。支持：chat-image://uuid（对话中图片引用）、本地绝对路径、相对工作区路径" },
                "prompt": { "type": "string", "description": "编辑描述文本，详细描述要进行的修改" },
                "width": { "type": "integer", "description": "输出图片宽度（可选，支持范围 256-4096）" },
                "height": { "type": "integer", "description": "输出图片高度（可选，支持范围 256-4096）" }
            },
            "required": ["image_path", "prompt"]
        }),
        move |args: Value| {
            let deps = deps.clone();
            Box::pin(async move { execute_image_edit(&args, &deps).await })
        },
    )
}

async fn execute_image_edit(
    args: &Value,
    deps: &RigToolDeps,
) -> Result<ToolOutput, ToolExecutionError> {
    let Some(raw_image_path) = string_arg(args, "image_path") else {
        return Err(ToolExecutionError::invalid_args(
            "错误：缺少必填参数 image_path",
        ));
    };

    let Some(prompt) = string_arg(args, "prompt") else {
        return Err(ToolExecutionError::invalid_args(
            "错误：缺少必填参数 prompt",
        ));
    };

    let image_path = if raw_image_path.starts_with("chat-image://") {
        match resolve_chat_image_id_async(raw_image_path.clone()).await {
            Ok(p) => p,
            Err(e) => {
                return Err(ToolExecutionError::other(format!(
                    "错误：无法解析 chat-image 协议引用：{e}"
                )))
            }
        }
    } else {
        let stripped = raw_image_path
            .strip_prefix("file://")
            .unwrap_or(&raw_image_path)
            .to_string();
        let workspace = deps.workspace.clone();
        let restrict = deps.restrict_to_workspace;
        let extra_dirs = deps.extra_allowed_dirs.clone();
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
            resolve_path(&workspace, restrict, &extra_dirs, &stripped)
        })
        .await
        {
            Ok(Ok(p)) => p,
            Ok(Err(e)) => return Err(ToolExecutionError::other(e)),
            Err(e) => {
                return Err(ToolExecutionError::other(format!(
                    "错误：解析图片路径任务失败：{e}"
                )))
            }
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
        return Err(ToolExecutionError::other(format!(
            "错误：图片文件不存在：{}",
            image_path.display()
        )));
    }

    // width/height 做范围校验（256-4096）而非 u64→u32 静默截断，与 generate_image 一致。
    let width = bounded_dimension_arg(args, "width").map_err(ToolExecutionError::invalid_args)?;
    let height = bounded_dimension_arg(args, "height").map_err(ToolExecutionError::invalid_args)?;

    let config = &deps.image;
    let default_model = if config.edit_model.trim().is_empty() {
        &config.model
    } else {
        &config.edit_model
    };

    if config.api_key.is_empty() {
        return Err(ToolExecutionError::other(
            "错误：图片编辑 API Key 未配置，请先在设置中配置",
        ));
    }

    // 路径必须可表示为 UTF-8 才能交给下层；解析失败时显式报错，
    // 不能回退到用户原始输入（那会绕过 resolve_path 的工作区校验）。
    let Some(image_path_str) = image_path.to_str() else {
        return Err(ToolExecutionError::other(
            "错误：图片路径包含非 UTF-8 字符，无法处理",
        ));
    };
    let image_path_str = image_path_str.to_string();

    match image_api::edit_image(
        &image_path_str,
        prompt,
        width,
        height,
        deps.db.clone(),
        deps.workspace_id.clone(),
        &config.api_key,
        &config.url,
        default_model,
    )
    .await
    {
        Ok(output) => {
            let ref_uri = format!("chat-image://{}", output.image_id);
            Ok(ToolOutput::text(format!(
                "图片编辑成功！尺寸 {}x{}，编辑描述：{}\n\n如需在回答中展示该图片，请使用：\n![图片描述]({})",
                output.width, output.height, output.generation_prompt, ref_uri
            )))
        }
        Err(e) => Err(ToolExecutionError::other(format!(
            "错误：图片编辑失败：{e}"
        ))),
    }
}
