use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::time::Instant;

use rusqlite::{params, Connection};
use tokio::sync::watch;

use super::model::SnapshotFile;

const MAX_INDEX_LINE_BYTES: usize = 1024 * 1024;
const INDEX_BATCH_LINES: usize = 5_000;

#[derive(Debug)]
pub(crate) enum IndexError {
    Cancelled(String),
    Failed(String),
}

pub(crate) struct IndexOutcome {
    pub files: Vec<SnapshotFile>,
    pub index_bytes: u64,
    pub backend: String,
}

pub(crate) fn build_index(
    staging: &Path,
    inputs: &[SnapshotFile],
    max_index_bytes: u64,
    cancel: watch::Receiver<bool>,
    deadline: Instant,
) -> Result<IndexOutcome, IndexError> {
    let index_path = staging.join("index.sqlite3");
    let connection = Connection::open(&index_path).map_err(index_failure)?;
    connection
        .execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA cache_size=-16000;
             CREATE TABLE files(
               id INTEGER PRIMARY KEY,
               source_label TEXT NOT NULL,
               raw_path TEXT NOT NULL,
               size_bytes INTEGER NOT NULL,
               line_count INTEGER NOT NULL,
               long_lines INTEGER NOT NULL
             );
             CREATE TABLE lines(
               id INTEGER PRIMARY KEY,
               file_id INTEGER NOT NULL,
               line_no INTEGER NOT NULL,
               byte_offset INTEGER NOT NULL,
               byte_length INTEGER NOT NULL,
               text TEXT NOT NULL,
               text_truncated INTEGER NOT NULL,
               UNIQUE(file_id, line_no)
             );
             CREATE INDEX lines_file_line ON lines(file_id, line_no);
             CREATE VIRTUAL TABLE lines_fts USING fts5(
               text,
               content='lines',
               content_rowid='id',
               tokenize='trigram'
             );",
        )
        .map_err(index_failure)?;
    let mut files = inputs.to_vec();
    let mut pending = 0usize;
    let mut transaction_open = false;
    for file in &mut files {
        check_work(&cancel, deadline)?;
        connection
            .execute(
                "INSERT INTO files(id, source_label, raw_path, size_bytes, line_count, long_lines)
                 VALUES(?1, ?2, ?3, ?4, 0, 0)",
                params![
                    file.file_id as i64,
                    file.source_label,
                    file.raw_path,
                    file.size_bytes as i64
                ],
            )
            .map_err(index_failure)?;
        let raw_path = staging.join(&file.raw_path);
        let raw = std::fs::File::open(&raw_path).map_err(|error| {
            IndexError::Failed(format!("打开索引来源失败 {}：{error}", raw_path.display()))
        })?;
        let mut reader = std::io::BufReader::with_capacity(256 * 1024, raw);
        let mut line_number = 0u64;
        let mut byte_offset = 0u64;
        while let Some(line) = read_bounded_line(&mut reader, |consumed| {
            // 单行超长时行号检查点不会触发，按累计字节数补检查点。
            if *cancel.borrow() {
                return Err(IndexError::Cancelled("日志索引已取消".into()));
            }
            if Instant::now() >= deadline {
                return Err(IndexError::Failed(format!(
                    "日志索引在单行第 {consumed} 字节处超过更新时间预算"
                )));
            }
            Ok(())
        })
        .map_err(|error| match error {
            ReadLineError::Io(error) => index_failure(error),
            ReadLineError::Checkpoint(error) => error,
        })? {
            line_number += 1;
            if line_number.is_multiple_of(1024) {
                check_work(&cancel, deadline)?;
            }
            if pending == 0 {
                connection.execute_batch("BEGIN").map_err(index_failure)?;
                transaction_open = true;
            }
            let text = String::from_utf8_lossy(&line.prefix).into_owned();
            let truncated = line.truncated || text.contains('\u{fffd}');
            connection
                .prepare_cached(
                    "INSERT INTO lines(file_id, line_no, byte_offset, byte_length, text, text_truncated)
                     VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
                )
                .and_then(|mut statement| {
                    statement.execute(params![
                        file.file_id as i64,
                        line_number as i64,
                        byte_offset as i64,
                        line.raw_length as i64,
                        &text,
                        truncated
                    ])
                })
                .map_err(index_failure)?;
            let row_id = connection.last_insert_rowid();
            connection
                .prepare_cached("INSERT INTO lines_fts(rowid, text) VALUES(?1, ?2)")
                .and_then(|mut statement| statement.execute(params![row_id, &text]))
                .map_err(index_failure)?;
            pending += 1;
            byte_offset += line.raw_length;
            file.long_lines += u64::from(line.truncated);
            if pending >= INDEX_BATCH_LINES {
                connection.execute_batch("COMMIT").map_err(index_failure)?;
                transaction_open = false;
                pending = 0;
            }
        }
        file.line_count = line_number;
        connection
            .execute(
                "UPDATE files SET line_count=?2, long_lines=?3 WHERE id=?1",
                params![
                    file.file_id as i64,
                    file.line_count as i64,
                    file.long_lines as i64
                ],
            )
            .map_err(index_failure)?;
    }
    if transaction_open {
        connection.execute_batch("COMMIT").map_err(index_failure)?;
    }
    connection
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA optimize;")
        .map_err(index_failure)?;
    connection
        .close()
        .map_err(|(_, error)| index_failure(error))?;
    let index_bytes = index_size(&index_path).map_err(IndexError::Failed)?;
    if index_bytes > max_index_bytes {
        return Err(IndexError::Failed(format!(
            "日志索引大小 {index_bytes} 超过本次上限 {max_index_bytes}"
        )));
    }
    Ok(IndexOutcome {
        files,
        index_bytes,
        backend: "fts5-trigram".into(),
    })
}

pub(crate) struct BoundedLine {
    pub prefix: Vec<u8>,
    pub raw_length: u64,
    pub truncated: bool,
}

/// 读行失败：I/O 错误或检查点回调中止（取消/超预算）。
pub(crate) enum ReadLineError<E> {
    Io(std::io::Error),
    Checkpoint(E),
}

/// 每累计消费这么多个字节调用一次检查点（单行数 GB 时仍能响应取消/截止）。
const LINE_CHECKPOINT_BYTES: u64 = 8 * 1024 * 1024;

pub(crate) fn read_bounded_line<E>(
    reader: &mut impl BufRead,
    mut checkpoint: impl FnMut(u64) -> Result<(), E>,
) -> Result<Option<BoundedLine>, ReadLineError<E>> {
    let mut prefix = Vec::new();
    let mut raw_length = 0u64;
    let mut truncated = false;
    let mut saw_input = false;
    let mut next_checkpoint = LINE_CHECKPOINT_BYTES;
    loop {
        if raw_length >= next_checkpoint {
            checkpoint(raw_length).map_err(ReadLineError::Checkpoint)?;
            next_checkpoint = raw_length + LINE_CHECKPOINT_BYTES;
        }
        let available = reader.fill_buf().map_err(ReadLineError::Io)?;
        if available.is_empty() {
            break;
        }
        saw_input = true;
        let newline = available.iter().position(|byte| *byte == b'\n');
        let take = newline.map_or(available.len(), |position| position + 1);
        let chunk = &available[..take];
        raw_length += take as u64;
        let keep = MAX_INDEX_LINE_BYTES
            .saturating_sub(prefix.len())
            .min(chunk.len());
        prefix.extend_from_slice(&chunk[..keep]);
        truncated |= keep < chunk.len();
        let has_newline = newline.is_some();
        reader.consume(take);
        if has_newline {
            break;
        }
    }
    if !saw_input && raw_length == 0 {
        return Ok(None);
    }
    if prefix.last() == Some(&b'\n') {
        prefix.pop();
        if prefix.last() == Some(&b'\r') {
            prefix.pop();
        }
    }
    Ok(Some(BoundedLine {
        prefix,
        raw_length,
        truncated,
    }))
}

fn check_work(cancel: &watch::Receiver<bool>, deadline: Instant) -> Result<(), IndexError> {
    if *cancel.borrow() {
        return Err(IndexError::Cancelled("日志索引已取消".into()));
    }
    if Instant::now() >= deadline {
        return Err(IndexError::Failed("日志索引超过更新时间预算".into()));
    }
    Ok(())
}

fn index_failure(error: impl std::fmt::Display) -> IndexError {
    IndexError::Failed(format!("建立日志全文索引失败：{error}"))
}

fn index_size(path: &Path) -> Result<u64, String> {
    let mut total = 0u64;
    for candidate in [
        PathBuf::from(path),
        PathBuf::from(format!("{}-wal", path.display())),
        PathBuf::from(format!("{}-shm", path.display())),
    ] {
        if candidate.exists() {
            total += std::fs::metadata(&candidate)
                .map_err(|error| format!("读取索引大小失败：{error}"))?
                .len();
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::log_snapshot::model::LogSource;

    #[test]
    fn sqlite_supports_trigram_substring_search() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE VIRTUAL TABLE logs USING fts5(text, tokenize='trigram');
                 INSERT INTO logs(text) VALUES('2026-01-01 ERROR trace=abc-123456'),('数据库连接超时');",
            )
            .unwrap();
        let count: i64 = connection
            .query_row(
                "SELECT count(*) FROM logs WHERE logs MATCH ?1",
                params!["\"c-123\""],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
        let chinese: i64 = connection
            .query_row(
                "SELECT count(*) FROM logs WHERE logs MATCH ?1",
                params!["\"连接超\""],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(chinese, 1);
    }

    #[test]
    fn index_records_offsets_and_searches_substrings() {
        let dir = crate::test_util::TempDirGuard::new("log-index");
        std::fs::create_dir(dir.path().join("raw")).unwrap();
        std::fs::write(
            dir.path().join("raw/1.log"),
            b"start\ntrace=abc-123456\nend\n",
        )
        .unwrap();
        let input = SnapshotFile {
            file_id: 1,
            source: LogSource::LocalFile {
                path: "input.log".into(),
            },
            source_label: "local:input.log".into(),
            raw_path: "raw/1.log".into(),
            size_bytes: 28,
            line_count: 0,
            long_lines: 0,
        };
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let outcome = build_index(
            dir.path(),
            &[input],
            10 * 1024 * 1024,
            cancel_rx,
            Instant::now() + std::time::Duration::from_secs(30),
        )
        .unwrap();
        assert_eq!(outcome.files[0].line_count, 3);
        let connection = Connection::open(dir.path().join("index.sqlite3")).unwrap();
        let (line, offset): (i64, i64) = connection
            .query_row(
                "SELECT l.line_no, l.byte_offset FROM lines l
                 JOIN lines_fts f ON l.id=f.rowid
                 WHERE lines_fts MATCH ?1",
                params!["\"c-123\""],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(line, 2);
        assert_eq!(offset, 6);
    }
}
