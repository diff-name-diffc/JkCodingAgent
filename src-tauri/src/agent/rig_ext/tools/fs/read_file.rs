//! read_file 工具（rig 形态）。迁移自旧自实现工具层（已随迁移删除）。
//! 语义不变：2MB 单文件硬上限 / 4MB 调用级共享预算 / `path:start-end` 行区间
//! 协议 / offset+limit 分页；全路径失败返回「错误：」可恢复错误。
//! 偏差：旧的结构化 `data` 载荷（files/resolvedPath/lines）不再产出——
//! rig runtime 只持久化原始文本，模型侧展示文本保持一致。

use std::fs;
use std::io::{BufRead, BufReader, Read};

use rig::tool::{PortableDynamicTool, ToolExecutionError, ToolOutput};
use serde_json::{json, Value};
use tokio::{
    task,
    time::{timeout, Duration},
};

use super::super::common::{
    non_empty_string_array_arg, render_labeled_sections, usize_arg, with_compression_parameters,
    DEFAULT_FORCE_COMPRESS_AFTER_CHARS,
};
use super::FsSandbox;

const FILE_IO_TIMEOUT_SECS: u64 = 30;

/// 单文件读取字节硬上限（2MB）。先打开文件、再基于同一文件句柄复检大小，
/// 并用 take() 截断读取——检查与读取基于同一 inode，消除 metadata 与 open
/// 之间文件被替换/增长的 TOCTOU 窗口；即使文件并发增长也不会超限读取，
/// 保证阻塞任务有界，而非仅依赖外层 timeout 兜底。
const MAX_READ_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// 单次 read_file 调用的总读取预算。多路径共享该预算，避免 paths 批量参数
/// 把单文件上限线性放大为 8 倍；预算在启动阻塞读取前完成切分。
const MAX_READ_CALL_BYTES: u64 = 4 * 1024 * 1024;

const DESCRIPTION: &str = "读取文本文件，输出格式为 行号|内容。paths 支持普通文件路径，也支持 path:start-end 协议精确读取包含边界的行范围，例如 backend/app/services/workspace_files.py:123-156；行范围超出文件实际行数时不报错，自动返回到文件末尾的可用内容。大文件可配合 offset 和 limit 分段读取。compress=false 绝不进行摘要；内联结果默认 10000 字符截断，显式传 offset 或 limit 分页读取时上限提高到 20000，截断均带定位标记。";

pub(super) fn read_file_tool(sandbox: FsSandbox) -> PortableDynamicTool {
    PortableDynamicTool::new("read_file", DESCRIPTION, parameters(), move |args| {
        let sandbox = sandbox.clone();
        Box::pin(async move { execute(args, sandbox).await })
    })
}

fn parameters() -> Value {
    with_compression_parameters(
        json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "paths": {
                    "type": "array",
                    "description": "要读取的文件路径列表。支持普通路径，也支持 path:start-end 包含边界的精确行范围，例如 backend/app/services/workspace_files.py:123-156。路径自带行范围时忽略 offset/limit。即使只读取一个文件，也必须传单元素数组。",
                    "minItems": 1,
                    "maxItems": 8,
                    "items": { "type": "string", "minLength": 1, "maxLength": 4096 }
                },
                "offset": { "type": "integer", "description": "起始行号，从 1 开始，默认 1" , "minimum": 1, "maximum": 10000000 },
                "limit": { "type": "integer", "description": "最多读取多少行，默认 2000", "minimum": 1, "maximum": 10000 }
            },
            "required": ["paths"]
        }),
        false,
        DEFAULT_FORCE_COMPRESS_AFTER_CHARS,
        "分析代码、配置或精确文本时保持关闭保留完整结果；只定位关键符号或需要概览时可开启并写明 compress_intent。",
    )
}

async fn execute(args: Value, sandbox: FsSandbox) -> Result<ToolOutput, ToolExecutionError> {
    let Some(paths) = non_empty_string_array_arg(&args, "paths") else {
        return Err(ToolExecutionError::other(
            "错误：缺少必填参数 paths，且 paths 必须是非空字符串数组",
        ));
    };
    let offset = usize_arg(&args, "offset").unwrap_or(1).max(1);
    let limit = usize_arg(&args, "limit").unwrap_or(2000).max(1);
    let per_file_budget = per_file_read_budget(paths.len());

    match timeout(
        Duration::from_secs(FILE_IO_TIMEOUT_SECS),
        task::spawn_blocking(move || {
            let outcomes = paths
                .iter()
                .map(|path| {
                    (
                        path,
                        read_file_outcome(path, offset, limit, per_file_budget, &sandbox),
                    )
                })
                .collect::<Vec<_>>();

            // 单文件也保留路径标签，使后续摘要能产生可复查的 path:start-end 定位。
            let display = render_labeled_sections(
                outcomes
                    .iter()
                    .map(|(path, outcome)| {
                        (format!("read_file path={path}"), outcome.display.clone())
                    })
                    .collect(),
            );
            let all_failed =
                !outcomes.is_empty() && outcomes.iter().all(|(_, outcome)| outcome.is_error);
            (display, all_failed)
        }),
    )
    .await
    {
        Ok(Ok((display, true))) => Err(ToolExecutionError::other(display)),
        Ok(Ok((display, false))) => Ok(ToolOutput::text(display)),
        Ok(Err(error)) => Err(ToolExecutionError::other(format!(
            "错误：读取文件任务失败：{error}"
        ))),
        Err(_) => Err(ToolExecutionError::timeout(format!(
            "错误：读取文件超时（{FILE_IO_TIMEOUT_SECS} 秒）"
        ))),
    }
}

#[derive(Debug, Eq, PartialEq)]
struct FileLineSpec<'a> {
    path: &'a str,
    range: Option<(usize, usize)>,
}

struct FileReadOutcome {
    display: String,
    is_error: bool,
}

impl FileReadOutcome {
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

fn read_file_outcome(
    path: &str,
    offset: usize,
    limit: usize,
    max_bytes: u64,
    sandbox: &FsSandbox,
) -> FileReadOutcome {
    let spec = match parse_file_line_spec(path) {
        Ok(parsed) => parsed,
        Err(message) => return FileReadOutcome::error(message),
    };
    let file_path = match sandbox.resolve(spec.path) {
        Ok(path) => path,
        Err(message) => return FileReadOutcome::error(message),
    };
    if !file_path.exists() {
        return FileReadOutcome::error(format!("错误：文件不存在：{path}"));
    }
    if file_path.is_dir() {
        return FileReadOutcome::error(format!("错误：{path} 是目录，不是文件"));
    }

    // 先打开文件，再基于打开的句柄复检大小：检查与读取针对同一文件，
    // 消除 metadata 检查与 open 之间文件被并发替换/增长的 TOCTOU 窗口。
    let file = match fs::File::open(&file_path) {
        Ok(file) => file,
        Err(error) => return FileReadOutcome::error(format!("错误：读取文件失败：{error}")),
    };
    match file.metadata() {
        Ok(meta) if meta.len() > max_bytes => {
            return FileReadOutcome::error(format!(
                "错误：文件过大（{} bytes），超过本次调用为该路径分配的 {max_bytes} bytes 读取限制",
                meta.len()
            ));
        }
        _ => {}
    }

    // take() 对读取字节数做硬截断：即便文件在读取期间并发增长，也不会超过
    // 调用级共享配额，阻塞任务与最终聚合均保持有界。
    let reader = BufReader::new(file.take(max_bytes));
    let (start_line, line_limit) = spec
        .range
        .map(|(start, end)| (start, end - start + 1))
        .unwrap_or((offset, limit));
    match collect_numbered_lines(reader, start_line, line_limit) {
        Ok(lines) => FileReadOutcome::ok(
            lines
                .iter()
                .map(|(number, text)| format!("{number}|{text}"))
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        Err(message) => FileReadOutcome::error(message),
    }
}

fn per_file_read_budget(path_count: usize) -> u64 {
    let path_count = u64::try_from(path_count.max(1)).unwrap_or(u64::MAX);
    (MAX_READ_CALL_BYTES / path_count).min(MAX_READ_FILE_BYTES)
}

fn collect_numbered_lines<R: BufRead>(
    reader: R,
    start_line: usize,
    line_limit: usize,
) -> Result<Vec<(usize, String)>, String> {
    let skip_lines = start_line.saturating_sub(1);
    let mut output = Vec::new();

    for (index, line_result) in reader.lines().enumerate() {
        let line_number = index + 1;
        let line = match line_result {
            Ok(line) => line,
            Err(error) => return Err(format!("错误：读取文件失败：{error}")),
        };
        if index < skip_lines {
            continue;
        }
        if output.len() >= line_limit {
            break;
        }
        output.push((line_number, line));
    }

    // 行范围超出文件实际行数时不报错，返回实际可读到的最大内容。
    Ok(output)
}

#[cfg(test)]
fn read_numbered_lines<R: BufRead>(reader: R, start_line: usize, line_limit: usize) -> String {
    match collect_numbered_lines(reader, start_line, line_limit) {
        Ok(lines) => lines
            .into_iter()
            .map(|(number, text)| format!("{number}|{text}"))
            .collect::<Vec<_>>()
            .join("\n"),
        Err(message) => message,
    }
}

fn parse_file_line_spec(raw: &str) -> Result<FileLineSpec<'_>, String> {
    let Some((path, suffix)) = raw.rsplit_once(':') else {
        return Ok(FileLineSpec {
            path: raw,
            range: None,
        });
    };
    let Some((start_text, end_text)) = suffix.split_once('-') else {
        return Ok(FileLineSpec {
            path: raw,
            range: None,
        });
    };

    let invalid = || {
        format!(
            "错误：无效的 read_file 行号协议 `{raw}`，应为 path:start-end，例如 backend/app/services/workspace_files.py:123-156"
        )
    };
    if path.is_empty()
        || start_text.is_empty()
        || end_text.is_empty()
        || !start_text.chars().all(|ch| ch.is_ascii_digit())
        || !end_text.chars().all(|ch| ch.is_ascii_digit())
    {
        return Err(invalid());
    }

    let start = start_text.parse::<usize>().map_err(|_| invalid())?;
    let end = end_text.parse::<usize>().map_err(|_| invalid())?;
    if start == 0 || end < start {
        return Err(invalid());
    }

    Ok(FileLineSpec {
        path,
        range: Some((start, end)),
    })
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::{
        parse_file_line_spec, per_file_read_budget, read_numbered_lines, FileLineSpec,
        MAX_READ_CALL_BYTES, MAX_READ_FILE_BYTES,
    };

    #[test]
    fn parses_inclusive_file_line_range() {
        assert_eq!(
            parse_file_line_spec("backend/app/services/workspace_files.py:123-156"),
            Ok(FileLineSpec {
                path: "backend/app/services/workspace_files.py",
                range: Some((123, 156)),
            })
        );
    }

    #[test]
    fn preserves_plain_paths_and_windows_drive_prefixes() {
        assert_eq!(
            parse_file_line_spec("backend/app/services/workspace_files.py"),
            Ok(FileLineSpec {
                path: "backend/app/services/workspace_files.py",
                range: None,
            })
        );
        assert_eq!(
            parse_file_line_spec("C:/workspace/backend/app.py:8-12"),
            Ok(FileLineSpec {
                path: "C:/workspace/backend/app.py",
                range: Some((8, 12)),
            })
        );
    }

    #[test]
    fn rejects_invalid_file_line_ranges() {
        for invalid in ["src/app.rs:0-3", "src/app.rs:9-3", "src/app.rs:3-last"] {
            assert!(parse_file_line_spec(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn reads_the_complete_inclusive_line_range() {
        let result = read_numbered_lines(Cursor::new("one\ntwo\nthree\nfour\nfive\n"), 2, 3);

        assert_eq!(result, "2|two\n3|three\n4|four");
    }

    #[test]
    fn returns_available_content_when_the_line_range_overshoots_the_file() {
        let result = read_numbered_lines(Cursor::new("one\ntwo\nthree\n"), 2, 3);

        assert_eq!(result, "2|two\n3|three");
    }

    #[test]
    fn returns_empty_when_the_start_line_is_beyond_the_file() {
        let result = read_numbered_lines(Cursor::new("one\ntwo\nthree\n"), 10, 3);

        assert_eq!(result, "");
    }

    #[test]
    fn multiple_paths_share_the_call_read_budget() {
        assert_eq!(per_file_read_budget(1), MAX_READ_FILE_BYTES);
        assert_eq!(per_file_read_budget(2), MAX_READ_FILE_BYTES);

        for path_count in 1..=8 {
            let per_file = per_file_read_budget(path_count);
            assert!(per_file <= MAX_READ_FILE_BYTES);
            assert!(per_file * path_count as u64 <= MAX_READ_CALL_BYTES);
        }
        assert_eq!(per_file_read_budget(8), 512 * 1024);
    }
}
