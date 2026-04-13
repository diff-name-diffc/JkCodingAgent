use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;

use async_trait::async_trait;
use glob::glob;
use serde_json::{json, Value};
use tokio::process::Command;
use tokio::time::{timeout, Duration};

use crate::dispatcher_llm::{ToolDefinition, ToolFunctionDefinition};

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

#[derive(Debug, Clone)]
pub struct ToolContext {
    pub workspace: PathBuf,
    pub max_result_chars: usize,
    pub exec_timeout_secs: u64,
    pub restrict_to_workspace: bool,
}

#[async_trait]
trait AgentTool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn parameters(&self) -> Value;
    async fn execute(&self, args: &Value, context: &ToolContext) -> String;
}

pub struct ToolRegistry {
    tools: Vec<Box<dyn AgentTool>>,
}

impl ToolRegistry {
    pub fn default_tools() -> Self {
        Self {
            tools: vec![
                Box::new(ReadFileTool),
                Box::new(WriteFileTool),
                Box::new(EditFileTool),
                Box::new(ListDirTool),
                Box::new(GlobTool),
                Box::new(ExecTool),
                Box::new(DispatchClaudeTool),
                Box::new(ContinueClaudeSessionTool),
                Box::new(ExitClaudeSessionTool),
                Box::new(MessageTool),
            ],
        }
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .iter()
            .map(|tool| ToolDefinition {
                kind: "function".to_string(),
                function: ToolFunctionDefinition {
                    name: tool.name().to_string(),
                    description: tool.description().to_string(),
                    parameters: tool.parameters(),
                },
            })
            .collect()
    }

    pub async fn execute(&self, name: &str, args: &Value, context: &ToolContext) -> String {
        match self.tools.iter().find(|tool| tool.name() == name) {
            Some(tool) => truncate(tool.execute(args, context).await, context.max_result_chars),
            None => format!("Error: tool '{name}' not found"),
        }
    }
}

struct ReadFileTool;
struct WriteFileTool;
struct EditFileTool;
struct ListDirTool;
struct GlobTool;
struct ExecTool;
struct DispatchClaudeTool;
struct ContinueClaudeSessionTool;
struct ExitClaudeSessionTool;
struct MessageTool;

// ── dispatch_claude tool ─────────────────────────────────────────────────────

/// `dispatch_claude` 工具的执行结果类型标记。
/// 真正的 Claude 进程拉起由 agent.rs 通过事件 + Tauri 命令配合完成，
/// 此工具仅返回一个带有特殊前缀的指令字符串，供 agent.rs 拦截并进入调度流程。
const DISPATCH_PREFIX: &str = "__DISPATCH_CLAUDE__:";

#[async_trait]
impl AgentTool for DispatchClaudeTool {
    fn name(&self) -> &'static str {
        "dispatch_claude"
    }

    fn description(&self) -> &'static str {
        "Delegate a coding task to a Claude Code specialist agent running in a real terminal. Use this for complex multi-file edits, refactoring, feature implementation, and other substantial coding work. Claude has full access to the workspace. The task description should be detailed and self-contained."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task_description": {
                    "type": "string",
                    "description": "A detailed, self-contained description of the coding task for Claude to execute"
                },
                "permission_mode": {
                    "type": "string",
                    "description": "Permission mode for Claude: ask (default permissions), auto_edit (auto-accept edits), full_access (skip all permissions)",
                    "enum": ["ask", "auto_edit", "full_access"],
                    "default": "full_access"
                }
            },
            "required": ["task_description"]
        })
    }

    async fn execute(&self, args: &Value, _context: &ToolContext) -> String {
        let task_description = match string_arg(args, "task_description") {
            Some(desc) if !desc.trim().is_empty() => desc,
            _ => return "Error: task_description is required and must not be empty".to_string(),
        };
        let permission_mode =
            string_arg(args, "permission_mode").unwrap_or_else(|| "full_access".to_string());

        // Return dispatch instruction for agent.rs to intercept
        format!(
            "{}{}|||{}",
            DISPATCH_PREFIX, task_description, permission_mode
        )
    }
}

/// Check if a tool result is a dispatch_claude instruction.
pub fn is_dispatch_instruction(result: &str) -> bool {
    result.starts_with(DISPATCH_PREFIX)
}

/// Parse a dispatch_claude instruction into (task_description, permission_mode).
pub fn parse_dispatch_instruction(result: &str) -> Option<(String, String)> {
    let after = result.strip_prefix(DISPATCH_PREFIX)?;
    let parts: Vec<&str> = after.splitn(2, "|||").collect();
    if parts.len() == 2 {
        Some((parts[0].to_string(), parts[1].to_string()))
    } else {
        Some((after.to_string(), "full_access".to_string()))
    }
}

// ── continue_claude_session tool ─────────────────────────────────────────────

const CONTINUE_PREFIX: &str = "__CONTINUE_CLAUDE__:";

#[async_trait]
impl AgentTool for ContinueClaudeSessionTool {
    fn name(&self) -> &'static str {
        "continue_claude_session"
    }

    fn description(&self) -> &'static str {
        "Continue an active Claude Code session by sending additional instructions to the running terminal. Use this when you need Claude to perform follow-up tasks or corrections in the same session. The Claude process must still be running."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task_description": {
                    "type": "string",
                    "description": "The follow-up instruction to send to the active Claude session"
                }
            },
            "required": ["task_description"]
        })
    }

    async fn execute(&self, args: &Value, _context: &ToolContext) -> String {
        let task_description = match string_arg(args, "task_description") {
            Some(desc) if !desc.trim().is_empty() => desc,
            _ => return "Error: task_description is required and must not be empty".to_string(),
        };
        format!("{}{}", CONTINUE_PREFIX, task_description)
    }
}

pub fn is_continue_instruction(result: &str) -> bool {
    result.starts_with(CONTINUE_PREFIX)
}

pub fn parse_continue_instruction(result: &str) -> Option<String> {
    result.strip_prefix(CONTINUE_PREFIX).map(|s| s.to_string())
}

// ── exit_claude_session tool ─────────────────────────────────────────────────

const EXIT_PREFIX: &str = "__EXIT_CLAUDE__:";

#[async_trait]
impl AgentTool for ExitClaudeSessionTool {
    fn name(&self) -> &'static str {
        "exit_claude_session"
    }

    fn description(&self) -> &'static str {
        "Exit the active Claude Code session by sending /exit to the terminal. Use this when the Claude task is complete and no further interaction is needed. This will terminate the Claude process."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "reason": {
                    "type": "string",
                    "description": "Optional reason for exiting the session"
                }
            }
        })
    }

    async fn execute(&self, args: &Value, _context: &ToolContext) -> String {
        let reason = string_arg(args, "reason").unwrap_or_else(|| "Task completed".to_string());
        format!("{}{}", EXIT_PREFIX, reason)
    }
}

pub fn is_exit_instruction(result: &str) -> bool {
    result.starts_with(EXIT_PREFIX)
}

pub fn parse_exit_instruction(result: &str) -> Option<String> {
    result.strip_prefix(EXIT_PREFIX).map(|s| s.to_string())
}

// ── Standard tools (ported from mini_code_bot) ──────────────────────────────

#[async_trait]
impl AgentTool for ReadFileTool {
    fn name(&self) -> &'static str {
        "read_file"
    }

    fn description(&self) -> &'static str {
        "Read a text file. Output format: LINE_NUM|CONTENT. Use offset and limit for large files."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "The file path to read" },
                "offset": { "type": "integer", "description": "Line number to start reading from (1-indexed, default 1)", "minimum": 1 },
                "limit": { "type": "integer", "description": "Maximum number of lines to read (default 2000)", "minimum": 1 }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
        let Some(path) = string_arg(args, "path") else {
            return "Error: path is required".to_string();
        };
        let offset = usize_arg(args, "offset").unwrap_or(1).max(1);
        let limit = usize_arg(args, "limit").unwrap_or(2000).max(1);

        let file_path = match resolve_path(context, &path) {
            Ok(path) => path,
            Err(message) => return message,
        };
        if !file_path.exists() {
            return format!("Error: file not found: {path}");
        }
        if file_path.is_dir() {
            return format!("Error: {path} is a directory, not a file");
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
            Err(error) => format!("Error reading file: {error}"),
        }
    }
}

#[async_trait]
impl AgentTool for WriteFileTool {
    fn name(&self) -> &'static str {
        "write_file"
    }

    fn description(&self) -> &'static str {
        "Write content to a file. Overwrites if the file already exists; creates parent directories as needed."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "The file path to write to" },
                "content": { "type": "string", "description": "The content to write" }
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
        let Some(path) = string_arg(args, "path") else {
            return "Error: path is required".to_string();
        };
        let Some(content) = string_arg(args, "content") else {
            return "Error: content is required".to_string();
        };

        let file_path = match resolve_path(context, &path) {
            Ok(path) => path,
            Err(message) => return message,
        };
        if let Some(parent) = file_path.parent() {
            if let Err(error) = fs::create_dir_all(parent) {
                return format!("Error creating parent directory: {error}");
            }
        }
        match fs::write(&file_path, &content) {
            Ok(()) => format!(
                "Successfully wrote {} chars to {}",
                content.len(),
                file_path.display()
            ),
            Err(error) => format!("Error writing file: {error}"),
        }
    }
}

#[async_trait]
impl AgentTool for EditFileTool {
    fn name(&self) -> &'static str {
        "edit_file"
    }

    fn description(&self) -> &'static str {
        "Edit a file by replacing old_text with new_text. If old_text matches multiple times, provide more context or set replace_all=true."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "The file path to edit" },
                "old_text": { "type": "string", "description": "The text to find and replace" },
                "new_text": { "type": "string", "description": "The text to replace with" },
                "replace_all": { "type": "string", "description": "Replace all occurrences (default false)", "enum": ["true", "false"] }
            },
            "required": ["path", "old_text", "new_text"]
        })
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
        let Some(path) = string_arg(args, "path") else {
            return "Error: path is required".to_string();
        };
        let Some(old_text) = string_arg(args, "old_text") else {
            return "Error: old_text is required".to_string();
        };
        let Some(new_text) = string_arg(args, "new_text") else {
            return "Error: new_text is required".to_string();
        };
        let replace_all = boolish_arg(args, "replace_all").unwrap_or(false);

        let file_path = match resolve_path(context, &path) {
            Ok(path) => path,
            Err(message) => return message,
        };
        let Ok(content) = fs::read_to_string(&file_path) else {
            return format!("Error: file not found or not readable: {path}");
        };
        if !content.contains(&old_text) {
            return format!("Error: old_text not found in {path}");
        }
        if !replace_all && content.matches(&old_text).count() > 1 {
            return "Error: old_text matched multiple times; provide more context or set replace_all=true".to_string();
        }

        let updated = if replace_all {
            content.replace(&old_text, &new_text)
        } else {
            content.replacen(&old_text, &new_text, 1)
        };

        match fs::write(&file_path, updated) {
            Ok(()) => format!("Successfully edited {}", file_path.display()),
            Err(error) => format!("Error editing file: {error}"),
        }
    }
}

#[async_trait]
impl AgentTool for ListDirTool {
    fn name(&self) -> &'static str {
        "list_dir"
    }

    fn description(&self) -> &'static str {
        "List the contents of a directory. Set recursive=true to explore nested structure."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "The directory path to list" },
                "recursive": { "type": "string", "description": "Recursively list all files (default false)", "enum": ["true", "false"] },
                "max_entries": { "type": "integer", "description": "Maximum entries to return (default 200)", "minimum": 1 }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
        let Some(path) = string_arg(args, "path") else {
            return "Error: path is required".to_string();
        };
        let recursive = boolish_arg(args, "recursive").unwrap_or(false);
        let max_entries = usize_arg(args, "max_entries").unwrap_or(200).max(1);

        let dir_path = match resolve_path(context, &path) {
            Ok(path) => path,
            Err(message) => return message,
        };
        if !dir_path.exists() {
            return format!("Error: directory not found: {path}");
        }
        if !dir_path.is_dir() {
            return format!("Error: {path} is not a directory");
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
        "Find files matching a glob pattern. Results are sorted by modification time (newest first)."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "Glob pattern to match, e.g. '*.rs' or 'src/**/*.ts'" },
                "path": { "type": "string", "description": "Directory to search from (default '.')" },
                "max_results": { "type": "integer", "description": "Maximum number of matches to return (default 250)", "minimum": 1 }
            },
            "required": ["pattern"]
        })
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
        let Some(pattern) = string_arg(args, "pattern") else {
            return "Error: pattern is required".to_string();
        };
        let path = string_arg(args, "path").unwrap_or_else(|| ".".to_string());
        let max_results = usize_arg(args, "max_results").unwrap_or(250).max(1);
        let dir_path = match resolve_path(context, &path) {
            Ok(path) => path,
            Err(message) => return message,
        };
        let search_pattern = dir_path.join(pattern);
        let Some(search_pattern) = search_pattern.to_str() else {
            return "Error: glob pattern is not valid UTF-8".to_string();
        };

        let mut matches = Vec::new();
        for entry in match glob(search_pattern) {
            Ok(entries) => entries,
            Err(error) => return format!("Error in glob pattern: {error}"),
        } {
            match entry {
                Ok(path) if !path.file_name().is_some_and(is_noise) => matches.push(path),
                Ok(_) => {}
                Err(error) => return format!("Error in glob search: {error}"),
            }
        }
        matches.sort_by_key(|path| {
            fs::metadata(path)
                .and_then(|metadata| metadata.modified())
                .ok()
        });
        matches.reverse();

        if matches.is_empty() {
            return format!("No files matching pattern in {}", dir_path.display());
        }

        let mut lines = matches
            .iter()
            .take(max_results)
            .map(|path| rel(path, &dir_path))
            .collect::<Vec<_>>();
        if matches.len() > max_results {
            lines.push(format!("... ({} of {} shown)", max_results, matches.len()));
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
        "Execute a shell command in the active workspace and return stdout/stderr."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "The shell command to execute" },
                "timeout": { "type": "integer", "description": "Timeout in seconds (default 60)", "minimum": 1 }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
        let Some(command) = string_arg(args, "command") else {
            return "Error: command is required".to_string();
        };
        if is_dangerous(&command) {
            return format!("Error: command blocked for safety: {command}");
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
            Err(error) => return format!("Error executing command: {error}"),
        };

        let output =
            match timeout(Duration::from_secs(timeout_secs), child.wait_with_output()).await {
                Ok(Ok(output)) => output,
                Ok(Err(error)) => return format!("Error executing command: {error}"),
                Err(_) => return format!("Error: command timed out after {timeout_secs}s"),
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
                result.push_str("\n--- stderr ---\n");
            }
            result.push_str(&stderr);
        }
        if !output.status.success() {
            result.push_str(&format!("\n\n[Exit code: {}]", output.status));
        }
        if result.is_empty() {
            "[Command completed with no output]".to_string()
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
        "Send a final message to the user."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "content": { "type": "string", "description": "The message content to send" }
            },
            "required": ["content"]
        })
    }

    async fn execute(&self, args: &Value, _context: &ToolContext) -> String {
        match string_arg(args, "content") {
            Some(content) => format!("Message sent ({} chars)", content.len()),
            None => "Error: content is required".to_string(),
        }
    }
}

// ── Utility functions ────────────────────────────────────────────────────────

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
            .map_err(|error| format!("Error resolving workspace: {error}"))?;
        let candidate = if normalized.exists() {
            normalized
                .canonicalize()
                .map_err(|error| format!("Error resolving path: {error}"))?
        } else {
            normalized
        };
        if !candidate.starts_with(&workspace) {
            return Err(format!(
                "Error: access denied outside workspace: {raw_path}"
            ));
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

fn truncate(mut text: String, max_chars: usize) -> String {
    if text.len() <= max_chars {
        return text;
    }
    text.truncate(max_chars);
    text.push_str("\n\n[Output truncated]");
    text
}
