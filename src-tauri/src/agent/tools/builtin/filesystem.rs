use std::fs;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::{
    task,
    time::{timeout, Duration},
};

use super::common::{
    boolish_arg, collect_entries, non_empty_string_array_arg, render_labeled_sections,
    resolve_path, string_arg, usize_arg, with_compression_parameters,
};
use crate::agent::tools::context::ToolContext;
use crate::agent::tools::registry::AgentTool;

pub(super) fn read_file_tool() -> Box<dyn AgentTool> {
    Box::new(ReadFileTool)
}

pub(super) fn write_file_tool() -> Box<dyn AgentTool> {
    Box::new(WriteFileTool)
}

pub(super) fn edit_file_tool() -> Box<dyn AgentTool> {
    Box::new(EditFileTool)
}

pub(super) fn list_dir_tool() -> Box<dyn AgentTool> {
    Box::new(ListDirTool)
}

struct ReadFileTool;
struct WriteFileTool;
struct EditFileTool;
struct ListDirTool;

const FILE_IO_TIMEOUT_SECS: u64 = 30;

#[async_trait]
impl AgentTool for ReadFileTool {
    fn name(&self) -> &'static str {
        "read_file"
    }

    fn description(&self) -> &'static str {
        "读取文本文件，输出格式为 行号|内容。分析代码时优先使用；大文件请配合 offset 和 limit 分段读取。默认关闭压缩保留完整结果，若只需定位关键符号可开启 compress=true 并写明 compress_intent。"
    }

    fn parameters(&self) -> Value {
        with_compression_parameters(
            json!({
                "type": "object",
                "properties": {
                    "paths": {
                        "type": "array",
                        "description": "要读取的文件路径列表。即使只读取一个文件，也必须传单元素数组。传入多个路径时，结果会按文件路径分段返回。",
                        "minItems": 1,
                        "items": { "type": "string" }
                    },
                    "offset": { "type": "integer", "description": "起始行号，从 1 开始，默认 1" , "minimum": 1 },
                    "limit": { "type": "integer", "description": "最多读取多少行，默认 2000", "minimum": 1 }
                },
                "required": ["paths"]
            }),
            false,
            "分析代码、配置或精确文本时保持关闭保留完整结果；只定位关键符号或需要概览时可开启并写明 compress_intent。",
        )
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
        let Some(paths) = non_empty_string_array_arg(args, "paths") else {
            return "错误：缺少必填参数 paths，且 paths 必须是非空字符串数组".to_string();
        };
        let offset = usize_arg(args, "offset").unwrap_or(1).max(1);
        let limit = usize_arg(args, "limit").unwrap_or(2000).max(1);
        let context = context.clone();

        match timeout(
            Duration::from_secs(FILE_IO_TIMEOUT_SECS),
            task::spawn_blocking(move || {
                let sections = paths
                    .iter()
                    .map(|path| {
                        (
                            format!("read_file path={path}"),
                            read_file_lines(path, offset, limit, &context),
                        )
                    })
                    .collect::<Vec<_>>();

                render_single_or_grouped_sections(sections)
            }),
        )
        .await
        {
            Ok(Ok(output)) => output,
            Ok(Err(error)) => format!("读取文件任务失败：{error}"),
            Err(_) => format!("读取文件超时（{FILE_IO_TIMEOUT_SECS} 秒）"),
        }
    }
}

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

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
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
                    return format!("创建父目录失败：{error}");
                }
            }
            match fs::write(&file_path, &content) {
                Ok(()) => format!(
                    "写入成功：{} 字符 -> {}",
                    content.len(),
                    file_path.display()
                ),
                Err(error) => format!("写入文件失败：{error}"),
            }
        })
        .await
        {
            Ok(output) => output,
            Err(error) => format!("写入文件任务失败：{error}"),
        }
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
                    "replace_all": { "type": "string", "description": "是否替换全部命中项，默认 false", "enum": ["true", "false"] }
                },
                "required": ["path", "old_text", "new_text"]
            }),
            false,
            "编辑工具通常只返回简短确认信息，默认关闭压缩。",
        )
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
        let Some(path) = string_arg(args, "path") else {
            return "错误：缺少必填参数 path".to_string();
        };
        let Some(old_text) = string_arg(args, "old_text") else {
            return "错误：缺少必填参数 old_text".to_string();
        };
        let Some(new_text) = string_arg(args, "new_text") else {
            return "错误：缺少必填参数 new_text".to_string();
        };
        let replace_all = boolish_arg(args, "replace_all").unwrap_or(false);

        let context = context.clone();
        match tokio::task::spawn_blocking(move || {
            let file_path = match resolve_path(&context, &path) {
                Ok(path) => path,
                Err(message) => return message,
            };
            let Ok(content) = fs::read_to_string(&file_path) else {
                return format!("错误：文件不存在或不可读：{path}");
            };
            if !content.contains(&old_text) {
                return format!("错误：在 {path} 中未找到 old_text");
            }
            if !replace_all && content.matches(&old_text).count() > 1 {
                return "错误：old_text 命中多处，请补充上下文或设置 replace_all=true".to_string();
            }

            let updated = if replace_all {
                content.replace(&old_text, &new_text)
            } else {
                content.replacen(&old_text, &new_text, 1)
            };

            match fs::write(&file_path, updated) {
                Ok(()) => format!("编辑成功：{}", file_path.display()),
                Err(error) => format!("编辑文件失败：{error}"),
            }
        })
        .await
        {
            Ok(output) => output,
            Err(error) => format!("编辑文件任务失败：{error}"),
        }
    }
}

#[async_trait]
impl AgentTool for ListDirTool {
    fn name(&self) -> &'static str {
        "list_dir"
    }

    fn description(&self) -> &'static str {
        "列出目录内容。需要继续深入结构时可设置 recursive=true。默认保留目录结构，若只需要概览可设置 compress=true 并写明 compress_intent。"
    }

    fn parameters(&self) -> Value {
        with_compression_parameters(
            json!({
                "type": "object",
                "properties": {
                    "paths": {
                        "type": "array",
                        "description": "要查看的目录路径列表。即使只查看一个目录，也必须传单元素数组。传入多个路径时，结果会按目录路径分段返回。",
                        "minItems": 1,
                        "items": { "type": "string" }
                    },
                    "recursive": { "type": "string", "description": "是否递归列出子目录内容，默认 false", "enum": ["true", "false"] },
                    "max_entries": { "type": "integer", "description": "最多返回多少条，默认 200", "minimum": 1 }
                },
                "required": ["paths"]
            }),
            false,
            "调查目录层级、文件名或结构差异时保持关闭保留完整结果；只看概览时可开启并写明 compress_intent。",
        )
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
        let Some(paths) = non_empty_string_array_arg(args, "paths") else {
            return "错误：缺少必填参数 paths，且 paths 必须是非空字符串数组".to_string();
        };
        let recursive = boolish_arg(args, "recursive").unwrap_or(false);
        let max_entries = usize_arg(args, "max_entries").unwrap_or(200).max(1);
        let context = context.clone();

        match task::spawn_blocking(move || {
            let sections = paths
                .iter()
                .map(|path| {
                    (
                        format!("list_dir path={path}"),
                        list_dir_entries(path, recursive, max_entries, &context),
                    )
                })
                .collect::<Vec<_>>();

            render_single_or_grouped_sections(sections)
        })
        .await
        {
            Ok(output) => output,
            Err(error) => format!("读取目录任务失败：{error}"),
        }
    }
}

fn render_single_or_grouped_sections(sections: Vec<(String, String)>) -> String {
    if sections.len() == 1 {
        sections
            .into_iter()
            .next()
            .map(|(_, content)| content)
            .unwrap_or_default()
    } else {
        render_labeled_sections(sections)
    }
}

fn read_file_lines(path: &str, offset: usize, limit: usize, context: &ToolContext) -> String {
    let file_path = match resolve_path(context, path) {
        Ok(path) => path,
        Err(message) => return message,
    };
    if !file_path.exists() {
        return format!("错误：文件不存在：{path}");
    }
    if file_path.is_dir() {
        return format!("错误：{path} 是目录，不是文件");
    }

    match fs::metadata(&file_path) {
        Ok(meta) if meta.len() > 2 * 1024 * 1024 => {
            return format!("错误：文件过大（{} bytes），超过 2MB 读取限制", meta.len());
        }
        _ => {}
    }
    match fs::read_to_string(&file_path) {
        Ok(content) => {
            let start = offset.saturating_sub(1);
            content
                .lines()
                .skip(start)
                .take(limit)
                .enumerate()
                .map(|(index, line)| format!("{}|{}", start + index + 1, line))
                .collect::<Vec<_>>()
                .join("\n")
        }
        Err(error) => format!("读取文件失败：{error}"),
    }
}

fn list_dir_entries(
    path: &str,
    recursive: bool,
    max_entries: usize,
    context: &ToolContext,
) -> String {
    let dir_path = match resolve_path(context, path) {
        Ok(path) => path,
        Err(message) => return message,
    };
    if !dir_path.exists() {
        return format!("错误：目录不存在：{path}");
    }
    if !dir_path.is_dir() {
        return format!("错误：{path} 不是目录");
    }

    let mut entries = Vec::new();
    collect_entries(&dir_path, &dir_path, recursive, max_entries, &mut entries);
    entries.join("\n")
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;

    use super::{ListDirTool, ReadFileTool};
    use crate::agent::tools::context::ToolContext;
    use crate::agent::tools::registry::AgentTool;

    fn create_workspace() -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "jkcodingagent-filesystem-test-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(root.join("src")).expect("create src");
        fs::create_dir_all(root.join("docs")).expect("create docs");
        fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("write main");
        fs::write(root.join("docs/README.md"), "# Docs\n").expect("write docs");
        root
    }

    fn tool_context(workspace: std::path::PathBuf) -> ToolContext {
        ToolContext {
            workspace_id: "test-workspace".to_string(),
            workspace,
            session_title: "test-session".to_string(),
            exec_timeout_secs: 30,
            restrict_to_workspace: true,
            extra_allowed_dirs: vec![],
            app_handle: None,
            llm_provider: None,
            vision_model: String::new(),
            image_model_url: String::new(),
            image_model_api_key: String::new(),
            image_model: String::new(),
            image_edit_model: String::new(),
            sub_agent_tool_registry: None,
        }
    }

    #[tokio::test]
    async fn list_dir_groups_multiple_paths() {
        let workspace = create_workspace();
        let context = tool_context(workspace.clone());

        let output = ListDirTool
            .execute(
                &json!({
                    "paths": ["src", "docs"],
                    "max_entries": 20
                }),
                &context,
            )
            .await;

        assert!(output.contains("## list_dir path=src"));
        assert!(output.contains("[file] main.rs"));
        assert!(output.contains("## list_dir path=docs"));
        assert!(output.contains("[file] README.md"));

        let _ = fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn read_file_groups_multiple_paths() {
        let workspace = create_workspace();
        let context = tool_context(workspace.clone());

        let output = ReadFileTool
            .execute(
                &json!({
                    "paths": ["src/main.rs", "docs/README.md"],
                    "limit": 5
                }),
                &context,
            )
            .await;

        assert!(output.contains("## read_file path=src/main.rs"));
        assert!(output.contains("1|fn main() {}"));
        assert!(output.contains("## read_file path=docs/README.md"));
        assert!(output.contains("1|# Docs"));

        let _ = fs::remove_dir_all(workspace);
    }

    #[test]
    fn read_only_file_tools_expose_paths_only() {
        for schema in [ReadFileTool.parameters(), ListDirTool.parameters()] {
            let properties = schema
                .get("properties")
                .and_then(serde_json::Value::as_object)
                .expect("schema properties");
            assert!(properties.contains_key("paths"));
            assert!(!properties.contains_key("path"));
            assert_eq!(
                schema.get("required"),
                Some(&json!(["paths"])),
                "paths should be the only required path field"
            );
        }
    }
}
