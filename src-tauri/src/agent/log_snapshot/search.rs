use std::collections::BTreeMap;

use rusqlite::{params, Connection, OpenFlags};
use tokio::sync::watch;

use super::model::GenerationManifest;
use super::query::{
    decode_cursor, encode_cursor, query_hash, CursorPosition, FileMatchCount, LoadedGeneration,
    LogLine, LogMatch, LogSearchRequest, LogSearchResult, OutputMode,
};

#[derive(Clone, Copy, Debug)]
pub(crate) struct Hit {
    pub row_id: u64,
    pub file_id: u64,
    pub line_number: u64,
}

pub(crate) async fn execute(
    generation: LoadedGeneration,
    request: LogSearchRequest,
    cancel: Option<watch::Receiver<bool>>,
) -> Result<LogSearchResult, String> {
    if request.can_use_index() {
        let generation_clone = generation.clone();
        let request_clone = request.clone();
        let indexed = tokio::task::spawn_blocking(move || {
            search_index(&generation_clone, &request_clone, cancel)
        })
        .await
        .map_err(|error| format!("日志索引查询任务失败：{error}"))??;
        return Ok(indexed);
    }
    super::scan::execute(generation, request, cancel).await
}

fn search_index(
    generation: &LoadedGeneration,
    request: &LogSearchRequest,
    cancel: Option<watch::Receiver<bool>>,
) -> Result<LogSearchResult, String> {
    let connection = Connection::open_with_flags(
        generation.root.join("index.sqlite3"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|error| format!("打开日志索引失败：{error}"))?;
    let mut result = LogSearchResult::empty(
        generation.manifest.snapshot_id.clone(),
        generation.manifest.generation,
        "fts5-trigram".into(),
        request.output_mode,
    );
    // 索引只保存超长行（>1MB）的前缀：行尾的命中会漏检，必须显式告知模型
    // 改用 engine=scan 复查，而不是静默认为「日志里没有」。
    if generation
        .manifest
        .files
        .iter()
        .any(|file| file.long_lines > 0)
    {
        result.warnings.push(
            "快照中存在超过 1MB 的超长行，索引只覆盖其行首前缀；行尾内容可能漏检，需要确认时改用 engine=scan 复查".into(),
        );
    }
    let fts_query = request
        .patterns
        .iter()
        .map(|pattern| format!("\"{}\"", pattern.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ");
    if request.output_mode == OutputMode::Matches {
        let after = validate_cursor(request, &generation.manifest)?
            .map(|cursor| cursor.row_id)
            .unwrap_or(0);
        let (mut hits, truncated) =
            collect_match_page(&connection, request, &fts_query, after, &cancel)?;
        if hits.len() > request.max_matches {
            hits.truncate(request.max_matches);
        }
        result.results_truncated = truncated;
        result.scan_complete = !truncated;
        if truncated {
            if let Some(last) = hits.last() {
                result.next_cursor = Some(make_cursor(request, &generation.manifest, *last)?);
            }
        }
        result.matches = materialize_matches(&connection, &hits, request)?;
    } else {
        let counts = collect_counts(&connection, request, &fts_query, &cancel)?;
        result.total_match_lines = Some(counts.values().sum());
        result.files = counts
            .into_iter()
            .filter_map(|(file_id, match_count)| {
                generation
                    .manifest
                    .files
                    .iter()
                    .find(|file| file.file_id == file_id)
                    .map(|file| FileMatchCount {
                        file_id,
                        source_label: file.source_label.clone(),
                        match_count,
                    })
            })
            .collect();
        if result.files.len() > request.max_files {
            result.files.truncate(request.max_files);
            result.results_truncated = true;
        }
    }
    Ok(result)
}

fn collect_match_page(
    connection: &Connection,
    request: &LogSearchRequest,
    fts_query: &str,
    after: u64,
    cancel: &Option<watch::Receiver<bool>>,
) -> Result<(Vec<Hit>, bool), String> {
    let mut accepted = Vec::new();
    let mut after_id = after;
    let file_filter = file_filter(request);
    loop {
        check_cancel(cancel)?;
        let sql = format!(
            "SELECT l.id, l.file_id, l.line_no, l.text
             FROM lines_fts JOIN lines l ON l.id=lines_fts.rowid
             WHERE lines_fts MATCH ?1 AND l.id > ?2{file_filter}
             ORDER BY l.id LIMIT 512"
        );
        let mut statement = connection
            .prepare(&sql)
            .map_err(|error| format!("准备日志索引查询失败：{error}"))?;
        let candidates = statement
            .query_map(params![fts_query, after_id as i64], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(|error| format!("执行日志索引查询失败：{error}"))?;
        let mut scanned = 0usize;
        for candidate in candidates {
            let (row_id, file_id, line_number, text) =
                candidate.map_err(|error| format!("读取日志索引行失败：{error}"))?;
            scanned += 1;
            if scanned.is_multiple_of(512) {
                check_cancel(cancel)?;
            }
            after_id = after_id.max(row_id as u64);
            if fixed_line_matches(&text, &request.patterns, request.case_sensitive) {
                accepted.push(Hit {
                    row_id: row_id as u64,
                    file_id: file_id as u64,
                    line_number: line_number as u64,
                });
                if accepted.len() > request.max_matches {
                    return Ok((accepted, true));
                }
            }
        }
        if scanned < 512 {
            return Ok((accepted, false));
        }
    }
}

fn collect_counts(
    connection: &Connection,
    request: &LogSearchRequest,
    fts_query: &str,
    cancel: &Option<watch::Receiver<bool>>,
) -> Result<BTreeMap<u64, u64>, String> {
    let file_filter = file_filter(request);
    let sql = format!(
        "SELECT l.file_id, l.text FROM lines_fts
         JOIN lines l ON l.id=lines_fts.rowid
         WHERE lines_fts MATCH ?1{file_filter}"
    );
    let mut statement = connection
        .prepare(&sql)
        .map_err(|error| format!("准备日志计数查询失败：{error}"))?;
    let rows = statement
        .query_map(params![fts_query], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| format!("执行日志计数查询失败：{error}"))?;
    let mut counts = BTreeMap::new();
    let mut scanned = 0usize;
    for row in rows {
        scanned += 1;
        if scanned.is_multiple_of(512) {
            check_cancel(cancel)?;
        }
        let (file_id, text) = row.map_err(|error| error.to_string())?;
        if fixed_line_matches(&text, &request.patterns, request.case_sensitive) {
            *counts.entry(file_id as u64).or_insert(0) += 1;
        }
    }
    Ok(counts)
}

fn check_cancel(cancel: &Option<watch::Receiver<bool>>) -> Result<(), String> {
    if cancel
        .as_ref()
        .is_some_and(|receiver| *receiver.borrow() || receiver.has_changed().is_err())
    {
        return Err("错误：日志索引查询已取消".into());
    }
    Ok(())
}

pub(crate) fn materialize_matches(
    connection: &Connection,
    hits: &[Hit],
    request: &LogSearchRequest,
) -> Result<Vec<LogMatch>, String> {
    let mut matches = Vec::with_capacity(hits.len());
    for hit in hits {
        let line = read_line(connection, *hit, false, request.max_line_chars)?;
        let before = if request.context_before > 0 {
            read_context(
                connection,
                hit.file_id,
                hit.line_number
                    .saturating_sub(request.context_before as u64)
                    .max(1),
                hit.line_number.saturating_sub(1),
                request.max_line_chars,
            )?
        } else {
            Vec::new()
        };
        let after = if request.context_after > 0 {
            read_context(
                connection,
                hit.file_id,
                hit.line_number + 1,
                hit.line_number + request.context_after as u64,
                request.max_line_chars,
            )?
        } else {
            Vec::new()
        };
        matches.push(LogMatch {
            line,
            before,
            after,
        });
    }
    Ok(matches)
}

fn read_context(
    connection: &Connection,
    file_id: u64,
    start: u64,
    end: u64,
    max_line_chars: usize,
) -> Result<Vec<LogLine>, String> {
    if end < start {
        return Ok(Vec::new());
    }
    let mut statement = connection
        .prepare(
            "SELECT id, file_id, line_no, byte_offset, byte_length, text, text_truncated
             FROM lines WHERE file_id=?1 AND line_no BETWEEN ?2 AND ?3 ORDER BY line_no",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(
            params![file_id as i64, start as i64, end as i64],
            map_log_line,
        )
        .map_err(|error| error.to_string())?;
    let mut lines = Vec::new();
    for row in rows {
        let (hit, offset, length, text, truncated) = row.map_err(|error| error.to_string())?;
        lines.push(log_line(
            hit,
            offset,
            length,
            &text,
            truncated,
            true,
            max_line_chars,
        ));
    }
    Ok(lines)
}

fn read_line(
    connection: &Connection,
    hit: Hit,
    context: bool,
    max_line_chars: usize,
) -> Result<LogLine, String> {
    let (sql, first, second) = if hit.row_id > 0 {
        ("id=?1 AND ?2=0", hit.row_id, 0)
    } else {
        ("file_id=?1 AND line_no=?2", hit.file_id, hit.line_number)
    };
    let sql = format!(
        "SELECT id, file_id, line_no, byte_offset, byte_length, text, text_truncated
         FROM lines WHERE {sql}"
    );
    connection
        .query_row(&sql, params![first as i64, second as i64], |row| {
            let (hit, offset, length, text, truncated) = map_log_line(row)?;
            Ok(log_line(
                hit,
                offset,
                length,
                &text,
                truncated,
                context,
                max_line_chars,
            ))
        })
        .map_err(|error| format!("读取日志命中位置失败：{error}"))
}

fn map_log_line(row: &rusqlite::Row<'_>) -> rusqlite::Result<(Hit, u64, u64, String, bool)> {
    Ok((
        Hit {
            row_id: row.get::<_, i64>(0)? as u64,
            file_id: row.get::<_, i64>(1)? as u64,
            line_number: row.get::<_, i64>(2)? as u64,
        },
        row.get::<_, i64>(3)? as u64,
        row.get::<_, i64>(4)? as u64,
        row.get(5)?,
        row.get(6)?,
    ))
}

fn log_line(
    hit: Hit,
    byte_offset: u64,
    byte_length: u64,
    text: &str,
    text_truncated: bool,
    context: bool,
    max_line_chars: usize,
) -> LogLine {
    let clipped = text.chars().take(max_line_chars).collect::<String>();
    LogLine {
        file_id: hit.file_id,
        line_number: hit.line_number,
        byte_offset,
        byte_length,
        text_truncated: text_truncated || clipped.chars().count() < text.chars().count(),
        text: clipped,
        context,
    }
}

fn fixed_line_matches(text: &str, patterns: &[String], case_sensitive: bool) -> bool {
    if case_sensitive {
        patterns.iter().any(|pattern| text.contains(pattern))
    } else {
        let text = text.to_lowercase();
        patterns
            .iter()
            .any(|pattern| text.contains(&pattern.to_lowercase()))
    }
}

fn file_filter(request: &LogSearchRequest) -> String {
    if request.file_ids.is_empty() {
        String::new()
    } else {
        format!(
            " AND l.file_id IN ({})",
            request
                .file_ids
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(",")
        )
    }
}

pub(crate) fn validate_cursor(
    request: &LogSearchRequest,
    manifest: &GenerationManifest,
) -> Result<Option<CursorPosition>, String> {
    let Some(raw) = &request.cursor else {
        return Ok(None);
    };
    let cursor = decode_cursor(raw)?;
    if cursor.snapshot_id != manifest.snapshot_id
        || cursor.generation != manifest.generation
        || cursor.query_hash != query_hash(request)
    {
        return Err("cursor 不属于当前日志快照、代次或查询条件".into());
    }
    Ok(Some(cursor))
}

pub(crate) fn make_cursor(
    request: &LogSearchRequest,
    manifest: &GenerationManifest,
    hit: Hit,
) -> Result<String, String> {
    encode_cursor(&CursorPosition {
        snapshot_id: manifest.snapshot_id.clone(),
        generation: manifest.generation,
        query_hash: query_hash(request),
        row_id: hit.row_id,
        file_id: Some(hit.file_id),
        line_number: Some(hit.line_number),
    })
}
