use std::fs;
use std::io::{BufRead, BufReader};

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::{
    task,
    time::{timeout, Duration},
};

use super::super::common::{
    non_empty_string_array_arg, render_labeled_sections, resolve_path, usize_arg,
    with_compression_parameters,
};
use crate::agent::tools::context::ToolContext;
use crate::agent::tools::registry::AgentTool;

const FILE_IO_TIMEOUT_SECS: u64 = 30;

pub(super) fn read_file_tool() -> Box<dyn AgentTool> {
    Box::new(ReadFileTool)
}

struct ReadFileTool;

#[derive(Debug, Eq, PartialEq)]
struct FileLineSpec<'a> {
    path: &'a str,
    range: Option<(usize, usize)>,
}

#[async_trait]
impl AgentTool for ReadFileTool {
    fn name(&self) -> &'static str {
        "read_file"
    }

    fn description(&self) -> &'static str {
        "读取文本文件，输出格式为 行号|内容。paths 支持普通文件路径，也支持 path:start-end 协议精确读取包含边界的行范围，例如 backend/app/services/workspace_files.py:123-156。大文件可配合 offset 和 limit 分段读取。compress=false 绝不进行摘要，超过 2000 字符的内联结果会带定位标记截断。"
    }

    fn parameters(&self) -> Value {
        with_compression_parameters(
            json!({
                "type": "object",
                "properties": {
                    "paths": {
                        "type": "array",
                        "description": "要读取的文件路径列表。支持普通路径，也支持 path:start-end 包含边界的精确行范围，例如 backend/app/services/workspace_files.py:123-156。路径自带行范围时忽略 offset/limit。即使只读取一个文件，也必须传单元素数组。",
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

                // 单文件也保留路径标签，使后续摘要能产生可复查的 path:start-end 定位。
                render_labeled_sections(sections)
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

fn read_file_lines(path: &str, offset: usize, limit: usize, context: &ToolContext) -> String {
    let spec = match parse_file_line_spec(path) {
        Ok(parsed) => parsed,
        Err(message) => return message,
    };
    let file_path = match resolve_path(context, spec.path) {
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

    let file = match fs::File::open(&file_path) {
        Ok(file) => file,
        Err(error) => return format!("读取文件失败：{error}"),
    };

    let reader = BufReader::new(file);
    let (start_line, line_limit) = spec
        .range
        .map(|(start, end)| (start, end - start + 1))
        .unwrap_or((offset, limit));
    read_numbered_lines(reader, spec.path, start_line, line_limit, spec.range)
}

fn read_numbered_lines<R: BufRead>(
    reader: R,
    file_path_text: &str,
    start_line: usize,
    line_limit: usize,
    requested_range: Option<(usize, usize)>,
) -> String {
    let skip_lines = start_line.saturating_sub(1);
    let mut output = Vec::new();
    let mut last_line_number = 0;

    for (index, line_result) in reader.lines().enumerate() {
        let line_number = index + 1;
        last_line_number = line_number;
        let line = match line_result {
            Ok(line) => line,
            Err(error) => return format!("读取文件失败：{error}"),
        };
        if index < skip_lines {
            continue;
        }
        if output.len() >= line_limit {
            break;
        }
        output.push(format!("{line_number}|{line}"));
    }

    if let Some((start, end)) = requested_range {
        let expected_lines = end - start + 1;
        if output.len() != expected_lines {
            return format!(
                "错误：行号范围 {start}-{end} 超出文件范围，{file_path_text} 共 {last_line_number} 行"
            );
        }
    }
    output.join("\n")
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

    use super::{parse_file_line_spec, read_numbered_lines, FileLineSpec};

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
        let result = read_numbered_lines(
            Cursor::new("one\ntwo\nthree\nfour\nfive\n"),
            "src/app.rs",
            2,
            3,
            Some((2, 4)),
        );

        assert_eq!(result, "2|two\n3|three\n4|four");
    }

    #[test]
    fn rejects_a_line_range_beyond_the_end_of_the_file() {
        let result = read_numbered_lines(
            Cursor::new("one\ntwo\nthree\n"),
            "src/app.rs",
            2,
            3,
            Some((2, 4)),
        );

        assert_eq!(
            result,
            "错误：行号范围 2-4 超出文件范围，src/app.rs 共 3 行"
        );
    }
}
