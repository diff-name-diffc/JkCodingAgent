use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;

use async_trait::async_trait;
use glob::glob;
use serde_json::{json, Value};
use tokio::process::Command;
use tokio::time::{timeout, Duration};

use super::context::ToolContext;
use super::registry::AgentTool;

const NOISE: &[&str] = &[
    ".git",
    ".svn",
    ".hg",
    ".idea",
    ".vscode",
    ".vs",
    "node_modules",
    "__pycache__",
    ".venv",
    "venv",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    "dist",
    "build",
    ".next",
    ".output",
    "target",
];

const DANGEROUS_PATTERNS: &[&str] = &[
    "rm -rf /",
    "rm -rf /*",
    "mkfs",
    "dd if=",
    "shutdown",
    "reboot",
    "halt",
    "poweroff",
    "format",
    ":(){:|:&};:",
];

pub(super) fn builtin_tools() -> Vec<Box<dyn AgentTool>> {
    vec![
        Box::new(ReadFileTool),
        Box::new(WriteFileTool),
        Box::new(EditFileTool),
        Box::new(ListDirTool),
        Box::new(GlobTool),
        Box::new(ExecTool),
        Box::new(MessageTool),
    ]
}

struct ReadFileTool;
struct WriteFileTool;
struct EditFileTool;
struct ListDirTool;
struct GlobTool;
struct ExecTool;
struct MessageTool;

#[async_trait]
impl AgentTool for ReadFileTool {
    fn name(&self) -> &'static str {
        "read_file"
    }

    fn description(&self) -> &'static str {
        "读取文本文件，输出格式为 行号|内容。分析代码时优先使用；大文件请配合 offset 和 limit 分段读取。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "要读取的文件路径" },
                "offset": { "type": "integer", "description": "起始行号，从 1 开始，默认 1" , "minimum": 1 },
                "limit": { "type": "integer", "description": "最多读取多少行，默认 2000", "minimum": 1 }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
        let Some(path) = string_arg(args, "path") else {
            return "错误：缺少必填参数 path".to_string();
        };
        let offset = usize_arg(args, "offset").unwrap_or(1).max(1);
        let limit = usize_arg(args, "limit").unwrap_or(2000).max(1);

        let file_path = match resolve_path(context, &path) {
            Ok(path) => path,
            Err(message) => return message,
        };
        if !file_path.exists() {
            return format!("错误：文件不存在：{path}");
        }
        if file_path.is_dir() {
            return format!("错误：{path} 是目录，不是文件");
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
}

#[async_trait]
impl AgentTool for WriteFileTool {
    fn name(&self) -> &'static str {
        "write_file"
    }

    fn description(&self) -> &'static str {
        "将内容写入文件。若文件已存在则覆盖；必要时自动创建父目录。仅适合小范围修改或生成文件。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "要写入的文件路径" },
                "content": { "type": "string", "description": "要写入的内容" }
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
        let Some(path) = string_arg(args, "path") else {
            return "错误：缺少必填参数 path".to_string();
        };
        let Some(content) = string_arg(args, "content") else {
            return "错误：缺少必填参数 content".to_string();
        };

        let file_path = match resolve_path(context, &path) {
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
    }
}

#[async_trait]
impl AgentTool for EditFileTool {
    fn name(&self) -> &'static str {
        "edit_file"
    }

    fn description(&self) -> &'static str {
        "通过将 old_text 替换为 new_text 来编辑文件。如果 old_text 命中多处，请补充上下文或设置 replace_all=true。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "要编辑的文件路径" },
                "old_text": { "type": "string", "description": "要查找并替换的原文本" },
                "new_text": { "type": "string", "description": "替换后的新文本" },
                "replace_all": { "type": "string", "description": "是否替换全部命中项，默认 false", "enum": ["true", "false"] }
            },
            "required": ["path", "old_text", "new_text"]
        })
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

        let file_path = match resolve_path(context, &path) {
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
    }
}

#[async_trait]
impl AgentTool for ListDirTool {
    fn name(&self) -> &'static str {
        "list_dir"
    }

    fn description(&self) -> &'static str {
        "列出目录内容。需要继续深入结构时可设置 recursive=true。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "要查看的目录路径" },
                "recursive": { "type": "string", "description": "是否递归列出子目录内容，默认 false", "enum": ["true", "false"] },
                "max_entries": { "type": "integer", "description": "最多返回多少条，默认 200", "minimum": 1 }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
        let Some(path) = string_arg(args, "path") else {
            return "错误：缺少必填参数 path".to_string();
        };
        let recursive = boolish_arg(args, "recursive").unwrap_or(false);
        let max_entries = usize_arg(args, "max_entries").unwrap_or(200).max(1);

        let dir_path = match resolve_path(context, &path) {
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
}

#[async_trait]
impl AgentTool for GlobTool {
    fn name(&self) -> &'static str {
        "glob"
    }

    fn description(&self) -> &'static str {
        "按 glob 模式查找文件，结果按修改时间倒序排列。适合快速缩小文件范围。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "匹配模式，例如 '*.rs' 或 'src/**/*.ts'" },
                "path": { "type": "string", "description": "从哪个目录开始搜索，默认 '.'" },
                "max_results": { "type": "integer", "description": "最多返回多少个结果，默认 250", "minimum": 1 }
            },
            "required": ["pattern"]
        })
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
        let Some(pattern) = string_arg(args, "pattern") else {
            return "错误：缺少必填参数 pattern".to_string();
        };
        let path = string_arg(args, "path").unwrap_or_else(|| ".".to_string());
        let max_results = usize_arg(args, "max_results").unwrap_or(250).max(1);
        let dir_path = match resolve_path(context, &path) {
            Ok(path) => path,
            Err(message) => return message,
        };
        let search_pattern = dir_path.join(pattern);
        let Some(search_pattern) = search_pattern.to_str() else {
            return "错误：glob 模式不是有效的 UTF-8".to_string();
        };

        let mut matches = Vec::new();
        for entry in match glob(search_pattern) {
            Ok(entries) => entries,
            Err(error) => return format!("glob 模式无效：{error}"),
        } {
            match entry {
                Ok(path) if !path.file_name().is_some_and(is_noise) => matches.push(path),
                Ok(_) => {}
                Err(error) => return format!("glob 搜索失败：{error}"),
            }
        }
        matches.sort_by_key(|path| {
            fs::metadata(path)
                .and_then(|metadata| metadata.modified())
                .ok()
        });
        matches.reverse();

        if matches.is_empty() {
            return format!("未找到匹配文件：{}", dir_path.display());
        }

        let mut lines = matches
            .iter()
            .take(max_results)
            .map(|path| rel(path, &dir_path))
            .collect::<Vec<_>>();
        if matches.len() > max_results {
            lines.push(format!("...（已显示 {} / {}）", max_results, matches.len()));
        }
        lines.join("\n")
    }
}

#[async_trait]
impl AgentTool for ExecTool {
    fn name(&self) -> &'static str {
        "exec"
    }

    fn description(&self) -> &'static str {
        "在当前工作区执行 shell 命令并返回输出。适合搜索、构建、测试、查看 git 信息；优先使用只读命令。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "要执行的 shell 命令" },
                "timeout": { "type": "integer", "description": "超时时间，单位秒，默认 60", "minimum": 1 }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
        let Some(command) = string_arg(args, "command") else {
            return "错误：缺少必填参数 command".to_string();
        };
        if is_dangerous(&command) {
            return format!("错误：基于安全策略已拦截命令：{command}");
        }
        let timeout_secs = u64_arg(args, "timeout")
            .unwrap_or(context.exec_timeout_secs)
            .max(1);

        let child = match Command::new("sh")
            .arg("-lc")
            .arg(&command)
            .current_dir(&context.workspace)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
        {
            Ok(child) => child,
            Err(error) => return format!("执行命令失败：{error}"),
        };

        let output =
            match timeout(Duration::from_secs(timeout_secs), child.wait_with_output()).await {
                Ok(Ok(output)) => output,
                Ok(Err(error)) => return format!("执行命令失败：{error}"),
                Err(_) => return format!("错误：命令执行超时（{timeout_secs} 秒）"),
            };

        let stdout = String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_string();
        let stderr = String::from_utf8_lossy(&output.stderr)
            .trim_end()
            .to_string();
        let mut result = String::new();
        if !stdout.is_empty() {
            result.push_str(&stdout);
        }
        if !stderr.is_empty() {
            if !result.is_empty() {
                result.push_str("\n【标准错误】\n");
            }
            result.push_str(&stderr);
        }
        if !output.status.success() {
            result.push_str(&format!("\n\n[退出状态：{}]", output.status));
        }
        if result.is_empty() {
            "[命令已完成，无输出]".to_string()
        } else {
            result
        }
    }
}

#[async_trait]
impl AgentTool for MessageTool {
    fn name(&self) -> &'static str {
        "message"
    }

    fn description(&self) -> &'static str {
        "向用户发送最终回复。通常在调查完成、结果整理完成或协调结束后使用。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "content": { "type": "string", "description": "要发送给用户的内容" }
            },
            "required": ["content"]
        })
    }

    async fn execute(&self, args: &Value, _context: &ToolContext) -> String {
        match string_arg(args, "content") {
            Some(content) => format!("消息已发送（{} 字符）", content.len()),
            None => "错误：缺少必填参数 content".to_string(),
        }
    }
}

fn string_arg(args: &Value, key: &str) -> Option<String> {
    args.get(key)?.as_str().map(str::to_string)
}

fn usize_arg(args: &Value, key: &str) -> Option<usize> {
    args.get(key)?.as_u64().map(|value| value as usize)
}

fn u64_arg(args: &Value, key: &str) -> Option<u64> {
    args.get(key)?.as_u64()
}

fn boolish_arg(args: &Value, key: &str) -> Option<bool> {
    let value = args.get(key)?;
    if let Some(flag) = value.as_bool() {
        return Some(flag);
    }
    value.as_str().map(|flag| flag.eq_ignore_ascii_case("true"))
}

fn resolve_path(context: &ToolContext, raw_path: &str) -> Result<PathBuf, String> {
    let raw = PathBuf::from(raw_path);
    let joined = if raw.is_absolute() {
        raw
    } else {
        context.workspace.join(raw)
    };
    let normalized = lexical_normalize(&joined);

    if context.restrict_to_workspace {
        let workspace = context
            .workspace
            .canonicalize()
            .map_err(|error| format!("解析工作区路径失败：{error}"))?;
        let candidate = if normalized.exists() {
            normalized
                .canonicalize()
                .map_err(|error| format!("解析路径失败：{error}"))?
        } else {
            normalized
        };
        if !candidate.starts_with(&workspace) {
            return Err(format!("错误：禁止访问工作区之外的路径：{raw_path}"));
        }
        return Ok(candidate);
    }

    Ok(normalized)
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn collect_entries(
    root: &Path,
    current: &Path,
    recursive: bool,
    max_entries: usize,
    entries: &mut Vec<String>,
) {
    if entries.len() >= max_entries {
        return;
    }
    let Ok(read_dir) = fs::read_dir(current) else {
        return;
    };
    let mut items = read_dir.filter_map(Result::ok).collect::<Vec<_>>();
    items.sort_by_key(|entry| entry.file_name());

    for item in items {
        if entries.len() >= max_entries {
            entries.push(format!("... ({max_entries} entries shown)"));
            return;
        }
        if is_noise(&item.file_name()) {
            continue;
        }
        let path = item.path();
        if path.is_dir() {
            entries.push(format!("[dir] {}/", rel(&path, root)));
            if recursive {
                collect_entries(root, &path, recursive, max_entries, entries);
            }
        } else {
            entries.push(format!("[file] {}", rel(&path, root)));
        }
    }
}

fn is_noise(name: &std::ffi::OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return true;
    };
    NOISE.contains(&name)
        || (name.starts_with('.') && !matches!(name, ".env" | ".gitignore" | ".dockerignore"))
}

fn rel(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
}

fn is_dangerous(command: &str) -> bool {
    let lower = command.to_lowercase();
    DANGEROUS_PATTERNS
        .iter()
        .any(|pattern| lower.contains(pattern))
}
