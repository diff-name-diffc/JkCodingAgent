use std::io::{Read, Seek, SeekFrom};

use rusqlite::{params, Connection, OpenFlags};

use super::index::read_bounded_line;
use super::query::{LoadedGeneration, LogLine, LogReadRequest, LogReadResult};
use super::store::SnapshotStore;

pub(crate) fn execute(
    store: &SnapshotStore,
    generation: &LoadedGeneration,
    request: LogReadRequest,
) -> Result<LogReadResult, String> {
    let started = std::time::Instant::now();
    let (file, path) = store.raw_file_path(generation, request.file_id)?;
    let actual_size = std::fs::metadata(&path)
        .map_err(|error| format!("读取日志文件元数据失败：{error}"))?
        .len();
    if actual_size != file.size_bytes {
        return Err("日志代次文件大小与索引不一致，可能已损坏；请重新 update".into());
    }
    let mut result = if let Some(offset) = request.byte_offset {
        read_bytes(store, generation, request, file, path, offset)?
    } else {
        read_lines(store, generation, request, file, path)?
    };
    result.duration_ms = super::model::duration_ms(started.elapsed());
    result.warnings = generation.manifest.warnings.clone();
    Ok(result)
}

fn read_bytes(
    _store: &SnapshotStore,
    generation: &LoadedGeneration,
    request: LogReadRequest,
    file: super::model::SnapshotFile,
    path: std::path::PathBuf,
    offset: u64,
) -> Result<LogReadResult, String> {
    if offset > file.size_bytes {
        return Err("byte_offset 超出文件大小".into());
    }
    let mut input =
        std::fs::File::open(&path).map_err(|error| format!("打开日志原文失败：{error}"))?;
    input
        .seek(SeekFrom::Start(offset))
        .map_err(|error| format!("定位日志原文失败：{error}"))?;
    let mut bytes = Vec::new();
    input
        .take(request.max_bytes as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("读取日志原文失败：{error}"))?;
    // max_bytes 窗口可能落在多字节 UTF-8 字符中间：error_len() 为 None 说明
    // 只是尾部字符不完整（不是内容本身无效），截掉半个字符并把 next_byte_offset
    // 对齐到字符边界，避免文本与下一页开头出现 U+FFFD 被当成日志原文。
    if let Err(error) = std::str::from_utf8(&bytes) {
        if error.error_len().is_none() {
            bytes.truncate(error.valid_up_to());
        }
    }
    let next_offset = offset + bytes.len() as u64;
    let next = (next_offset < file.size_bytes).then_some(next_offset);
    Ok(LogReadResult {
        snapshot_id: generation.manifest.snapshot_id.clone(),
        generation: generation.manifest.generation,
        duration_ms: 0,
        warnings: Vec::new(),
        file,
        lines: Vec::new(),
        text: Some(String::from_utf8_lossy(&bytes).into_owned()),
        next_line: None,
        next_byte_offset: next,
        truncated: next.is_some(),
    })
}

fn read_lines(
    _store: &SnapshotStore,
    generation: &LoadedGeneration,
    request: LogReadRequest,
    file: super::model::SnapshotFile,
    path: std::path::PathBuf,
) -> Result<LogReadResult, String> {
    let target = request.line_number.unwrap_or(1);
    let start = target.saturating_sub(request.before as u64).max(1);
    let end = target.saturating_add(request.after as u64);
    let mut result = LogReadResult {
        snapshot_id: generation.manifest.snapshot_id.clone(),
        generation: generation.manifest.generation,
        duration_ms: 0,
        warnings: Vec::new(),
        file: file.clone(),
        lines: Vec::new(),
        text: None,
        next_line: None,
        next_byte_offset: None,
        truncated: false,
    };
    if start > file.line_count || start > end {
        return Ok(result);
    }
    let connection = Connection::open_with_flags(
        generation.root.join("index.sqlite3"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|error| format!("打开日志索引失败：{error}"))?;
    let start_offset: u64 = connection
        .query_row(
            "SELECT byte_offset FROM lines WHERE file_id=?1 AND line_no=?2",
            params![file.file_id as i64, start as i64],
            |row| row.get::<_, i64>(0).map(|value| value as u64),
        )
        .map_err(|error| format!("定位日志行失败：{error}"))?;
    drop(connection);
    let mut input =
        std::fs::File::open(&path).map_err(|error| format!("打开日志原文失败：{error}"))?;
    input
        .seek(SeekFrom::Start(start_offset))
        .map_err(|error| format!("定位日志原文失败：{error}"))?;
    let mut reader = std::io::BufReader::with_capacity(256 * 1024, input);
    let mut current = start;
    let mut offset = start_offset;
    let mut bytes_used = 0usize;
    while current <= end && current <= file.line_count {
        let Some(line) = read_bounded_line(&mut reader, |_| Ok::<(), String>(())).map_err(
            |error| match error {
                super::index::ReadLineError::Io(error) => {
                    format!("读取日志原文失败：{error}")
                }
                super::index::ReadLineError::Checkpoint(message) => message,
            },
        )?
        else {
            return Err("日志代次行数与原文不一致，可能已损坏".into());
        };
        if !result.lines.is_empty() && bytes_used + line.prefix.len() > request.max_bytes {
            result.next_line = Some(current);
            break;
        }
        bytes_used += line.prefix.len();
        let text = String::from_utf8_lossy(&line.prefix).into_owned();
        let clipped = text
            .chars()
            .take(request.max_line_chars)
            .collect::<String>();
        let clipped_by_chars = clipped.chars().count() < text.chars().count();
        result.truncated |= line.truncated || clipped_by_chars;
        result.lines.push(LogLine {
            file_id: file.file_id,
            line_number: current,
            byte_offset: offset,
            byte_length: line.raw_length,
            text: clipped,
            text_truncated: line.truncated || clipped_by_chars || text.contains('\u{fffd}'),
            context: current != target,
        });
        offset += line.raw_length;
        current += 1;
    }
    if result.next_line.is_none() && current <= file.line_count {
        result.next_line = Some(current);
    }
    result.truncated |= result.next_line.is_some();
    Ok(result)
}
