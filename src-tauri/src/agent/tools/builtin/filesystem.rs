use std::fs;

use async_trait::async_trait;
use serde_json::{json, Value};

use super::common::{boolish_arg, resolve_path, string_arg, with_compression_parameters};
use crate::agent::tools::context::ToolContext;
use crate::agent::tools::registry::AgentTool;
use crate::agent::tools::ToolResult;

mod list_dir;
mod read_file;

pub(super) fn read_file_tool() -> Box<dyn AgentTool> {
    read_file::read_file_tool()
}

pub(super) fn write_file_tool() -> Box<dyn AgentTool> {
    Box::new(WriteFileTool)
}

pub(super) fn edit_file_tool() -> Box<dyn AgentTool> {
    Box::new(EditFileTool)
}

pub(super) fn list_dir_tool() -> Box<dyn AgentTool> {
    list_dir::list_dir_tool()
}

struct WriteFileTool;
struct EditFileTool;

#[async_trait]
impl AgentTool for WriteFileTool {
    fn name(&self) -> &'static str {
        "write_file"
    }

    fn description(&self) -> &'static str {
        "将内容写入文件。若文件已存在则覆盖；必要时自动创建父目录。仅适合小范围修改或生成文件。通常保持默认 compress=false 即可。"
    }

    fn parameters(&self) -> Value {
        with_compression_parameters(
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "要写入的文件路径" },
                    "content": { "type": "string", "description": "要写入的内容" }
                },
                "required": ["path", "content"]
            }),
            false,
            "写入工具通常只返回简短确认信息，默认关闭压缩。",
        )
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> ToolResult {
        ToolResult::from_text(
            async {
                let Some(path) = string_arg(args, "path") else {
                    return "错误：缺少必填参数 path".to_string();
                };
                let Some(content) = string_arg(args, "content") else {
                    return "错误：缺少必填参数 content".to_string();
                };

                let context = context.clone();
                match tokio::task::spawn_blocking(move || {
                    let file_path = match resolve_path(&context, &path) {
                        Ok(path) => path,
                        Err(message) => return message,
                    };
                    if let Some(parent) = file_path.parent() {
                        if let Err(error) = fs::create_dir_all(parent) {
                            return format!("错误：创建父目录失败：{error}");
                        }
                    }
                    match fs::write(&file_path, &content) {
                        Ok(()) => format!(
                            "写入成功：{} 字符 -> {}",
                            content.chars().count(),
                            file_path.display()
                        ),
                        Err(error) => format!("错误：写入文件失败：{error}"),
                    }
                })
                .await
                {
                    Ok(output) => output,
                    Err(error) => format!("错误：写入文件任务失败：{error}"),
                }
            }
            .await,
        )
    }
}

#[async_trait]
impl AgentTool for EditFileTool {
    fn name(&self) -> &'static str {
        "edit_file"
    }

    fn description(&self) -> &'static str {
        "通过将 old_text 替换为 new_text 来编辑文件。如果 old_text 命中多处，请补充上下文或设置 replace_all=true。通常保持默认 compress=false 即可。"
    }

    fn parameters(&self) -> Value {
        with_compression_parameters(
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "要编辑的文件路径" },
                    "old_text": { "type": "string", "description": "要查找并替换的原文本" },
                    "new_text": { "type": "string", "description": "替换后的新文本" },
                    "replace_all": { "type": "boolean", "description": "是否替换全部命中项，默认 false", "default": false }
                },
                "required": ["path", "old_text", "new_text"]
            }),
            false,
            "编辑工具通常只返回简短确认信息，默认关闭压缩。",
        )
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> ToolResult {
        ToolResult::from_text(
            async {
                let Some(path) = string_arg(args, "path") else {
                    return "错误：缺少必填参数 path".to_string();
                };
                let Some(old_text) = string_arg(args, "old_text") else {
                    return "错误：缺少必填参数 old_text".to_string();
                };
                let Some(new_text) = string_arg(args, "new_text") else {
                    return "错误：缺少必填参数 new_text".to_string();
                };
                if old_text.is_empty() {
                    // 空 old_text 必须显式拦截：`content.replace("", new)` 会在每个字符边界
                    // 插入 new_text 严重破坏文件；空文件场景还能绕过“命中多处”检查。
                    return "错误：old_text 不能为空字符串".to_string();
                }
                if old_text == new_text {
                    return "编辑成功（无变化：old_text 与 new_text 相同）".to_string();
                }
                let replace_all = boolish_arg(args, "replace_all").unwrap_or(false);

                let context = context.clone();
                match tokio::task::spawn_blocking(move || {
                    edit_file_checked(&context, &path, &old_text, &new_text, replace_all)
                })
                .await
                {
                    Ok(output) => output,
                    Err(error) => format!("错误：编辑文件任务失败：{error}"),
                }
            }
            .await,
        )
    }
}

/// 读-改-写的并发安全实现：读取前后复检文件大小与 mtime，窗口内被并发修改
/// （如 LLM 并行发起多次 edit_file）时重读重试；重试耗尽则放弃写入，
/// 绝不使用过期快照覆盖其他改动（fail-closed）。
fn edit_file_checked(
    context: &ToolContext,
    path: &str,
    old_text: &str,
    new_text: &str,
    replace_all: bool,
) -> String {
    const MAX_EDIT_ATTEMPTS: usize = 3;

    let file_path = match resolve_path(context, path) {
        Ok(path) => path,
        Err(message) => return message,
    };

    for attempt in 1..=MAX_EDIT_ATTEMPTS {
        let before = match fs::metadata(&file_path) {
            Ok(meta) => meta,
            Err(_) => return format!("错误：文件不存在或不可读：{path}"),
        };
        let before_len = before.len();
        let before_modified = before.modified().ok();

        let Ok(content) = fs::read_to_string(&file_path) else {
            return format!("错误：文件不存在或不可读：{path}");
        };
        if !content.contains(old_text) {
            return format!("错误：在 {path} 中未找到 old_text");
        }
        if !replace_all && content.matches(old_text).count() > 1 {
            return "错误：old_text 命中多处，请补充上下文或设置 replace_all=true".to_string();
        }

        let updated = if replace_all {
            content.replace(old_text, new_text)
        } else {
            content.replacen(old_text, new_text, 1)
        };

        let concurrently_modified = match fs::metadata(&file_path) {
            Ok(after) => {
                after.len() != before_len
                    || match (before_modified, after.modified().ok()) {
                        (Some(left), Some(right)) => left != right,
                        // mtime 不可用时保守视为已变化
                        _ => true,
                    }
            }
            Err(error) => return format!("错误：写入前读取文件元数据失败：{error}"),
        };
        if concurrently_modified {
            if attempt < MAX_EDIT_ATTEMPTS {
                continue;
            }
            return format!("错误：{path} 在编辑期间被并发修改，已放弃写入以避免覆盖其他改动");
        }

        return match fs::write(&file_path, updated) {
            Ok(()) => format!("编辑成功：{}", file_path.display()),
            Err(error) => format!("错误：编辑文件失败：{error}"),
        };
    }
    // 循环每一轮都必然 return；此分支仅用于满足类型检查。
    format!("错误：{path} 编辑重试循环意外退出")
}
