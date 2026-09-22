//! glob / grep 工具（rig 形态）。迁移自旧 `tools/builtin/search.rs` 与
//! `tools/builtin/search/` 子模块：ripgrep 主后端、系统 grep 回退、有界子进程
//! 输出、JSON/文本渲染全部保留。旧 `ToolContext` 收敛为构造期 `FsSandbox`。
//! 偏差：旧的结构化 `data` 载荷（queries/files/totalMatches/backend 等）不再
//! 产出——rig runtime 只持久化原始文本，模型侧展示文本保持一致。

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::fs;
use std::io;
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use futures::future::join_all;
use glob::{MatchOptions, Pattern};
use serde_json::{json, Value};
use rig::tool::{PortableDynamicTool, ToolExecutionError, ToolOutput};
use tokio::io::AsyncReadExt;
use tokio::{process::Command, task};
use walkdir::WalkDir;

use super::super::common::{
    boolish_arg, is_noise, lexical_normalize, non_empty_string_array_arg, rel,
    render_labeled_sections, string_arg, string_array_arg, string_list_arg, usize_arg,
    with_compression_parameters, DEFAULT_FORCE_COMPRESS_AFTER_CHARS,
};
use super::FsSandbox;
use crate::workspace::fs::IGNORED_DIRS;

mod glob_backend;
mod grep_backend;
mod grep_fallback;
mod grep_process;
mod grep_render;

use glob_backend::*;
use grep_backend::*;
use grep_fallback::*;
use grep_process::*;
use grep_render::*;

/// macOS GUI 进程不继承 shell 的 PATH（Homebrew/cargo 等目录缺失），
/// 父进程 PATH 缺这些目录时 `Command::new("rg")` 在已安装 ripgrep 的机器上
/// 也会 NotFound，静默降级到系统 grep。不能靠 `.env("PATH", ...)` 修：
/// 它只改写子进程的环境块，而各平台对「envp 中的 PATH 是否参与可执行文件
/// 解析」行为不一——macOS 的 posix_spawn 由内核按 envp 的 PATH 解析，
/// Linux 的 execvp/posix_spawnp 却按**调用进程**的 PATH 解析，Windows 同样
/// 用父进程 PATH。因此在 spawn 前于父进程侧显式探测候选目录，命中即以绝对
/// 路径启动；未命中原样返回名字，由 spawn 抛 NotFound 触发 grep 回退。
fn resolve_tool(name: &str) -> std::path::PathBuf {
    let mut dirs: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(existing) = std::env::var("PATH") {
        dirs.extend(std::env::split_paths(&existing));
    }
    // dirs::home_dir 在 Windows 上走 USERPROFILE（该环境通常没有 HOME）。
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".cargo/bin"));
        dirs.push(home.join(".local/bin"));
    }
    #[cfg(not(windows))]
    for dir in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"] {
        dirs.push(std::path::PathBuf::from(dir));
    }
    // 保序去重（HashSet 记录已见项）：补齐的目录与原 PATH 中的重复项通常
    // 不相邻，retain 的相邻去重无法收敛 PATH 体积。
    let mut seen = std::collections::HashSet::new();
    for dir in dirs.into_iter().filter(|dir| seen.insert(dir.clone())) {
        for candidate in tool_file_names(name) {
            let path = dir.join(candidate);
            if is_executable_file(&path) {
                return path;
            }
        }
    }
    std::path::PathBuf::from(name)
}

/// Windows 上可执行文件带 .exe 后缀；其余 PATHEXT 变体与 rg/grep 无关。
#[cfg(windows)]
fn tool_file_names(name: &str) -> Vec<String> {
    vec![format!("{name}.exe"), name.to_string()]
}

#[cfg(not(windows))]
fn tool_file_names(name: &str) -> Vec<String> {
    vec![name.to_string()]
}

/// 常规文件判定；Unix 额外要求任一可执行位。fs::metadata 跟随符号链接
/// （Homebrew/cargo bin 目录中的工具多为符号链接）。
fn is_executable_file(path: &std::path::Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    metadata.is_file()
}

pub(super) fn glob_tool(sandbox: FsSandbox) -> PortableDynamicTool {
    PortableDynamicTool::new(
        "glob",
        "按 glob 模式查找文件，结果按修改时间倒序排列。适合快速缩小文件范围。默认保留匹配文件列表，若只需要概览可设置 compress=true 并写明 compress_intent。",
        glob_parameters(),
        move |args| {
            let sandbox = sandbox.clone();
            Box::pin(async move { execute_glob(args, sandbox).await })
        },
    )
}

pub(super) fn grep_tool(sandbox: FsSandbox) -> PortableDynamicTool {
    PortableDynamicTool::new(
        "grep",
        "使用 ripgrep 在工作区内搜索文本。推荐先用 glob 缩小文件范围，再用 grep 精确定位符号、配置键或错误文本，最后用 read_file 读取确认。compress=false 时严格禁止摘要；超过 10000 字符的匹配结果会返回前 10000 字符，并标明截断行和完整产物位置。只有 compress=true 且结果超过 5000 字符时才进行摘要。",
        grep_parameters(),
        move |args| {
            let sandbox = sandbox.clone();
            Box::pin(async move { execute_grep(args, sandbox).await })
        },
    )
}

/// 单次 grep 调用内并发执行的 rg/grep 子进程上限（patterns×paths 组合数封顶）。
const MAX_CONCURRENT_GREP_QUERIES: usize = 4;
/// 单个 glob 查询最多检查的目录项数。结果数上限只约束返回值，不能约束
/// `**/*` 的遍历成本；扫描预算是避免超大仓库拖死后台线程的第二道边界。
const MAX_GLOB_SCAN_ENTRIES: usize = 100_000;

/// 搜索后端可能在 Rust 侧命中 `max_files` 之前产生大量输出。必须在读取
/// 子进程管道时限流，而不是等 `Command::output` 将全部内容装进内存后再截断。
const MAX_GREP_STDOUT_BYTES: usize = 8 * 1024 * 1024;
/// 单次 grep 工具调用中所有 pattern×path 查询共享的 stdout 总预算。
/// 每个子进程在 spawn 前获得固定份额，不能先全量读取、完成后再聚合截断。
const MAX_GREP_CALL_STDOUT_BYTES: usize = 16 * 1024 * 1024;
const MAX_GREP_STDERR_BYTES: usize = 64 * 1024;
const GREP_TIMEOUT: Duration = Duration::from_secs(60);

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

struct SearchOutcome {
    display: String,
    is_error: bool,
}

struct GrepRendered {
    display: String,
    total_matches: usize,
    truncated: bool,
}

struct BoundedCommandOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    truncated: bool,
}

impl SearchOutcome {
    fn error(message: String) -> Self {
        Self {
            display: message,
            is_error: true,
        }
    }

    fn ok(display: String) -> Self {
        Self {
            display,
            is_error: false,
        }
    }
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

fn glob_parameters() -> Value {
    with_compression_parameters(
        json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "pattern": { "type": "string", "minLength": 1, "maxLength": 4096, "description": "匹配模式，例如 '*.rs' 或 'src/**/*.ts'" },
                "patterns": {
                    "type": "array",
                    "description": "要批量搜索的 glob 模式列表。传入多个模式时，结果会按 pattern 分段返回。",
                    "minItems": 1,
                    "maxItems": 4,
                    "items": { "type": "string", "minLength": 1, "maxLength": 4096 }
                },
                "paths": {
                    "type": "array",
                    "description": "搜索起始目录列表，默认 ['.']。即使只指定一个目录，也必须传单元素数组。与 patterns 同时提供时会搜索每个 path + pattern 组合。",
                    "minItems": 1,
                    "maxItems": 4,
                    "items": { "type": "string", "minLength": 1, "maxLength": 4096 }
                },
                "max_results": { "type": "integer", "description": "每个 path + pattern 组合最多返回多少个结果，默认 250", "minimum": 1, "maximum": 1000 }
            },
            "anyOf": [
                { "required": ["pattern"] },
                { "required": ["patterns"] }
            ]
        }),
        false,
        DEFAULT_FORCE_COMPRESS_AFTER_CHARS,
        "当后续需要精确文件列表时保持关闭保留完整结果；只看分布或概况时可开启并写明 compress_intent。",
    )
}

async fn execute_glob(args: Value, sandbox: FsSandbox) -> Result<ToolOutput, ToolExecutionError> {
    let patterns = match string_list_arg(&args, "pattern", "patterns") {
        Ok(patterns) => patterns,
        Err(message) => return Err(ToolExecutionError::invalid_args(message)),
    };
    let paths =
        non_empty_string_array_arg(&args, "paths").unwrap_or_else(|| vec![".".to_string()]);
    let max_results = usize_arg(&args, "max_results").unwrap_or(250).max(1);

    match task::spawn_blocking(move || {
        let mut outcomes = Vec::new();
        for path in &paths {
            for pattern in &patterns {
                outcomes.push((
                    path,
                    pattern,
                    run_glob_query(path, pattern, max_results, &sandbox),
                ));
            }
        }
        let display = render_single_or_grouped_sections(
            outcomes
                .iter()
                .map(|(path, pattern, outcome)| {
                    (
                        format!("glob path={path} pattern={pattern}"),
                        outcome.display.clone(),
                    )
                })
                .collect(),
        );
        let all_failed =
            !outcomes.is_empty() && outcomes.iter().all(|(_, _, outcome)| outcome.is_error);
        (display, all_failed)
    })
    .await
    {
        Ok((display, true)) => Err(ToolExecutionError::other(display)),
        Ok((display, false)) => Ok(ToolOutput::text(display)),
        Err(error) => Err(ToolExecutionError::other(format!(
            "错误：glob 搜索任务失败：{error}"
        ))),
    }
}

fn grep_parameters() -> Value {
    with_compression_parameters(
        json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "pattern": { "type": "string", "minLength": 1, "maxLength": 4096, "description": "要搜索的模式。默认按正则处理。" },
                "patterns": {
                    "type": "array",
                    "description": "要批量搜索的模式列表。传入多个模式时，结果会按 pattern 分段返回。",
                    "minItems": 1,
                    "maxItems": 4,
                    "items": { "type": "string", "minLength": 1, "maxLength": 4096 }
                },
                "paths": {
                    "type": "array",
                    "description": "搜索起点列表，默认 ['.']；可传目录或单个文件。即使只指定一个搜索起点，也必须传单元素数组。与 patterns 同时提供时会搜索每个 path + pattern 组合。",
                    "minItems": 1,
                    "maxItems": 4,
                    "items": { "type": "string", "minLength": 1, "maxLength": 4096 }
                },
                "include": {
                    "type": "array",
                    "description": "可选的 glob 过滤列表，例如 ['src/**/*.rs', 'src/**/*.ts']",
                    "maxItems": 16,
                    "items": { "type": "string", "minLength": 1, "maxLength": 1024 }
                },
                "exclude": {
                    "type": "array",
                    "description": "要排除的 glob 列表，例如 ['target/**', 'dist/**']",
                    "maxItems": 16,
                    "items": { "type": "string", "minLength": 1, "maxLength": 1024 }
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
                "context_before": { "type": "integer", "description": "前置上下文行数，默认 0", "minimum": 0, "maximum": 50 },
                "context_after": { "type": "integer", "description": "后置上下文行数，默认 0", "minimum": 0, "maximum": 50 },
                "max_matches_per_file": {
                    "type": "integer",
                    "description": "每个文件最多返回多少处匹配，默认 20",
                    "minimum": 1,
                    "maximum": 200
                },
                "max_files": {
                    "type": "integer",
                    "description": "最多展示多少个命中文件，默认 25",
                    "minimum": 1,
                    "maximum": 200
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
        DEFAULT_FORCE_COMPRESS_AFTER_CHARS,
        "grep 是精确检索工具；compress=false 时不得摘要，超过 10000 字符则带行号信息截断。只在超长结果需要按路径和行号提取多段关键内容时开启 compress=true 并写明 compress_intent。",
    )
}

async fn execute_grep(args: Value, sandbox: FsSandbox) -> Result<ToolOutput, ToolExecutionError> {
    let patterns = match string_list_arg(&args, "pattern", "patterns") {
        Ok(patterns) => patterns,
        Err(message) => return Err(ToolExecutionError::invalid_args(message)),
    };
    let paths =
        non_empty_string_array_arg(&args, "paths").unwrap_or_else(|| vec![".".to_string()]);
    let options = GrepOptions {
        include: string_array_arg(&args, "include").unwrap_or_default(),
        exclude: string_array_arg(&args, "exclude").unwrap_or_default(),
        match_mode: string_arg(&args, "match_mode").unwrap_or_else(|| "regex".to_string()),
        case_sensitive: args.get("case_sensitive").and_then(Value::as_bool),
        word: boolish_arg(&args, "word").unwrap_or(false),
        context_before: usize_arg(&args, "context_before").unwrap_or(0),
        context_after: usize_arg(&args, "context_after").unwrap_or(0),
        max_matches_per_file: usize_arg(&args, "max_matches_per_file").unwrap_or(20).max(1),
        max_files: usize_arg(&args, "max_files").unwrap_or(25).max(1),
        files_with_matches: boolish_arg(&args, "files_with_matches").unwrap_or(false),
        include_hidden: boolish_arg(&args, "include_hidden").unwrap_or(false),
        no_ignore: boolish_arg(&args, "no_ignore").unwrap_or(false),
    };

    let queries = patterns
        .iter()
        .flat_map(|pattern| {
            paths
                .iter()
                .map(move |path| (pattern.clone(), path.clone()))
        })
        .collect::<Vec<_>>();
    let stdout_limit = grep_stdout_budget_per_query(queries.len());
    // 并发上限：patterns×paths 组合过多时限制同时启动的 rg/grep 子进程数，
    // 避免一次调用瞬间拉起数十上百个进程扫满工作区造成资源耗尽。
    let mut outcomes = Vec::new();
    for chunk in queries.chunks(MAX_CONCURRENT_GREP_QUERIES) {
        let futures = chunk.iter().map(|(pattern, path)| {
            let options = &options;
            let sandbox = &sandbox;
            async move {
                (
                    path,
                    pattern,
                    run_grep_query(pattern, path, options, sandbox, stdout_limit).await,
                )
            }
        });
        outcomes.extend(join_all(futures).await);
    }

    let display = render_single_or_grouped_sections(
        outcomes
            .iter()
            .map(|(path, pattern, outcome)| {
                (
                    format!("grep path={path} pattern={pattern}"),
                    outcome.display.clone(),
                )
            })
            .collect(),
    );
    let all_failed =
        !outcomes.is_empty() && outcomes.iter().all(|(_, _, outcome)| outcome.is_error);
    if all_failed {
        Err(ToolExecutionError::other(display))
    } else {
        Ok(ToolOutput::text(display))
    }
}

fn grep_stdout_budget_per_query(query_count: usize) -> usize {
    (MAX_GREP_CALL_STDOUT_BYTES / query_count.max(1)).min(MAX_GREP_STDOUT_BYTES)
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

#[cfg(test)]
mod tests;
