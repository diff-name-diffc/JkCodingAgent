use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::fs;
use std::io;
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use futures::future::join_all;
use glob::{MatchOptions, Pattern};
use serde_json::{json, Value};
use tokio::io::AsyncReadExt;
use tokio::{process::Command, task};
use walkdir::WalkDir;

use super::common::{
    boolish_arg, is_noise, lexical_normalize, non_empty_string_array_arg, rel,
    render_labeled_sections, resolve_path, string_arg, string_array_arg, string_list_arg,
    usize_arg, with_compression_parameters,
};
use crate::agent::tools::context::ToolContext;
use crate::agent::tools::registry::AgentTool;
use crate::agent::tools::ToolResult;
use crate::workspace::fs::IGNORED_DIRS;

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

pub(super) fn glob_tool() -> Box<dyn AgentTool> {
    Box::new(GlobTool)
}

pub(super) fn grep_tool() -> Box<dyn AgentTool> {
    Box::new(GrepTool)
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

struct SearchOutcome {
    display: String,
    data: Value,
}

struct GrepRendered {
    display: String,
    files: Vec<Value>,
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
    fn error(kind: &str, path: &str, pattern: &str, message: String) -> Self {
        Self {
            display: message.clone(),
            data: json!({
                "kind": kind,
                "path": path,
                "pattern": pattern,
                "error": message,
            }),
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
            "当后续需要精确文件列表时保持关闭保留完整结果；只看分布或概况时可开启并写明 compress_intent。",
        )
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> ToolResult {
        let patterns = match string_list_arg(args, "pattern", "patterns") {
            Ok(patterns) => patterns,
            Err(message) => return ToolResult::recoverable_error(message),
        };
        let paths =
            non_empty_string_array_arg(args, "paths").unwrap_or_else(|| vec![".".to_string()]);
        let max_results = usize_arg(args, "max_results").unwrap_or(250).max(1);
        let context = context.clone();

        match task::spawn_blocking(move || {
            let mut outcomes = Vec::new();
            for path in &paths {
                for pattern in &patterns {
                    outcomes.push((
                        path,
                        pattern,
                        run_glob_query(path, pattern, max_results, &context),
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
            let all_failed = !outcomes.is_empty()
                && outcomes
                    .iter()
                    .all(|(_, _, outcome)| outcome.data.get("error").is_some());
            let data = json!({
                "queries": outcomes.into_iter().map(|(_, _, outcome)| outcome.data).collect::<Vec<_>>()
            });
            (display, data, all_failed)
        })
        .await
        {
            Ok((display, data, true)) => ToolResult::recoverable_error(display).with_data(data),
            Ok((display, data, false)) => ToolResult::success_data(data, display.clone(), display),
            Err(error) => {
                ToolResult::recoverable_error(format!("错误：glob 搜索任务失败：{error}"))
            }
        }
    }
}

#[async_trait]
impl AgentTool for GrepTool {
    fn name(&self) -> &'static str {
        "grep"
    }

    fn description(&self) -> &'static str {
        "使用 ripgrep 在工作区内搜索文本。推荐先用 glob 缩小文件范围，再用 grep 精确定位符号、配置键或错误文本，最后用 read_file 读取确认。compress=false 时严格禁止摘要；超过 10000 字符的匹配结果会返回前 10000 字符，并标明截断行和完整产物位置。只有 compress=true 且结果超过 5000 字符时才进行摘要。"
    }

    fn parameters(&self) -> Value {
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
            "grep 是精确检索工具；compress=false 时不得摘要，超过 10000 字符则带行号信息截断。只在超长结果需要按路径和行号提取多段关键内容时开启 compress=true 并写明 compress_intent。",
        )
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> ToolResult {
        let patterns = match string_list_arg(args, "pattern", "patterns") {
            Ok(patterns) => patterns,
            Err(message) => return ToolResult::recoverable_error(message),
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
        let stdout_limit = grep_stdout_budget_per_query(queries.len());
        // 并发上限：patterns×paths 组合过多时限制同时启动的 rg/grep 子进程数，
        // 避免一次调用瞬间拉起数十上百个进程扫满工作区造成资源耗尽。
        let mut outcomes = Vec::new();
        for chunk in queries.chunks(MAX_CONCURRENT_GREP_QUERIES) {
            let futures = chunk.iter().map(|(pattern, path)| {
                let options = &options;
                async move {
                    (
                        path,
                        pattern,
                        run_grep_query(pattern, path, options, context, stdout_limit).await,
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
        let all_failed = !outcomes.is_empty()
            && outcomes
                .iter()
                .all(|(_, _, outcome)| outcome.data.get("error").is_some());
        let data = json!({
            "queries": outcomes.into_iter().map(|(_, _, outcome)| outcome.data).collect::<Vec<_>>(),
            "stdoutBudgetBytes": MAX_GREP_CALL_STDOUT_BYTES,
            "perQueryStdoutBudgetBytes": stdout_limit,
        });
        if all_failed {
            ToolResult::recoverable_error(display).with_data(data)
        } else {
            ToolResult::success_data(data, display.clone(), display)
        }
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

fn safe_glob_entries(
    root: &Path,
    max_depth: Option<usize>,
) -> impl Iterator<Item = Result<walkdir::DirEntry, walkdir::Error>> {
    let mut builder = WalkDir::new(root).follow_links(false).min_depth(1);
    if let Some(max_depth) = max_depth {
        builder = builder.max_depth(max_depth);
    }
    builder
        .into_iter()
        .filter_entry(|entry| !entry.file_type().is_symlink() && !is_noise(entry.file_name()))
}

fn run_glob_query(
    path: &str,
    pattern: &str,
    max_results: usize,
    context: &ToolContext,
) -> SearchOutcome {
    let dir_path = match resolve_path(context, path) {
        Ok(path) => path,
        Err(message) => return SearchOutcome::error("glob", path, pattern, message),
    };
    let search_pattern = match glob_search_pattern(context, &dir_path, pattern) {
        Ok(search_pattern) => search_pattern,
        Err(message) => return SearchOutcome::error("glob", path, pattern, message),
    };
    let relative_pattern = match Path::new(&search_pattern).strip_prefix(&dir_path) {
        Ok(pattern) => pattern,
        Err(_) => {
            return SearchOutcome::error(
                "glob",
                path,
                pattern,
                "错误：glob 模式无法转换为工作区相对路径".to_string(),
            )
        }
    };
    let matcher = match Pattern::new(&relative_pattern.to_string_lossy()) {
        Ok(matcher) => matcher,
        Err(error) => {
            return SearchOutcome::error(
                "glob",
                path,
                pattern,
                format!("错误：glob 模式无效：{error}"),
            )
        }
    };
    let max_depth = relative_pattern
        .components()
        .all(|component| component.as_os_str() != "**")
        .then(|| relative_pattern.components().count());
    let match_options = MatchOptions {
        case_sensitive: true,
        require_literal_separator: true,
        require_literal_leading_dot: false,
    };

    // 只保留修改时间最新的 max_results 项；总命中数单独计数。旧实现先把
    // 整个工作区所有匹配路径与 metadata 全量收进 Vec 后才截断，`**/*`
    // 可轻易把单次调用变成无界内存增长。
    let mut newest: BinaryHeap<Reverse<(Option<std::time::SystemTime>, std::path::PathBuf)>> =
        BinaryHeap::with_capacity(max_results.saturating_add(1));
    let mut total_matches = 0usize;
    let mut scanned_entries = 0usize;
    let mut scan_truncated = false;
    for entry in safe_glob_entries(&dir_path, max_depth) {
        if scanned_entries >= MAX_GLOB_SCAN_ENTRIES {
            scan_truncated = true;
            break;
        }
        scanned_entries += 1;
        match entry {
            Ok(entry) => {
                let path = entry.into_path();
                let relative = path.strip_prefix(&dir_path).unwrap_or(&path);
                if !matcher.matches_path_with(relative, match_options) {
                    continue;
                }
                total_matches = total_matches.saturating_add(1);
                let modified = fs::symlink_metadata(&path)
                    .and_then(|metadata| metadata.modified())
                    .ok();
                let candidate = (modified, path);
                if newest.len() < max_results {
                    newest.push(Reverse(candidate));
                } else if newest
                    .peek()
                    .is_some_and(|Reverse(oldest)| candidate > *oldest)
                {
                    newest.pop();
                    newest.push(Reverse(candidate));
                }
            }
            Err(error) => {
                return SearchOutcome::error(
                    "glob",
                    path,
                    pattern,
                    format!("错误：glob 搜索失败：{error}"),
                )
            }
        }
    }
    let mut matches_with_metadata = newest
        .into_iter()
        .map(|Reverse((modified, path))| (path, modified))
        .collect::<Vec<_>>();
    matches_with_metadata.sort_by_key(|(_, modified)| *modified);
    matches_with_metadata.reverse();

    if matches_with_metadata.is_empty() {
        let display = if scan_truncated {
            format!(
                "在前 {MAX_GLOB_SCAN_ENTRIES} 个目录项中未找到匹配文件；扫描已达上限，请缩小路径或模式：{}",
                dir_path.display()
            )
        } else {
            format!("未找到匹配文件：{}", dir_path.display())
        };
        return SearchOutcome {
            display,
            data: json!({
                "kind": "glob",
                "path": path,
                "pattern": pattern,
                "matches": [],
                "total": 0,
                "truncated": scan_truncated,
                "scanTruncated": scan_truncated,
                "scannedEntries": scanned_entries,
            }),
        };
    }

    let matches = matches_with_metadata
        .iter()
        .map(|(path, _)| rel(path, &dir_path))
        .collect::<Vec<_>>();
    let mut lines = matches.clone();
    let result_truncated = total_matches > matches.len();
    if result_truncated {
        lines.push(format!(
            "...（已显示 {} / {}）",
            matches.len(),
            total_matches
        ));
    }
    if scan_truncated {
        lines.push(format!(
            "...（扫描达到 {MAX_GLOB_SCAN_ENTRIES} 个目录项上限；命中总数仅代表已扫描范围，请缩小路径或模式）"
        ));
    }
    SearchOutcome {
        display: lines.join("\n"),
        data: json!({
            "kind": "glob",
            "path": path,
            "pattern": pattern,
            "matches": matches,
            "total": total_matches,
            "truncated": result_truncated || scan_truncated,
            "scanTruncated": scan_truncated,
            "scannedEntries": scanned_entries,
        }),
    }
}

/// 将起始目录与用户传入的 glob 模式拼接后做归一化与工作区校验，防止
/// 「pattern 为绝对路径」或「pattern 含 `..`」绕过目录校验遍历工作区之外的文件。
/// glob 通配符无法 canonicalize，因此用词法归一化 + 前缀包含判断（fail-closed）。
fn glob_search_pattern(
    context: &ToolContext,
    dir_path: &Path,
    pattern: &str,
) -> Result<String, String> {
    let joined = lexical_normalize(&dir_path.join(pattern));

    if context.restrict_to_workspace {
        let mut allowed_roots = vec![context.workspace.clone()];
        allowed_roots.extend(context.extra_allowed_dirs.iter().cloned());
        if !path_within_allowed_roots(&joined, &allowed_roots) {
            return Err(format!(
                "错误：glob 模式越界，禁止搜索工作区之外的路径：{pattern}"
            ));
        }
    }

    joined
        .to_str()
        .map(str::to_string)
        .ok_or_else(|| "错误：glob 模式不是有效的 UTF-8".to_string())
}

/// 判断（词法归一化后的）路径是否位于任一允许根目录之内。
/// 允许根目录无法 canonicalize 时按不包含处理（fail-closed）。
fn path_within_allowed_roots(joined: &Path, allowed_roots: &[std::path::PathBuf]) -> bool {
    allowed_roots.iter().any(|root| {
        root.canonicalize()
            .map(|canonical| joined.starts_with(&canonical))
            .unwrap_or(false)
    })
}

/// 以硬上限并发读取 stdout/stderr。任一管道超过上限时立即终止子进程，
/// 但保留已经读取的完整前缀供调用方渲染并显式标记 truncated。
async fn run_bounded_search_command(
    mut command: Command,
    stdout_limit: usize,
) -> io::Result<BoundedCommandOutput> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn()?;

    match tokio::time::timeout(
        GREP_TIMEOUT,
        collect_bounded_output(&mut child, stdout_limit),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "grep 搜索超时（60 秒）",
            ))
        }
    }
}

async fn collect_bounded_output(
    child: &mut tokio::process::Child,
    stdout_limit: usize,
) -> io::Result<BoundedCommandOutput> {
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("grep stdout 管道未创建"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("grep stderr 管道未创建"))?;
    let mut stdout_bytes = Vec::new();
    let mut stderr_bytes = Vec::new();
    let mut stdout_done = false;
    let mut stderr_done = false;
    let mut status = None;
    let mut truncated = false;
    let mut stdout_chunk = [0u8; 16 * 1024];
    let mut stderr_chunk = [0u8; 4 * 1024];

    enum Event {
        Stdout(io::Result<usize>),
        Stderr(io::Result<usize>),
        Exited(io::Result<ExitStatus>),
    }

    while status.is_none() || !stdout_done || !stderr_done {
        let event = tokio::select! {
            read = stdout.read(&mut stdout_chunk), if !stdout_done => Event::Stdout(read),
            read = stderr.read(&mut stderr_chunk), if !stderr_done => Event::Stderr(read),
            exited = child.wait(), if status.is_none() => Event::Exited(exited),
        };

        let exceeded = match event {
            Event::Stdout(Ok(0)) => {
                stdout_done = true;
                false
            }
            Event::Stdout(Ok(read)) => {
                append_capped(&mut stdout_bytes, &stdout_chunk[..read], stdout_limit)
            }
            Event::Stdout(Err(error)) => return Err(error),
            Event::Stderr(Ok(0)) => {
                stderr_done = true;
                false
            }
            Event::Stderr(Ok(read)) => append_capped(
                &mut stderr_bytes,
                &stderr_chunk[..read],
                MAX_GREP_STDERR_BYTES,
            ),
            Event::Stderr(Err(error)) => return Err(error),
            Event::Exited(result) => {
                status = Some(result?);
                false
            }
        };

        if exceeded && !truncated {
            truncated = true;
            child.start_kill()?;
        }
    }

    Ok(BoundedCommandOutput {
        status: status.ok_or_else(|| io::Error::other("grep 子进程未返回退出状态"))?,
        stdout: stdout_bytes,
        stderr: stderr_bytes,
        truncated,
    })
}

/// 返回本次追加是否超过上限。目标缓冲区永远不会增长到 limit 之外。
fn append_capped(target: &mut Vec<u8>, chunk: &[u8], limit: usize) -> bool {
    let remaining = limit.saturating_sub(target.len());
    target.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
    chunk.len() > remaining
}

fn search_status_is_success(output: &BoundedCommandOutput) -> bool {
    output.truncated || matches!(output.status.code(), Some(0 | 1))
}

fn mark_backend_truncated(rendered: &mut GrepRendered, backend: &str, stdout_limit: usize) {
    rendered.truncated = true;
    let note = format!(
        "...（{backend} 原始输出达到 {stdout_limit} bytes 上限，结果为部分数据；请缩小路径或模式）"
    );
    if rendered.display.is_empty() {
        rendered.display = note;
    } else {
        rendered.display.push('\n');
        rendered.display.push_str(&note);
    }
}

async fn run_grep_query(
    pattern: &str,
    path: &str,
    options: &GrepOptions,
    context: &ToolContext,
    stdout_limit: usize,
) -> SearchOutcome {
    // 路径解析、存在性检查、workspace canonicalize 与 rg 可执行文件探测都是
    // 同步文件系统 I/O，统一移入 spawn_blocking，避免阻塞 Tokio 工作线程。
    let context = context.clone();
    let path_owned = path.to_string();
    let prep = match task::spawn_blocking(move || prepare_grep_search(&path_owned, &context)).await
    {
        Ok(result) => result,
        Err(error) => {
            return SearchOutcome::error(
                "grep",
                path,
                pattern,
                format!("错误：grep 搜索准备任务失败：{error}"),
            )
        }
    };
    let GrepSearchPrep {
        workspace,
        target,
        rg_tool,
    } = match prep {
        Ok(prep) => prep,
        Err(message) => return SearchOutcome::error("grep", path, pattern, message),
    };

    let mut command = Command::new(rg_tool);
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

    // `-e` 隔离 pattern 与选项：以 `-` 开头的 pattern（如搜索字面量 `--force`）
    // 否则会被解析为命令行选项；`--` 同样保护以 `-` 开头的 target 路径。
    command.arg("-e").arg(pattern).arg("--").arg(&target);

    let output = match run_bounded_search_command(command, stdout_limit).await {
        Ok(output) => output,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return run_grep_fallback(pattern, &workspace, &target, options, stdout_limit).await;
        }
        Err(error) => {
            return SearchOutcome::error(
                "grep",
                path,
                pattern,
                format!("错误：执行 grep 搜索失败：{error}"),
            )
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !search_status_is_success(&output) {
        if stderr.is_empty() {
            return SearchOutcome::error(
                "grep",
                path,
                pattern,
                format!("错误：grep 搜索失败，退出状态：{}", output.status),
            );
        }
        return SearchOutcome::error(
            "grep",
            path,
            pattern,
            format!(
                "错误：grep 搜索失败：{stderr}\n\n[退出状态：{}]",
                output.status
            ),
        );
    }

    let mut rendered = render_grep_stdout(&stdout, options.max_files, options.files_with_matches);
    if output.truncated {
        mark_backend_truncated(&mut rendered, "ripgrep", stdout_limit);
    }
    if rendered.display.is_empty() {
        let display = format!("未找到匹配内容：{pattern}");
        return SearchOutcome {
            display,
            data: json!({
                "kind": "grep",
                "path": path,
                "pattern": pattern,
                "files": [],
                "totalMatches": 0,
                "truncated": output.truncated,
                "backend": "ripgrep",
            }),
        };
    }
    SearchOutcome {
        display: rendered.display,
        data: json!({
            "kind": "grep",
            "path": path,
            "pattern": pattern,
            "files": rendered.files,
            "totalMatches": rendered.total_matches,
            "truncated": rendered.truncated,
            "backend": "ripgrep",
        }),
    }
}

async fn run_grep_fallback(
    pattern: &str,
    workspace: &std::path::Path,
    target: &str,
    options: &GrepOptions,
    stdout_limit: usize,
) -> SearchOutcome {
    // 同主路径：可执行文件探测是同步文件系统 I/O，移入 spawn_blocking。
    let grep_tool = match task::spawn_blocking(|| resolve_tool("grep")).await {
        Ok(tool) => tool,
        Err(error) => {
            return SearchOutcome::error(
                "grep",
                target,
                pattern,
                format!("错误：grep 回退工具探测任务失败：{error}"),
            )
        }
    };
    let mut command = Command::new(grep_tool);
    // grep 只在 TTY 上着色，管道输出无需禁色参数；--no-color 是 ripgrep 专属，BSD/GNU grep 会报退出状态 2。
    // -I 跳二进制、-m 限单文件命中数；以 workspace 为 cwd + 相对 target，输出路径与 rg 主路径一致（相对、省 token）。
    command
        .arg("-rn")
        .arg("-H")
        .arg("-I")
        .arg("-m")
        .arg(options.max_matches_per_file.to_string())
        .current_dir(workspace)
        .kill_on_drop(true);

    match options.case_sensitive {
        Some(true) => {}
        Some(false) => {
            command.arg("-i");
        }
        // grep 没有 smart-case：pattern 不含大写字母时才 -i，近似 rg 行为。
        None => {
            if !pattern.chars().any(|c| c.is_uppercase()) {
                command.arg("-i");
            }
        }
    }
    if options.match_mode == "fixed" {
        command.arg("-F");
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
    // 系统 grep 不认 .gitignore、默认遍历隐藏文件与构建产物目录，这里显式排除对齐 rg 默认行为。
    if !options.include_hidden {
        command.arg("--exclude=.*").arg("--exclude-dir=.*");
    }
    if !options.no_ignore {
        for dir in IGNORED_DIRS {
            command.arg(format!("--exclude-dir={dir}"));
        }
    }
    // include/exclude 降级：grep 只支持 basename 级 glob。
    // 含 `/` 的 include 降级成 basename 会放宽为全工作区递归匹配，宽松
    // include 下命中量放大、可能提前触发 max_files 截断，反而掩盖真正目标，
    // 直接跳过并在结果中注明已放宽。含 `/` 的 exclude 同样无法表达：
    // --exclude-dir 只匹配目录 basename，`src/generated/**` 转成
    // `--exclude-dir=src/generated` 永远不命中、形同虚设——与其静默失效，
    // 不如注明放宽。裸名 exclude 在 rg 中匹配任意深度的同名文件与目录，
    // grep 需 `--exclude` + `--exclude-dir` 并用（只用 --exclude 不跳过同名
    // 目录，结果会静默偏宽）。
    // （方向上 include 宁缺毋滥：漏掉过滤好过引入掩盖目标的噪声。）
    let mut relaxed: Vec<String> = Vec::new();
    for glob in &options.include {
        if glob.contains('/') {
            relaxed.push(format!("include `{glob}`"));
            continue;
        }
        if let Some(base) = basename_glob(glob) {
            command.arg(format!("--include={base}"));
        }
    }
    for glob in &options.exclude {
        match grep_fallback_exclude(glob) {
            Ok(flags) => {
                command.args(flags);
            }
            Err(note) => relaxed.push(note),
        }
    }
    let relaxed_note = (!relaxed.is_empty()).then(|| {
        format!(
            "[注意] 本机缺少 ripgrep，已回退到系统 grep：其不支持路径 glob，以下过滤条件未生效（结果可能偏宽）：{}\n",
            relaxed.join("、")
        )
    });

    // 与 rg 主路径同理：`-e` 保护以 `-` 开头的 pattern，`--` 保护 target。
    command.arg("-e").arg(pattern).arg("--").arg(target);

    let output = match run_bounded_search_command(command, stdout_limit).await {
        Ok(output) => output,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return SearchOutcome::error(
                "grep",
                target,
                pattern,
                "错误：未找到 ripgrep (`rg`) 或系统 grep，无法执行搜索".to_string(),
            );
        }
        Err(error) => {
            return SearchOutcome::error(
                "grep",
                target,
                pattern,
                format!("错误：grep 回退搜索失败：{error}"),
            )
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !search_status_is_success(&output) {
        if stderr.is_empty() {
            return SearchOutcome::error(
                "grep",
                target,
                pattern,
                format!("错误：grep 回退搜索失败，退出状态：{}", output.status),
            );
        }
        return SearchOutcome::error(
            "grep",
            target,
            pattern,
            format!(
                "错误：grep 回退搜索失败：{stderr}\n\n[退出状态：{}]",
                output.status
            ),
        );
    }
    if stdout.trim().is_empty() {
        let backend_note = output.truncated.then(|| {
            format!(
                "...（grep 原始输出达到 {stdout_limit} bytes 上限，未得到完整记录；请缩小路径或模式）"
            )
        });
        let display = match backend_note {
            Some(note) => format!("{}{note}", relaxed_note.clone().unwrap_or_default()),
            None => format!(
                "{}未找到匹配内容：{pattern}",
                relaxed_note.clone().unwrap_or_default()
            ),
        };
        return SearchOutcome {
            display,
            data: json!({
                "kind": "grep",
                "path": target,
                "pattern": pattern,
                "files": [],
                "totalMatches": 0,
                "truncated": output.truncated,
                "backend": "grep",
                "relaxedFilters": relaxed,
            }),
        };
    }
    // 渲染含逐候选路径的同步 stat（split_grep_line），移入 spawn_blocking
    // 避免在 async worker 线程上做文件系统 I/O（与 glob 工具的处理一致）。
    let workspace_owned = workspace.to_path_buf();
    let max_files = options.max_files;
    let files_with_matches = options.files_with_matches;
    let backend_truncated = output.truncated;
    match task::spawn_blocking(move || {
        render_grep_fallback_output(&stdout, &workspace_owned, max_files, files_with_matches)
    })
    .await
    {
        Ok(mut rendered) => {
            if backend_truncated {
                mark_backend_truncated(&mut rendered, "grep", stdout_limit);
            }
            let display = match relaxed_note {
                Some(note) => format!("{note}{}", rendered.display),
                None => rendered.display,
            };
            SearchOutcome {
                display,
                data: json!({
                    "kind": "grep",
                    "path": target,
                    "pattern": pattern,
                    "files": rendered.files,
                    "totalMatches": rendered.total_matches,
                    "truncated": rendered.truncated,
                    "backend": "grep",
                    "relaxedFilters": relaxed,
                }),
            }
        }
        Err(error) => SearchOutcome::error(
            "grep",
            target,
            pattern,
            format!("错误：grep 回退结果渲染失败：{error}"),
        ),
    }
}

/// basename 级 glob 的校验（仅对不含 `/` 的模式调用）：过滤掉 `*`/`**`
/// 等等价于「不过滤」的模式。含 `/` 的路径 glob 无法安全降级为 basename
/// 匹配（语义偏差见调用方注释），不会进入本函数。
fn basename_glob(glob: &str) -> Option<String> {
    // rsplit 至少产出一项，此处不存在失败分支。
    let base = glob
        .rsplit('/')
        .next()
        .expect("rsplit 至少产出一项")
        .trim_end_matches('/');
    if base.is_empty() || base == "*" || base == "**" {
        return None;
    }
    Some(base.to_string())
}

/// `dir/**`、`dir/*` 形式的排除模式转成 --exclude-dir=dir（grep 无路径 glob 能力）。
fn dir_glob(glob: &str) -> Option<String> {
    let trimmed = glob.trim_end_matches('/');
    let dir = trimmed
        .strip_suffix("/**")
        .or_else(|| trimmed.strip_suffix("/*"))?
        .trim_end_matches('/');
    if dir.is_empty() || dir.contains('*') {
        return None;
    }
    Some(dir.to_string())
}

/// grep 回退路径上降级单个 exclude 模式。Ok 为应追加给 grep 的参数；
/// Err 为无法安全降级的模式说明（调用方汇入放宽提示），避免静默偏差。
/// - 裸名模式（`target`、`*.log`、尾斜杠的 `target/`）：rg（gitignore 语义）
///   匹配任意深度的同名文件或目录，grep 需 `--exclude-dir`（目录）与
///   `--exclude`（文件，尾斜杠按 gitignore 语义仅限目录则省略）并用；
/// - `name/**` / `name/*`：以 `--exclude-dir=name` 近似。注意 rg 的 `name/*`
///   与 `name/**` 均剪枝整棵目录树（gitignore 目录剪枝语义），两者等价；
///   且 rg 含 `/` 的 glob 锚定搜索根（仅排除顶层 name 目录），grep 则排除
///   任意深度同名目录，回退结果略窄——对 target/dist 等常见构建产物目录是
///   可接受的近似；
/// - 其余含 `/` 的路径模式：`--exclude-dir` 只匹配目录 basename，
///   `src/generated` 之类永远不命中，与其静默失效不如注明放宽。
fn grep_fallback_exclude(glob: &str) -> Result<Vec<String>, String> {
    if let Some(dir) = dir_glob(glob) {
        return if dir.contains('/') {
            Err(format!("exclude `{glob}`"))
        } else {
            Ok(vec![format!("--exclude-dir={dir}")])
        };
    }
    let trimmed = glob.trim_end_matches('/');
    if trimmed.contains('/') {
        return Err(format!("exclude `{glob}`"));
    }
    let Some(base) = basename_glob(trimmed) else {
        // `*`/`**` 等等价于「不排除」：无需参数，也无需放宽提示
        return Ok(Vec::new());
    };
    let mut flags = vec![format!("--exclude-dir={base}")];
    if trimmed.len() == glob.len() {
        flags.push(format!("--exclude={base}"));
    }
    Ok(flags)
}

/// 解析 `path:行: 文本`（命中）与 `path-行- 文本`（上下文）。
/// 两种分隔符都可能出现在路径或文本内部（如 `logs-2024-01/a.rs` 的目录名、
/// 上下文行文本里的 `:5:` 字面量），仅取首个出现位置会误切。grep 以
/// workspace 为 cwd、输出相对路径，因此用「路径段对应真实存在的文件」裁决：
/// 枚举全部 `:行:` 与 `-行-` 候选切分，收集路径可证实的候选——
/// - 恰有一个：直接采用；
/// - 多个同时可证实（路径自身含 `:数字:`/`-数字-`，且某个较短前缀也恰好
///   是文件，如 `a:12:b.rs` 与文件 `a` 并存）：按字节无法区分，取最长路径
///   候选——把更多字符当路径、更少当文本，最接近 grep 的原始输出结构；
/// - 零个：非 grep 格式行。
/// 行号解析失败（如溢出 u64）按该候选无效处理，继续尝试后续位置。
/// `path_cache` 缓存已证实/已否定的路径：大输出中同一文件路径反复出现，
/// 无缓存时 stat 次数约为 行数×候选位置数，缓存后收敛为唯一候选路径数。
fn split_grep_line<'a>(
    line: &'a str,
    workspace: &std::path::Path,
    path_cache: &mut HashMap<&'a str, bool>,
) -> Option<(&'a str, u64, bool, &'a str)> {
    let bytes = line.as_bytes();
    let mut confirmed: Vec<(&'a str, u64, bool, &'a str)> = Vec::new();
    for sep in [b':', b'-'] {
        for i in 1..bytes.len() {
            if bytes[i] != sep {
                continue;
            }
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j == start || j >= bytes.len() || bytes[j] != sep {
                continue;
            }
            let Some(line_number) = line[start..j].parse::<u64>().ok() else {
                continue;
            };
            let path = &line[..i];
            let is_file = *path_cache
                .entry(path)
                .or_insert_with(|| workspace.join(path).is_file());
            if is_file {
                confirmed.push((path, line_number, sep == b':', &line[j + 1..]));
            }
        }
    }
    match confirmed.len() {
        0 => None,
        1 => confirmed.pop(),
        _ => confirmed
            .into_iter()
            .max_by_key(|(path, _, _, _)| path.len()),
    }
}

fn render_grep_fallback_output(
    stdout: &str,
    workspace: &std::path::Path,
    max_files: usize,
    files_with_matches: bool,
) -> GrepRendered {
    let mut file_results: Vec<GrepFileResult> = Vec::new();
    let mut file_index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut path_cache: HashMap<&str, bool> = HashMap::new();
    let mut total_matches = 0usize;
    let mut truncated_by_file_limit = false;

    for line in stdout.lines() {
        let Some((path, line_number, is_match, text)) =
            split_grep_line(line, workspace, &mut path_cache)
        else {
            continue;
        };
        if is_noise(std::ffi::OsStr::new(
            std::path::Path::new(path).file_name().unwrap_or_default(),
        )) {
            continue;
        }

        let entry = match file_index.get(path) {
            Some(&idx) => Some(idx),
            None if file_results.len() < max_files => {
                let idx = file_results.len();
                file_results.push(GrepFileResult {
                    path: path.to_string(),
                    match_count: 0,
                    lines: Vec::new(),
                });
                file_index.insert(path.to_string(), idx);
                Some(idx)
            }
            None => {
                // 超过 max_files 的命中文件被丢弃：与 rg 主路径一致，标记截断，
                // 避免静默丢弃并在统计中误导模型已覆盖全部结果。
                truncated_by_file_limit = true;
                None
            }
        };
        let Some(idx) = entry else {
            continue;
        };
        if is_match {
            total_matches += 1;
            file_results[idx].match_count += 1;
        }
        if files_with_matches {
            continue;
        }
        file_results[idx].lines.push(GrepLine {
            line_number,
            text: text.to_string(),
            is_match,
        });
    }

    let matched_files = file_results
        .iter()
        .filter(|file| file.match_count > 0)
        .collect::<Vec<_>>();
    if matched_files.is_empty() {
        return GrepRendered {
            display: String::new(),
            files: Vec::new(),
            total_matches: 0,
            truncated: false,
        };
    }

    let mut lines = vec![format!(
        "共 {} 个文件 / {} 处匹配 (grep 回退)",
        matched_files.len(),
        total_matches
    )];
    if truncated_by_file_limit {
        lines.push(format!(
            "...（只展示前 {} 个命中文件，匹配数为部分统计，请继续缩小范围）",
            max_files
        ));
    }

    if files_with_matches {
        lines.extend(matched_files.iter().map(|file| file.path.clone()));
        return GrepRendered {
            display: lines.join("\n"),
            files: matched_files
                .into_iter()
                .map(|file| {
                    json!({
                        "path": file.path,
                        "matchCount": file.match_count,
                        "lines": [],
                    })
                })
                .collect(),
            total_matches,
            truncated: truncated_by_file_limit,
        };
    }

    for file in &matched_files {
        lines.push(String::new());
        lines.push(file.path.clone());
        for entry in &file.lines {
            let separator = if entry.is_match { ':' } else { '-' };
            lines.push(format!("{}{} {}", entry.line_number, separator, entry.text));
        }
    }
    let files = matched_files
        .into_iter()
        .map(|file| {
            json!({
                "path": file.path,
                "matchCount": file.match_count,
                "lines": file.lines.iter().map(|line| json!({
                    "lineNumber": line.line_number,
                    "text": line.text,
                    "isMatch": line.is_match,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    GrepRendered {
        display: lines.join("\n"),
        files,
        total_matches,
        truncated: truncated_by_file_limit,
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

/// grep 主路径的同步准备结果（在 spawn_blocking 中计算）。
struct GrepSearchPrep {
    workspace: std::path::PathBuf,
    target: String,
    rg_tool: std::path::PathBuf,
}

fn prepare_grep_search(path: &str, context: &ToolContext) -> Result<GrepSearchPrep, String> {
    let search_path = resolve_path(context, path)?;
    if !search_path.exists() {
        return Err(format!("错误：搜索路径不存在：{path}"));
    }
    let workspace = context
        .workspace
        .canonicalize()
        .map_err(|error| format!("错误：解析工作区路径失败：{error}"))?;
    let target = workspace_relative_target(&workspace, &search_path);
    Ok(GrepSearchPrep {
        workspace,
        target,
        rg_tool: resolve_tool("rg"),
    })
}

fn render_grep_stdout(stdout: &str, max_files: usize, files_with_matches: bool) -> GrepRendered {
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
        return GrepRendered {
            display: String::new(),
            files: Vec::new(),
            total_matches: 0,
            truncated: false,
        };
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
        return GrepRendered {
            display: lines.join("\n"),
            files: matched_files
                .into_iter()
                .map(|file| {
                    json!({
                        "path": file.path,
                        "matchCount": file.match_count,
                        "lines": [],
                    })
                })
                .collect(),
            total_matches,
            truncated: truncated_by_file_limit,
        };
    }

    for file in &matched_files {
        lines.push(String::new());
        lines.push(file.path.clone());
        for entry in &file.lines {
            let separator = if entry.is_match { ':' } else { '-' };
            lines.push(format!("{}{} {}", entry.line_number, separator, entry.text));
        }
    }

    let files = matched_files
        .into_iter()
        .map(|file| {
            json!({
                "path": file.path,
                "matchCount": file.match_count,
                "lines": file.lines.iter().map(|line| json!({
                    "lineNumber": line.line_number,
                    "text": line.text,
                    "isMatch": line.is_match,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    GrepRendered {
        display: lines.join("\n"),
        files,
        total_matches,
        truncated: truncated_by_file_limit,
    }
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
    use super::*;

    const BOUNDED_TEST_STDOUT_BYTES: usize = 64 * 1024;

    /// 在临时目录中创建相对路径文件，返回目录（测试结束由调用方清理）。
    fn make_workspace(name: &str, files: &[&str]) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("jk-search-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        for file in files {
            let path = dir.join(file);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(path, "x").unwrap();
        }
        dir
    }

    #[test]
    fn split_grep_line_parses_match_and_context_lines() {
        let workspace = make_workspace("basic", &["src/a.rs"]);
        let mut cache = HashMap::new();

        let parsed = split_grep_line("src/a.rs:12:命中内容", &workspace, &mut cache).unwrap();
        assert_eq!(parsed, ("src/a.rs", 12, true, "命中内容"));

        let parsed = split_grep_line("src/a.rs-11-上下文", &workspace, &mut cache).unwrap();
        assert_eq!(parsed, ("src/a.rs", 11, false, "上下文"));

        // 路径不可证实的行返回 None。
        assert!(split_grep_line("ghost.rs:1:text", &workspace, &mut cache).is_none());

        let _ = fs::remove_dir_all(&workspace);
    }

    #[test]
    fn split_grep_line_prefers_longest_path_when_prefix_is_also_a_file() {
        // 文件名自身含 `:数字:`，且短前缀 `a` 恰好也是文件：
        // 旧实现按首个可证实候选切分会误判为 `a:12`，正文错位成 `b.rs:5:foo`。
        let workspace = make_workspace("colon-path", &["a", "a:12:b.rs"]);
        let mut cache = HashMap::new();

        let parsed = split_grep_line("a:12:b.rs:5:foo", &workspace, &mut cache).unwrap();
        assert_eq!(parsed, ("a:12:b.rs", 5, true, "foo"));

        // 缓存不得固化错误候选：同一行的解析结果保持一致。
        let parsed = split_grep_line("a:12:b.rs:9:bar", &workspace, &mut cache).unwrap();
        assert_eq!(parsed, ("a:12:b.rs", 9, true, "bar"));

        let _ = fs::remove_dir_all(&workspace);
    }

    #[test]
    fn split_grep_line_handles_dashed_dir_names() {
        // 目录名含 `-数字-`：命中行的 `:行:` 候选应先于 `-行-` 候选被证实。
        let workspace = make_workspace("dash-dir", &["logs-2024-1/app.rs"]);
        let mut cache = HashMap::new();

        let parsed = split_grep_line("logs-2024-1/app.rs:7:hit", &workspace, &mut cache).unwrap();
        assert_eq!(parsed, ("logs-2024-1/app.rs", 7, true, "hit"));

        let parsed = split_grep_line("logs-2024-1/app.rs-6-ctx", &workspace, &mut cache).unwrap();
        assert_eq!(parsed, ("logs-2024-1/app.rs", 6, false, "ctx"));

        let _ = fs::remove_dir_all(&workspace);
    }

    #[test]
    fn grep_fallback_exclude_bare_name_covers_files_and_dirs() {
        // rg 的 !target 匹配任意深度的同名文件与目录；grep 只用 --exclude
        // 不跳过同名目录（结果静默偏宽），必须两个参数并用。
        assert_eq!(
            grep_fallback_exclude("target").unwrap(),
            ["--exclude-dir=target", "--exclude=target"]
        );
        assert_eq!(
            grep_fallback_exclude("*.log").unwrap(),
            ["--exclude-dir=*.log", "--exclude=*.log"]
        );
        // gitignore 尾斜杠语义：仅目录
        assert_eq!(
            grep_fallback_exclude("target/").unwrap(),
            ["--exclude-dir=target"]
        );
    }

    #[test]
    fn grep_fallback_exclude_bare_dir_tree_maps_to_exclude_dir() {
        // rg 的 `!name/*` 与 `!name/**` 同义（gitignore 目录剪枝，均排除整棵树），
        // 以 --exclude-dir=name 近似（锚定差异见函数注释）。
        assert_eq!(
            grep_fallback_exclude("dist/**").unwrap(),
            ["--exclude-dir=dist"]
        );
        assert_eq!(
            grep_fallback_exclude("dist/*").unwrap(),
            ["--exclude-dir=dist"]
        );
    }

    #[test]
    fn grep_fallback_exclude_path_patterns_are_relaxed_not_silent() {
        // --exclude-dir 只匹配目录 basename，路径形排除无法表达：
        // 必须注明放宽，而非静默失效（旧实现会产出永不命中的
        // `--exclude-dir=src/generated`）。
        assert!(grep_fallback_exclude("src/generated/**").is_err());
        assert!(grep_fallback_exclude("src/**/*.rs").is_err());
        assert!(grep_fallback_exclude("src/generated").is_err());
        // `*`/`**` 等价于不排除：无参数、无提示
        assert!(grep_fallback_exclude("*").unwrap().is_empty());
        assert!(grep_fallback_exclude("**").unwrap().is_empty());
    }

    #[test]
    fn path_within_allowed_roots_accepts_inside_and_rejects_escape() {
        let workspace_raw = make_workspace("glob-contain", &["src/a.rs"]);
        // canonicalize：macOS 临时目录 /var/... 是 /private/var/... 的符号链接，
        // 与生产路径一致（restrict 开启时 resolve_path 返回 canonical 目录）。
        let workspace = workspace_raw.canonicalize().unwrap();
        let inside = workspace.join("src/**/*.rs");
        assert!(path_within_allowed_roots(&inside, &[workspace.clone()]));

        // `..` 归一化后越出工作区
        let escaped = lexical_normalize(&workspace.join("../../etc/*"));
        assert!(!path_within_allowed_roots(&escaped, &[workspace.clone()]));

        // 绝对路径模式不位于工作区内
        assert!(!path_within_allowed_roots(
            Path::new("/etc/*"),
            &[workspace.clone()]
        ));

        let _ = fs::remove_dir_all(&workspace_raw);
    }

    #[cfg(unix)]
    #[test]
    fn safe_glob_entries_never_follows_or_returns_symlinks() {
        use std::os::unix::fs::symlink;

        let workspace = make_workspace("glob-symlink", &["local.rs"]);
        let outside = make_workspace("glob-outside", &["secret.rs"]);
        symlink(&outside, workspace.join("outside-link")).unwrap();

        let entries = safe_glob_entries(&workspace, None)
            .map(|entry| {
                entry
                    .unwrap()
                    .path()
                    .strip_prefix(&workspace)
                    .unwrap()
                    .to_path_buf()
            })
            .collect::<Vec<_>>();
        assert_eq!(entries, [std::path::PathBuf::from("local.rs")]);

        let _ = fs::remove_dir_all(&workspace);
        let _ = fs::remove_dir_all(&outside);
    }

    #[test]
    fn render_grep_fallback_output_marks_file_limit_truncation() {
        let workspace = make_workspace("fallback-trunc", &["a.rs", "b.rs", "c.rs"]);
        let stdout = "a.rs:1:match\nb.rs:1:match\nc.rs:1:match\n";

        let rendered = render_grep_fallback_output(stdout, &workspace, 1, false);
        assert!(rendered.display.contains("a.rs"), "应保留首个命中文件");
        assert!(
            rendered.display.contains("只展示前 1 个命中文件"),
            "超过 max_files 时必须给出截断提示：{}",
            rendered.display
        );
        assert!(
            rendered.display.contains("部分统计"),
            "截断时必须注明匹配数为部分统计：{}",
            rendered.display
        );
        assert!(rendered.truncated);
        assert_eq!(rendered.files.len(), 1);
        assert!(
            !rendered.display.contains("b.rs:1"),
            "超限文件的匹配行应被丢弃"
        );

        let _ = fs::remove_dir_all(&workspace);
    }

    #[test]
    fn append_capped_never_grows_past_limit() {
        let mut output = vec![1, 2];
        assert!(append_capped(&mut output, &[3, 4, 5], 4));
        assert_eq!(output, [1, 2, 3, 4]);
        assert!(append_capped(&mut output, &[6], 4));
        assert_eq!(output.len(), 4);
    }

    #[test]
    fn grep_queries_share_the_call_stdout_budget() {
        assert_eq!(grep_stdout_budget_per_query(1), MAX_GREP_STDOUT_BYTES);
        assert_eq!(grep_stdout_budget_per_query(2), MAX_GREP_STDOUT_BYTES);

        for query_count in 1..=16 {
            let per_query = grep_stdout_budget_per_query(query_count);
            assert!(per_query <= MAX_GREP_STDOUT_BYTES);
            assert!(per_query * query_count <= MAX_GREP_CALL_STDOUT_BYTES);
        }
        assert_eq!(grep_stdout_budget_per_query(16), 1024 * 1024);
    }

    /// 由下一个测试作为独立子进程精确调用；普通测试运行时不产生大输出。
    #[test]
    fn bounded_command_child_writes_past_limit() {
        if std::env::var_os("JK_SEARCH_BOUNDED_OUTPUT_CHILD").is_none() {
            return;
        }
        use std::io::Write as _;
        let chunk = vec![b'x'; BOUNDED_TEST_STDOUT_BYTES];
        let mut stdout = std::io::stdout().lock();
        for _ in 0..=4 {
            stdout.write_all(&chunk).unwrap();
        }
        stdout.flush().unwrap();
    }

    #[tokio::test]
    async fn bounded_command_kills_child_at_stdout_limit() {
        let executable = std::env::current_exe().unwrap();
        let mut command = Command::new(executable);
        command
            .arg("--exact")
            .arg("agent::tools::builtin::search::tests::bounded_command_child_writes_past_limit")
            .arg("--nocapture")
            .env("JK_SEARCH_BOUNDED_OUTPUT_CHILD", "1")
            .kill_on_drop(true);

        let output = run_bounded_search_command(command, BOUNDED_TEST_STDOUT_BYTES)
            .await
            .unwrap();
        assert!(output.truncated);
        assert_eq!(output.stdout.len(), BOUNDED_TEST_STDOUT_BYTES);
        assert!(search_status_is_success(&output));
    }
}
