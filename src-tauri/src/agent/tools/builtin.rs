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
