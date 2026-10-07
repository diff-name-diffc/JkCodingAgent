//! generate_image / edit_image 工具（DashScope 直连 HTTP，落盘走
//! `chat_images::save_image`；产物以 `chat-image://{image_id}` 引用回显）。

use std::path::PathBuf;

use rig::tool::{PortableDynamicTool, ToolExecutionError, ToolOutput};
use serde_json::{json, Value};

use super::super::common::{
    bounded_dimension_arg, resolve_path, string_arg, u64_arg, with_call_timeout_parameters,
};
use super::super::deps::RigToolDeps;
use super::image_api::{self, ImageGenerationInput};
use crate::agent::rig_ext::tools::spec::{
    effective_timeout_secs, IMAGE_TOOL_CALL_TIMEOUT_RANGE, IMAGE_TOOL_TIMEOUT_SECS,
};
use crate::chat_images::{is_chat_image_path, resolve_chat_image_id_async};

pub(super) fn generate_image_tool(deps: &RigToolDeps) -> PortableDynamicTool {
    let deps = deps.clone();
    let parameters = with_call_timeout_parameters(
        json!({
            "type": "object",
            "properties": {
                "prompt": { "type": "string", "description": "图片描述文本，详细描述要生成的图片内容（场景、构图、配色、风格）。若图片需要包含文字（标题、标签、流程节点名、注释等），必须逐处写清文字的具体内容，并显式要求这些文字全部使用简体中文（例如：图中标题为「检索增强生成流程」，所有节点标签均为简体中文）；仅当用户明确要求其他语言时才使用对应语言，且同样需在 prompt 中显式声明" },
                "width": { "type": "integer", "description": "图片宽度（可选，支持范围 256-4096）" },
                "height": { "type": "integer", "description": "图片高度（可选，支持范围 256-4096）" },
                "style": { "type": "string", "description": "图片风格（可选）" },
                "negative_prompt": { "type": "string", "description": "负面提示词，指定不希望在图片中出现的内容（可选）。当图中文字要求为简体中文时，建议加入「英文文字、乱码文字」避免混入非中文文字" },
                "model": { "type": "string", "description": "使用的图片生成模型名称（可选，默认使用配置中的模型）" },
                "seed": { "type": "integer", "description": "随机种子（可选）" }
            },
            "required": ["prompt"]
        }),
        IMAGE_TOOL_CALL_TIMEOUT_RANGE,
        // 描述文案里的「默认」须与生效默认一致（用户配置 ?? 表默认），
        // 否则模型对预算的心智模型失真。
        deps.tool_timeouts
            .generate_image
            .unwrap_or(IMAGE_TOOL_TIMEOUT_SECS),
        "复杂提示词、大尺寸或 prompt_expand 扩写生成明显偏慢时可声明更长；常规生成无需填写。",
    );
    PortableDynamicTool::new(
        "generate_image",
        "根据文本描述生成图片。支持指定尺寸、风格等参数。调用外部图片生成模型（如 qwen-image-2.0-pro）生成图片，保存到本地后返回路径。语言要求：图片中出现的所有文字（标题、标签、流程节点名、注释、界面文案等）默认必须全部为简体中文——撰写 prompt 时必须把图中每处文字的具体中文内容写清楚，并在 prompt 中显式声明「图片中所有文字均使用简体中文」；仅当用户明确要求其他语言时才可使用对应语言（同样需显式声明）。",
        parameters,
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

    // 取消信号在工具边界（task-local 作用域）读取一次后显式下传；无
    // task-local（脱离循环的调用）时为 None，按无取消源处理。
    let cancel_rx = crate::agent::rig_ext::r#loop::invocation::ToolInvocationContext::current()
        .map(|context| context.cancel_rx);
    // 预算与策略层 deadline 同源（spec::effective_timeout_secs）：调用声明 >
    // 用户配置默认（deps.tool_timeouts）> 策略表默认，防止两处口径漂移。
    let timeout_secs = effective_timeout_secs(
        "generate_image",
        u64_arg(args, "timeout_secs"),
        deps.tool_timeouts.generate_image,
    );

    match image_api::generate_image(
        input,
        deps.db.clone(),
        deps.workspace_id.clone(),
        &config.api_key,
        &config.url,
        &config.model,
        timeout_secs,
        cancel_rx,
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
    let parameters = with_call_timeout_parameters(
        json!({
            "type": "object",
            "properties": {
                "image_path": { "type": "string", "description": "要编辑的图片引用。支持：chat-image://uuid（对话中图片引用）、本地绝对路径、相对工作区路径" },
                "prompt": { "type": "string", "description": "编辑描述文本，详细描述要进行的修改。涉及图中文字（新增、修改、翻译）时必须写清文字的具体内容；图中文字默认保持/使用简体中文，需在 prompt 中显式声明（仅当用户明确要求其他语言时除外）" },
                "width": { "type": "integer", "description": "输出图片宽度（可选，支持范围 256-4096）" },
                "height": { "type": "integer", "description": "输出图片高度（可选，支持范围 256-4096）" }
            },
            "required": ["image_path", "prompt"]
        }),
        IMAGE_TOOL_CALL_TIMEOUT_RANGE,
        // 描述文案里的「默认」须与生效默认一致（用户配置 ?? 表默认）。
        deps.tool_timeouts
            .edit_image
            .unwrap_or(IMAGE_TOOL_TIMEOUT_SECS),
        "编辑大图或修改幅度较大时生成偏慢，可声明更长；常规编辑无需填写。",
    );
    PortableDynamicTool::new(
        "edit_image",
        "根据用户提供的图片引用和编辑描述，对图片进行编辑（如修改风格、添加元素、调整细节等）。支持 chat-image://uuid 协议引用、本地绝对路径或相对路径。支持指定输出尺寸。语言要求：编辑涉及图中文字（新增、修改、翻译）时，目标文字默认必须为简体中文，必须在 prompt 中写清具体中文内容并显式声明语言要求；仅当用户明确要求其他语言时除外。",
        parameters,
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

    // 取消信号与超时预算与 generate_image 同构（见 execute_image_generation）。
    let cancel_rx = crate::agent::rig_ext::r#loop::invocation::ToolInvocationContext::current()
        .map(|context| context.cancel_rx);
    let timeout_secs = effective_timeout_secs(
        "edit_image",
        u64_arg(args, "timeout_secs"),
        deps.tool_timeouts.edit_image,
    );

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
        timeout_secs,
        cancel_rx,
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
