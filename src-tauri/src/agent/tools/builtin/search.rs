use std::collections::HashMap;
use std::fs;

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use glob::glob;
use serde_json::{json, Value};
use tokio::process::Command;

use super::common::{
    boolish_arg, is_noise, rel, resolve_path, string_arg, string_array_arg, usize_arg,
    with_result_mode_parameter,
};
use crate::agent::tools::context::ToolContext;
use crate::agent::tools::registry::AgentTool;

pub(super) fn glob_tool() -> Box<dyn AgentTool> {
    Box::new(GlobTool)
}

pub(super) fn grep_tool() -> Box<dyn AgentTool> {
    Box::new(GrepTool)
}

struct GlobTool;
struct GrepTool;

#[derive(Debug, Default)]
struct GrepFileResult {
    path: String,
    lines: Vec<GrepLine>,
    match_count: usize,
}

#[derive(Debug)]
struct GrepLine {
    line_number: u64,
    text: String,
    is_match: bool,
}

#[async_trait]
impl AgentTool for GlobTool {
    fn name(&self) -> &'static str {
        "glob"
    }

    fn description(&self) -> &'static str {
        "按 glob 模式查找文件，结果按修改时间倒序排列。适合快速缩小文件范围。默认保留匹配文件列表，若只需要概览可传 result_mode=summary。"
    }

    fn parameters(&self) -> Value {
        with_result_mode_parameter(
            json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "匹配模式，例如 '*.rs' 或 'src/**/*.ts'" },
                    "path": { "type": "string", "description": "从哪个目录开始搜索，默认 '.'" },
                    "max_results": { "type": "integer", "description": "最多返回多少个结果，默认 250", "minimum": 1 }
                },
                "required": ["pattern"]
            }),
            "full",
            "当后续需要精确文件列表时保留完整结果；只看分布或概况时改用 summary。",
        )
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
impl AgentTool for GrepTool {
    fn name(&self) -> &'static str {
        "grep"
    }

    fn description(&self) -> &'static str {
        "使用 ripgrep 在工作区内搜索文本。推荐先用 glob 缩小文件范围，再用 grep 精确定位符号、配置键或错误文本，最后再 read_file 读取确认。默认保留精确结果。"
    }

    fn parameters(&self) -> Value {
        with_result_mode_parameter(
            json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "要搜索的模式。默认按正则处理。" },
                    "path": { "type": "string", "description": "搜索起点，默认 '.'；可传目录或单个文件。" },
                    "include": {
                        "type": "array",
                        "description": "可选的 glob 过滤列表，例如 ['src/**/*.rs', 'src/**/*.ts']",
                        "items": { "type": "string" }
                    },
                    "exclude": {
                        "type": "array",
                        "description": "要排除的 glob 列表，例如 ['target/**', 'dist/**']",
                        "items": { "type": "string" }
                    },
                    "match_mode": {
                        "type": "string",
                        "description": "匹配模式：regex 使用正则；fixed 按字面量精确匹配。",
                        "enum": ["regex", "fixed"],
                        "default": "regex"
                    },
                    "case_sensitive": {
                        "type": "boolean",
                        "description": "是否大小写敏感。未提供时使用 smart-case。"
                    },
                    "word": {
                        "type": "boolean",
                        "description": "是否按完整单词匹配。"
                    },
                    "context_before": { "type": "integer", "description": "前置上下文行数，默认 0", "minimum": 0 },
                    "context_after": { "type": "integer", "description": "后置上下文行数，默认 0", "minimum": 0 },
                    "max_matches_per_file": {
                        "type": "integer",
                        "description": "每个文件最多返回多少处匹配，默认 20",
                        "minimum": 1
                    },
                    "max_files": {
                        "type": "integer",
                        "description": "最多展示多少个命中文件，默认 25",
                        "minimum": 1
                    },
                    "files_with_matches": {
                        "type": "boolean",
                        "description": "只返回命中的文件路径，不返回具体匹配行。"
                    },
                    "include_hidden": {
                        "type": "boolean",
                        "description": "是否包含隐藏文件。"
                    },
                    "no_ignore": {
                        "type": "boolean",
                        "description": "是否忽略 .gitignore 等规则。"
                    }
                },
                "required": ["pattern"]
            }),
            "full",
            "grep 是精确检索工具，通常应保留原始匹配行与行号；只在非常长且用户只要概览时再改用 summary。",
        )
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
        let Some(pattern) = string_arg(args, "pattern") else {
            return "错误：缺少必填参数 pattern".to_string();
        };
        let path = string_arg(args, "path").unwrap_or_else(|| ".".to_string());
        let include = string_array_arg(args, "include").unwrap_or_default();
        let exclude = string_array_arg(args, "exclude").unwrap_or_default();
        let match_mode = string_arg(args, "match_mode").unwrap_or_else(|| "regex".to_string());
        let case_sensitive = args.get("case_sensitive").and_then(Value::as_bool);
        let word = boolish_arg(args, "word").unwrap_or(false);
        let context_before = usize_arg(args, "context_before").unwrap_or(0);
        let context_after = usize_arg(args, "context_after").unwrap_or(0);
        let max_matches_per_file = usize_arg(args, "max_matches_per_file").unwrap_or(20).max(1);
        let max_files = usize_arg(args, "max_files").unwrap_or(25).max(1);
        let files_with_matches = boolish_arg(args, "files_with_matches").unwrap_or(false);
        let include_hidden = boolish_arg(args, "include_hidden").unwrap_or(false);
        let no_ignore = boolish_arg(args, "no_ignore").unwrap_or(false);

        let search_path = match resolve_path(context, &path) {
            Ok(path) => path,
            Err(message) => return message,
        };
        if !search_path.exists() {
            return format!("错误：搜索路径不存在：{path}");
        }

        let workspace = match context.workspace.canonicalize() {
            Ok(path) => path,
            Err(error) => return format!("解析工作区路径失败：{error}"),
        };
        let target = workspace_relative_target(&workspace, &search_path);

        let mut command = Command::new("rg");
        command
            .arg("--json")
            .arg("--line-number")
            .arg("--color")
            .arg("never")
            .arg("--max-count")
            .arg(max_matches_per_file.to_string())
            .current_dir(&workspace)
            .kill_on_drop(true);

        match case_sensitive {
            Some(true) => {
                command.arg("--case-sensitive");
            }
            Some(false) => {
                command.arg("--ignore-case");
            }
            None => {
                command.arg("--smart-case");
            }
        }

        if match_mode == "fixed" {
            command.arg("--fixed-strings");
        }
        if word {
            command.arg("--word-regexp");
        }
        if include_hidden {
            command.arg("--hidden");
        }
        if no_ignore {
            command.arg("--no-ignore");
        }
        if context_before == context_after && context_before > 0 {
            command.arg("--context").arg(context_before.to_string());
        } else {
            if context_before > 0 {
                command
                    .arg("--before-context")
                    .arg(context_before.to_string());
            }
            if context_after > 0 {
                command
                    .arg("--after-context")
                    .arg(context_after.to_string());
            }
        }

        for glob in include {
            command.arg("--glob").arg(glob);
        }
        for glob in exclude {
            command.arg("--glob").arg(format!("!{glob}"));
        }

        command.arg(&pattern).arg(&target);

        let output = match command.output().await {
            Ok(output) => output,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return "错误：未找到 ripgrep (`rg`) 可执行文件，无法执行 grep 搜索".to_string();
            }
            Err(error) => return format!("执行 grep 搜索失败：{error}"),
        };

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let status = output.status.code().unwrap_or_default();
        if status != 0 && status != 1 {
            if stderr.is_empty() {
                return format!("grep 搜索失败，退出状态：{}", output.status);
            }
            return format!("grep 搜索失败：{stderr}\n\n[退出状态：{}]", output.status);
        }

        let rendered = render_grep_stdout(&stdout, max_files, files_with_matches);
        if rendered.is_empty() {
            return format!("未找到匹配内容：{pattern}");
        }
        rendered
    }
}

fn workspace_relative_target(workspace: &std::path::Path, target: &std::path::Path) -> String {
    if target == workspace {
        ".".to_string()
    } else {
        target
            .strip_prefix(workspace)
            .unwrap_or(target)
            .to_string_lossy()
            .to_string()
    }
}

fn render_grep_stdout(stdout: &str, max_files: usize, files_with_matches: bool) -> String {
    let mut file_results = Vec::<GrepFileResult>::new();
    let mut file_indexes = HashMap::<String, usize>::new();
    let mut total_matches = 0usize;
    let mut truncated_by_file_limit = false;

    for raw_line in stdout.lines() {
        let Ok(value) = serde_json::from_str::<Value>(raw_line) else {
            continue;
        };
        let Some(kind) = value.get("type").and_then(Value::as_str) else {
            continue;
        };
        if !matches!(kind, "match" | "context") {
            continue;
        }

        let Some(path) = grep_event_path(&value) else {
            continue;
        };
        let index = if let Some(index) = file_indexes.get(&path).copied() {
            index
        } else if file_results.len() < max_files {
            let index = file_results.len();
            file_results.push(GrepFileResult {
                path: path.clone(),
                ..Default::default()
            });
            file_indexes.insert(path.clone(), index);
            index
        } else {
            truncated_by_file_limit = true;
            continue;
        };

        if kind == "match" {
            total_matches += 1;
            file_results[index].match_count += 1;
            if files_with_matches {
                continue;
            }
        } else if files_with_matches {
            continue;
        }

        let Some(line_number) = value
            .get("data")
            .and_then(|data| data.get("line_number"))
            .and_then(Value::as_u64)
        else {
            continue;
        };
        let Some(text) = grep_event_text(&value) else {
            continue;
        };
        file_results[index].lines.push(GrepLine {
            line_number,
            text,
            is_match: kind == "match",
        });
    }

    let matched_files = file_results
        .iter()
        .filter(|file| file.match_count > 0)
        .collect::<Vec<_>>();
    if matched_files.is_empty() {
        return String::new();
    }

    let mut lines = vec![format!(
        "共 {} 个文件 / {} 处匹配",
        matched_files.len(),
        total_matches
    )];
    if truncated_by_file_limit {
        lines.push(format!(
            "...（只展示前 {} 个命中文件，请继续缩小范围）",
            max_files
        ));
    }

    if files_with_matches {
        lines.extend(matched_files.iter().map(|file| file.path.clone()));
        return lines.join("\n");
    }

    for file in matched_files {
        lines.push(String::new());
        lines.push(file.path.clone());
        for entry in &file.lines {
            let separator = if entry.is_match { ':' } else { '-' };
            lines.push(format!("{}{} {}", entry.line_number, separator, entry.text));
        }
    }

    lines.join("\n")
}

fn grep_event_path(value: &Value) -> Option<String> {
    let data = value.get("data")?;
    let path = data.get("path")?;
    if let Some(text) = path.get("text").and_then(Value::as_str) {
        return Some(text.to_string());
    }
    let encoded = path.get("bytes").and_then(Value::as_str)?;
    STANDARD
        .decode(encoded)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
}

fn grep_event_text(value: &Value) -> Option<String> {
    let lines = value.get("data")?.get("lines")?;
    if let Some(text) = lines.get("text").and_then(Value::as_str) {
        return Some(text.trim_end_matches('\n').to_string());
    }
    let encoded = lines.get("bytes").and_then(Value::as_str)?;
    STANDARD
        .decode(encoded)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .map(|text| text.trim_end_matches('\n').to_string())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;

    use super::GrepTool;
    use crate::agent::tools::context::ToolContext;
    use crate::agent::tools::registry::AgentTool;

    fn ripgrep_available() -> bool {
        std::process::Command::new("rg")
            .arg("--version")
            .output()
            .is_ok()
    }

    fn create_workspace() -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("jkcodingagent-grep-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("src")).expect("create src");
        fs::write(
            root.join("src/search.rs"),
            "fn main() {\n    createD1HttpClient();\n    createD1HttpClient();\n}\n",
        )
        .expect("write rust file");
        fs::write(root.join("README.md"), "createD1HttpClient\n").expect("write readme");
        root
    }

    #[tokio::test]
    async fn grep_supports_glob_filters_and_precise_line_results() {
        if !ripgrep_available() {
            return;
        }

        let workspace = create_workspace();
        let context = ToolContext {
            workspace: workspace.clone(),
            workspace_id: "test-workspace".to_string(),
            dispatcher_db_path: workspace.join("dispatcher.db"),
            exec_timeout_secs: 30,
            restrict_to_workspace: true,
        };

        let output = GrepTool
            .execute(
                &json!({
                    "pattern": "createD1HttpClient",
                    "include": ["src/**/*.rs"],
                    "match_mode": "fixed"
                }),
                &context,
            )
            .await;

        assert!(output.contains("共 1 个文件 / 2 处匹配"));
        assert!(output.contains("src/search.rs"));
        assert!(output.contains("2:     createD1HttpClient();"));
        assert!(!output.contains("README.md"));

        let _ = fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn grep_can_return_only_matching_files() {
        if !ripgrep_available() {
            return;
        }

        let workspace = create_workspace();
        let context = ToolContext {
            workspace: workspace.clone(),
            workspace_id: "test-workspace".to_string(),
            dispatcher_db_path: workspace.join("dispatcher.db"),
            exec_timeout_secs: 30,
            restrict_to_workspace: true,
        };

        let output = GrepTool
            .execute(
                &json!({
                    "pattern": "createD1HttpClient",
                    "match_mode": "fixed",
                    "files_with_matches": true
                }),
                &context,
            )
            .await;

        assert!(output.contains("src/search.rs"));
        assert!(output.contains("README.md"));
        assert!(!output.contains("2:"));

        let _ = fs::remove_dir_all(workspace);
    }
}
