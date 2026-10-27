use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::time::{Duration, Instant};

use base64::{engine::general_purpose::STANDARD, Engine};
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::watch;

use super::query::{
    FileMatchCount, LoadedGeneration, LogSearchRequest, LogSearchResult, MatchMode, OutputMode,
};
use super::search::{make_cursor, materialize_matches, validate_cursor, Hit};

const SCAN_TIMEOUT: Duration = Duration::from_secs(240);
const MATCH_OUTPUT_LIMIT: usize = 8 * 1024 * 1024;
const COUNT_OUTPUT_LIMIT: usize = 1024 * 1024;
const STDERR_LIMIT: usize = 64 * 1024;

pub(crate) async fn execute(
    generation: LoadedGeneration,
    request: LogSearchRequest,
    cancel: Option<watch::Receiver<bool>>,
) -> Result<LogSearchResult, String> {
    let cursor = validate_cursor(&request, &generation.manifest)?;
    let mut files = generation
        .manifest
        .files
        .iter()
        .filter(|file| request.file_ids.is_empty() || request.file_ids.contains(&file.file_id))
        .cloned()
        .collect::<Vec<_>>();
    files.sort_by_key(|file| file.file_id);
    if files.is_empty() {
        return Ok(LogSearchResult::empty(
            generation.manifest.snapshot_id.clone(),
            generation.manifest.generation,
            "ripgrep".into(),
            request.output_mode,
        ));
    }
    let paths = files
        .iter()
        .map(|file| generation.root.join(&file.raw_path))
        .collect::<Vec<_>>();
    let count_mode = request.output_mode != OutputMode::Matches;
    let mut command = Command::new(crate::agent::rig_ext::tools::fs::resolve_tool("rg"));
    command
        .arg("--no-config")
        .arg("--no-ignore")
        .arg("--hidden")
        .arg("--text")
        .arg("--color=never")
        .arg("--threads=1")
        .arg(if request.case_sensitive {
            "--case-sensitive"
        } else {
            "--ignore-case"
        });
    if count_mode {
        command.arg("--count").arg("--with-filename");
    } else {
        command.arg("--json");
        if cursor.is_none() {
            command
                .arg("--max-count")
                .arg((request.max_matches + 1).to_string());
        }
    }
    if request.match_mode == MatchMode::Fixed {
        command.arg("--fixed-strings");
    }
    for pattern in &request.patterns {
        command.arg("--regexp").arg(pattern);
    }
    command.arg("--").args(&paths);
    let output = run_rg(
        command,
        if count_mode {
            COUNT_OUTPUT_LIMIT
        } else {
            MATCH_OUTPUT_LIMIT
        },
        cancel,
    )
    .await?;
    if output.cancelled {
        return Err("错误：日志精确扫描已取消".into());
    }
    let code = output.status.as_ref().and_then(ExitStatus::code);
    if !output.capped && !output.timed_out && !matches!(code, Some(0 | 1)) {
        return Err(format!(
            "错误：ripgrep 检索失败（exit={code:?}）：{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let mut result = LogSearchResult::empty(
        generation.manifest.snapshot_id.clone(),
        generation.manifest.generation,
        "ripgrep".into(),
        request.output_mode,
    );
    result.warnings = output.warnings();
    if count_mode {
        let truncated_output = output.capped || output.timed_out;
        let counts = parse_counts(&output.stdout, &paths, &files, truncated_output)?;
        result.files = counts
            .iter()
            .filter_map(|(file_id, match_count)| {
                files
                    .iter()
                    .find(|file| file.file_id == *file_id)
                    .map(|file| FileMatchCount {
                        file_id: *file_id,
                        source_label: file.source_label.clone(),
                        match_count: *match_count,
                    })
            })
            .collect();
        if result.files.len() > request.max_files {
            result.files.truncate(request.max_files);
            result.results_truncated = true;
        }
        let complete = !output.capped && !output.timed_out && matches!(code, Some(0 | 1));
        result.scan_complete = complete;
        result.results_truncated |= !complete;
        if complete {
            result.total_match_lines = Some(counts.values().sum());
        }
        return Ok(result);
    }

    let path_ids = paths
        .iter()
        .zip(&files)
        .map(|(path, file)| (path.to_string_lossy().into_owned(), file.file_id))
        .collect::<BTreeMap<_, _>>();
    let mut hits = Vec::new();
    let mut has_extra = false;
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) != Some("match") {
            continue;
        }
        let Some(data) = value.get("data") else {
            continue;
        };
        let Some(path) = data.get("path").and_then(event_text) else {
            continue;
        };
        let Some(file_id) = path_ids.get(&path).copied() else {
            continue;
        };
        let Some(line_number) = data.get("line_number").and_then(Value::as_u64) else {
            continue;
        };
        if let Some(cursor) = &cursor {
            let cursor_file = cursor
                .file_id
                .ok_or_else(|| "扫描 cursor 缺少文件位置".to_string())?;
            let cursor_line = cursor
                .line_number
                .ok_or_else(|| "扫描 cursor 缺少行位置".to_string())?;
            if (file_id, line_number) <= (cursor_file, cursor_line) {
                continue;
            }
        }
        if hits.len() >= request.max_matches {
            has_extra = true;
            break;
        }
        hits.push(Hit {
            row_id: 0,
            file_id,
            line_number,
        });
    }
    let complete = !output.capped && !output.timed_out && !has_extra && matches!(code, Some(0 | 1));
    result.scan_complete = complete;
    result.results_truncated = output.capped || output.timed_out || has_extra;
    if has_extra && !output.capped && !output.timed_out {
        if let Some(last) = hits.last() {
            result.next_cursor = Some(make_cursor(&request, &generation.manifest, *last)?);
        }
    }
    let index_path = generation.root.join("index.sqlite3");
    result.matches = tokio::task::spawn_blocking(move || {
        let connection = Connection::open_with_flags(index_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|error| format!("打开日志索引失败：{error}"))?;
        materialize_matches(&connection, &hits, &request)
    })
    .await
    .map_err(|error| format!("日志命中读取任务失败：{error}"))??;
    Ok(result)
}

fn parse_counts(
    stdout: &[u8],
    paths: &[PathBuf],
    files: &[super::model::SnapshotFile],
    truncated_output: bool,
) -> Result<BTreeMap<u64, u64>, String> {
    let path_ids = paths
        .iter()
        .zip(files)
        .map(|(path, file)| (path.to_string_lossy().into_owned(), file.file_id))
        .collect::<BTreeMap<_, _>>();
    let mut counts = BTreeMap::new();
    for line in String::from_utf8_lossy(stdout).lines() {
        let Some((path, count)) = line.rsplit_once(':') else {
            continue;
        };
        let Some(file_id) = path_ids.get(path).copied() else {
            continue;
        };
        match count.parse::<u64>() {
            Ok(count) => {
                counts.insert(file_id, count);
            }
            // 输出被上限/超时截断时末尾必然是半行：按部分结果收口而非整体失败。
            Err(_) if truncated_output => continue,
            Err(_) => return Err(format!("无法解析 rg 计数输出：{line}")),
        }
    }
    Ok(counts)
}

fn event_text(value: &Value) -> Option<String> {
    if let Some(text) = value.get("text").and_then(Value::as_str) {
        return Some(text.to_string());
    }
    let encoded = value.get("bytes").and_then(Value::as_str)?;
    STANDARD
        .decode(encoded)
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

struct ProcessOutput {
    status: Option<ExitStatus>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    capped: bool,
    cancelled: bool,
    timed_out: bool,
}

impl ProcessOutput {
    fn warnings(&self) -> Vec<String> {
        let mut warnings = Vec::new();
        if self.capped {
            warnings.push("扫描输出达到上限；结果不完整，请缩小模式或 file_ids 后重试".into());
        }
        if self.timed_out {
            warnings
                .push("扫描超过 240 秒；结果不完整，请使用索引友好的 fixed 查询或缩小范围".into());
        }
        warnings
    }
}

async fn run_rg(
    mut command: Command,
    stdout_limit: usize,
    cancel: Option<watch::Receiver<bool>>,
) -> Result<ProcessOutput, String> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("启动 ripgrep 失败：{error}"))?;
    let deadline = Instant::now() + SCAN_TIMEOUT;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| "rg stdout 管道缺失".to_string())?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| "rg stderr 管道缺失".to_string())?;
    let mut output = ProcessOutput {
        status: None,
        stdout: Vec::new(),
        stderr: Vec::new(),
        capped: false,
        cancelled: false,
        timed_out: false,
    };
    let mut stdout_done = false;
    let mut stderr_done = false;
    let mut stdout_chunk = [0u8; 32 * 1024];
    let mut stderr_chunk = [0u8; 8 * 1024];
    enum Event {
        Stdout(std::io::Result<usize>),
        Stderr(std::io::Result<usize>),
        Exited(std::io::Result<ExitStatus>),
        Cancelled,
        TimedOut,
    }
    while output.status.is_none() || !stdout_done || !stderr_done {
        let wait_for = deadline.saturating_duration_since(Instant::now());
        let event = tokio::select! {
            read = stdout.read(&mut stdout_chunk), if !stdout_done => Event::Stdout(read),
            read = stderr.read(&mut stderr_chunk), if !stderr_done => Event::Stderr(read),
            exited = child.wait(), if output.status.is_none() => Event::Exited(exited),
            _ = crate::shared::cancel::wait_for_cancel(cancel.clone()), if !output.cancelled => Event::Cancelled,
            _ = tokio::time::sleep(wait_for), if !output.timed_out => Event::TimedOut,
        };
        let mut kill = false;
        match event {
            Event::Stdout(Ok(0)) => stdout_done = true,
            Event::Stdout(Ok(read)) => {
                output.capped |=
                    append_capped(&mut output.stdout, &stdout_chunk[..read], stdout_limit);
                kill = output.capped;
            }
            Event::Stdout(Err(error)) => return Err(format!("读取 rg stdout 失败：{error}")),
            Event::Stderr(Ok(0)) => stderr_done = true,
            Event::Stderr(Ok(read)) => {
                append_capped(&mut output.stderr, &stderr_chunk[..read], STDERR_LIMIT);
            }
            Event::Stderr(Err(error)) => return Err(format!("读取 rg stderr 失败：{error}")),
            Event::Exited(result) => {
                output.status = Some(result.map_err(|error| format!("等待 rg 失败：{error}"))?);
            }
            Event::Cancelled => {
                output.cancelled = true;
                kill = true;
            }
            Event::TimedOut => {
                output.timed_out = true;
                kill = true;
            }
        }
        if kill {
            let _ = child.start_kill();
        }
    }
    Ok(output)
}

fn append_capped(target: &mut Vec<u8>, chunk: &[u8], limit: usize) -> bool {
    let remaining = limit.saturating_sub(target.len());
    target.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
    chunk.len() > remaining
}
