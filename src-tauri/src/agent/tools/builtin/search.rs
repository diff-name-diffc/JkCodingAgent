use std::collections::HashMap;
use std::fs;

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use futures::future::join_all;
use glob::glob;
use serde_json::{json, Value};
use tokio::{process::Command, task};

use super::common::{
    boolish_arg, is_noise, non_empty_string_array_arg, rel, render_labeled_sections, resolve_path,
    string_arg, string_array_arg, string_list_arg, usize_arg, with_compression_parameters,
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

#[derive(Debug)]
struct GrepOptions {
    include: Vec<String>,
    exclude: Vec<String>,
    match_mode: String,
    case_sensitive: Option<bool>,
    word: bool,
    context_before: usize,
    context_after: usize,
    max_matches_per_file: usize,
    max_files: usize,
    files_with_matches: bool,
    include_hidden: bool,
    no_ignore: bool,
}

#[async_trait]
impl AgentTool for GlobTool {
    fn name(&self) -> &'static str {
        "glob"
    }

    fn description(&self) -> &'static str {
        "按 glob 模式查找文件，结果按修改时间倒序排列。适合快速缩小文件范围。默认保留匹配文件列表，若只需要概览可设置 compress=true 并写明 compress_intent。"
    }

    fn parameters(&self) -> Value {
        with_compression_parameters(
            json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "匹配模式，例如 '*.rs' 或 'src/**/*.ts'" },
                    "patterns": {
                        "type": "array",
                        "description": "要批量搜索的 glob 模式列表。传入多个模式时，结果会按 pattern 分段返回。",
                        "items": { "type": "string" }
                    },
                    "paths": {
                        "type": "array",
                        "description": "搜索起始目录列表，默认 ['.']。即使只指定一个目录，也必须传单元素数组。与 patterns 同时提供时会搜索每个 path + pattern 组合。",
                        "minItems": 1,
                        "items": { "type": "string" }
                    },
                    "max_results": { "type": "integer", "description": "最多返回多少个结果，默认 250", "minimum": 1 }
                },
                "anyOf": [
                    { "required": ["pattern"] },
                    { "required": ["patterns"] }
                ]
            }),
            false,
            "当后续需要精确文件列表时保持关闭保留完整结果；只看分布或概况时可开启并写明 compress_intent。",
        )
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
        let patterns = match string_list_arg(args, "pattern", "patterns") {
            Ok(patterns) => patterns,
            Err(message) => return message,
        };
        let paths =
            non_empty_string_array_arg(args, "paths").unwrap_or_else(|| vec![".".to_string()]);
        let max_results = usize_arg(args, "max_results").unwrap_or(250).max(1);
        let context = context.clone();

        match task::spawn_blocking(move || {
            let mut sections = Vec::new();
            for path in &paths {
                for pattern in &patterns {
                    sections.push((
                        format!("glob path={path} pattern={pattern}"),
                        run_glob_query(path, pattern, max_results, &context),
                    ));
                }
            }

            render_single_or_grouped_sections(sections)
        })
        .await
        {
            Ok(output) => output,
            Err(error) => format!("glob 搜索任务失败：{error}"),
        }
    }
}

#[async_trait]
impl AgentTool for GrepTool {
    fn name(&self) -> &'static str {
        "grep"
    }

    fn description(&self) -> &'static str {
        "使用 ripgrep 在工作区内搜索文本。推荐先用 glob 缩小文件范围，再用 grep 精确定位符号、配置键或错误文本，最后用 read_file 读取确认。compress=false 时严格禁止摘要；超过 2000 字符的匹配结果会返回前 2000 字符，并标明截断行和完整产物位置。只有 compress=true 且结果超过 5000 字符时才进行摘要。"
    }

    fn parameters(&self) -> Value {
        with_compression_parameters(
            json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "要搜索的模式。默认按正则处理。" },
                    "patterns": {
                        "type": "array",
                        "description": "要批量搜索的模式列表。传入多个模式时，结果会按 pattern 分段返回。",
                        "items": { "type": "string" }
                    },
                    "paths": {
                        "type": "array",
                        "description": "搜索起点列表，默认 ['.']；可传目录或单个文件。即使只指定一个搜索起点，也必须传单元素数组。与 patterns 同时提供时会搜索每个 path + pattern 组合。",
                        "minItems": 1,
                        "items": { "type": "string" }
                    },
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
                "anyOf": [
                    { "required": ["pattern"] },
                    { "required": ["patterns"] }
                ]
            }),
            false,
            "grep 是精确检索工具；compress=false 时不得摘要，超过 2000 字符则带行号信息截断。只在超长结果需要按路径和行号提取多段关键内容时开启 compress=true 并写明 compress_intent。",
        )
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
        let patterns = match string_list_arg(args, "pattern", "patterns") {
            Ok(patterns) => patterns,
            Err(message) => return message,
        };
        let paths =
            non_empty_string_array_arg(args, "paths").unwrap_or_else(|| vec![".".to_string()]);
        let options = GrepOptions {
            include: string_array_arg(args, "include").unwrap_or_default(),
            exclude: string_array_arg(args, "exclude").unwrap_or_default(),
            match_mode: string_arg(args, "match_mode").unwrap_or_else(|| "regex".to_string()),
            case_sensitive: args.get("case_sensitive").and_then(Value::as_bool),
            word: boolish_arg(args, "word").unwrap_or(false),
            context_before: usize_arg(args, "context_before").unwrap_or(0),
            context_after: usize_arg(args, "context_after").unwrap_or(0),
            max_matches_per_file: usize_arg(args, "max_matches_per_file").unwrap_or(20).max(1),
            max_files: usize_arg(args, "max_files").unwrap_or(25).max(1),
            files_with_matches: boolish_arg(args, "files_with_matches").unwrap_or(false),
            include_hidden: boolish_arg(args, "include_hidden").unwrap_or(false),
            no_ignore: boolish_arg(args, "no_ignore").unwrap_or(false),
        };

        let queries = patterns
            .iter()
            .flat_map(|pattern| {
                paths
                    .iter()
                    .map(move |path| (pattern.clone(), path.clone()))
            })
            .collect::<Vec<_>>();
        let sections = join_all(queries.into_iter().map(|(pattern, path)| {
            let options = &options;
            async move {
                (
                    format!("grep path={path} pattern={pattern}"),
                    run_grep_query(&pattern, &path, options, context).await,
                )
            }
        }))
        .await;

        render_single_or_grouped_sections(sections)
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

fn run_glob_query(path: &str, pattern: &str, max_results: usize, context: &ToolContext) -> String {
    let dir_path = match resolve_path(context, path) {
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
    let mut matches_with_metadata = matches
        .into_iter()
        .map(|path| {
            let modified = fs::metadata(&path)
                .and_then(|metadata| metadata.modified())
                .ok();
            (path, modified)
        })
        .collect::<Vec<_>>();
    matches_with_metadata.sort_by_key(|(_, modified)| *modified);
    matches_with_metadata.reverse();

    if matches_with_metadata.is_empty() {
        return format!("未找到匹配文件：{}", dir_path.display());
    }

    let mut lines = matches_with_metadata
        .iter()
        .take(max_results)
        .map(|(path, _)| rel(path, &dir_path))
        .collect::<Vec<_>>();
    if matches_with_metadata.len() > max_results {
        lines.push(format!(
            "...（已显示 {} / {}）",
            max_results,
            matches_with_metadata.len()
        ));
    }
    lines.join("\n")
}

async fn run_grep_query(
    pattern: &str,
    path: &str,
    options: &GrepOptions,
    context: &ToolContext,
) -> String {
    let search_path = match resolve_path(context, path) {
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
        .arg(options.max_matches_per_file.to_string())
        .current_dir(&workspace)
        .kill_on_drop(true);

    match options.case_sensitive {
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

    if options.match_mode == "fixed" {
        command.arg("--fixed-strings");
    }
    if options.word {
        command.arg("--word-regexp");
    }
    if options.include_hidden {
        command.arg("--hidden");
    }
    if options.no_ignore {
        command.arg("--no-ignore");
    }
    if options.context_before == options.context_after && options.context_before > 0 {
        command
            .arg("--context")
            .arg(options.context_before.to_string());
    } else {
        if options.context_before > 0 {
            command
                .arg("--before-context")
                .arg(options.context_before.to_string());
        }
        if options.context_after > 0 {
            command
                .arg("--after-context")
                .arg(options.context_after.to_string());
        }
    }

    for glob in &options.include {
        command.arg("--glob").arg(glob);
    }
    for glob in &options.exclude {
        command.arg("--glob").arg(format!("!{glob}"));
    }

    command.arg(pattern).arg(&target);

    let output =
        match tokio::time::timeout(std::time::Duration::from_secs(60), command.output()).await {
            Ok(Ok(output)) => output,
            Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                return run_grep_fallback(pattern, &search_path, options).await;
            }
            Ok(Err(error)) => return format!("执行 grep 搜索失败：{error}"),
            Err(_) => return "grep 搜索超时（60 秒）".to_string(),
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

    let rendered = render_grep_stdout(&stdout, options.max_files, options.files_with_matches);
    if rendered.is_empty() {
        return format!("未找到匹配内容：{pattern}");
    }
    rendered
}

async fn run_grep_fallback(
    pattern: &str,
    search_path: &std::path::Path,
    options: &GrepOptions,
) -> String {
    let mut command = Command::new("grep");
    command.arg("-rn").arg("--no-color");

    if options.match_mode == "fixed" {
        command.arg("-F");
    }
    if matches!(options.case_sensitive, Some(false) | None) {
        command.arg("-i");
    }
    if options.word {
        command.arg("-w");
    }
    if options.context_before == options.context_after && options.context_before > 0 {
        command.arg("-C").arg(options.context_before.to_string());
    } else {
        if options.context_before > 0 {
            command.arg("-B").arg(options.context_before.to_string());
        }
        if options.context_after > 0 {
            command.arg("-A").arg(options.context_after.to_string());
        }
    }
    for glob in &options.include {
        command.arg(format!("--include={glob}"));
    }
    for glob in &options.exclude {
        command.arg(format!("--exclude={glob}"));
    }

    command.arg(pattern).arg(search_path);

    let output =
        match tokio::time::timeout(std::time::Duration::from_secs(60), command.output()).await {
            Ok(Ok(output)) => output,
            Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                return "错误：未找到 ripgrep (`rg`) 或系统 grep，无法执行搜索".to_string();
            }
            Ok(Err(error)) => return format!("grep 回退搜索失败：{error}"),
            Err(_) => return "grep 回退搜索超时（60 秒）".to_string(),
        };

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let status = output.status.code().unwrap_or_default();
    if status != 0 && status != 1 {
        return format!("grep 回退搜索失败，退出状态：{}", output.status);
    }
    if stdout.trim().is_empty() {
        return format!("未找到匹配内容：{pattern}");
    }
    render_grep_fallback_output(&stdout, options.max_files, options.files_with_matches)
}

fn render_grep_fallback_output(stdout: &str, max_files: usize, files_with_matches: bool) -> String {
    let mut file_results: Vec<(String, Vec<String>)> = Vec::new();
    let mut file_index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut total_matches = 0usize;

    for line in stdout.lines() {
        let Some(colon_pos) = line.find(':') else {
            continue;
        };
        let path = &line[..colon_pos];
        if is_noise(std::ffi::OsStr::new(
            std::path::Path::new(path).file_name().unwrap_or_default(),
        )) {
            continue;
        }
        let rest = &line[colon_pos + 1..];
        let is_match = rest
            .find(':')
            .is_some_and(|p| rest[..p].chars().all(|c| c.is_ascii_digit()));

        if !is_match {
            continue;
        }

        let entry = match file_index.get(path) {
            Some(&idx) => Some(idx),
            None if file_results.len() < max_files => {
                let idx = file_results.len();
                file_results.push((path.to_string(), Vec::new()));
                file_index.insert(path.to_string(), idx);
                Some(idx)
            }
            None => None,
        };
        if let Some(idx) = entry {
            total_matches += 1;
            if files_with_matches {
                continue;
            }
            file_results[idx].1.push(rest.to_string());
        }
    }

    let matched_files = file_results
        .iter()
        .filter(|(_, lines)| !lines.is_empty())
        .collect::<Vec<_>>();
    if matched_files.is_empty() {
        return String::new();
    }

    let mut lines = vec![format!(
        "共 {} 个文件 / {} 处匹配 (grep 回退)",
        matched_files.len(),
        total_matches
    )];

    if files_with_matches {
        lines.extend(matched_files.iter().map(|(path, _)| path.clone()));
        return lines.join("\n");
    }

    for (path, file_lines) in matched_files {
        lines.push(String::new());
        lines.push(path.clone());
        lines.extend(file_lines.iter().cloned());
    }
    lines.join("\n")
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
