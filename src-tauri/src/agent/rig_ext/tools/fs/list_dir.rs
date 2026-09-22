//! list_dir 工具（rig 形态）。迁移自旧自实现工具层（已随迁移删除）。
//! 语义不变：最多两层的有界目录列举、噪声目录过滤、文件条目附带精确总行数
//! （>4MB 跳过统计）、条目数截断标注；全路径失败返回「错误：」可恢复错误。
//! 偏差：旧的结构化 `data` 载荷（entries/maxDepth/truncated）不再产出。

use std::fs;
use std::io::{self, Read};

use rig::tool::{PortableDynamicTool, ToolExecutionError, ToolOutput};
use serde_json::{json, Value};
use tokio::task;

use super::super::common::{
    boolish_arg, is_noise, non_empty_string_array_arg, rel, render_labeled_sections, usize_arg,
    with_compression_parameters, DEFAULT_FORCE_COMPRESS_AFTER_CHARS,
};
use super::FsSandbox;

/// 行数统计会完整读取文件内容。超过该阈值的文件跳过行数统计并显式标注，
/// 避免对大体积 bundle / 日志 / 二进制文件逐字节读取、长时间占用阻塞线程池。
const LINE_COUNT_MAX_BYTES: u64 = 4 * 1024 * 1024;

const DESCRIPTION: &str = "列出指定目录下的有界文件结构。recursive=false 只返回直接子项；recursive=true 也最多返回指定 path 之下两个层级，绝不展开整棵工程目录树。文件条目会附带精确总行数，例如 [file] src/app.rs (:128行)，可直接配合 read_file 的 path:start-end 协议继续探索。";

pub(super) fn list_dir_tool(sandbox: FsSandbox) -> PortableDynamicTool {
    PortableDynamicTool::new("list_dir", DESCRIPTION, parameters(), move |args| {
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
                    "description": "要查看的目录路径列表。即使只查看一个目录，也必须传单元素数组。传入多个路径时，结果会按目录路径分段返回。",
                    "minItems": 1,
                    "maxItems": 4,
                    "items": { "type": "string", "minLength": 1, "maxLength": 4096 }
                },
                "recursive": { "type": "boolean", "description": "是否包含第二层子项，默认 false。开启后仍严格限制为 path 之下最多两层，不会递归整棵目录树。", "default": false },
                "max_entries": { "type": "integer", "description": "每个目录最多返回多少个文件/目录条目，默认 200", "minimum": 1, "maximum": 200 }
            },
            "required": ["paths"]
        }),
        false,
        DEFAULT_FORCE_COMPRESS_AFTER_CHARS,
        "目录结果最多只有两层，文件名后带总行数。需要精确文件清单和行数时保持关闭；只从超长列表中提取结构概览时可开启并写明 compress_intent。",
    )
}

async fn execute(args: Value, sandbox: FsSandbox) -> Result<ToolOutput, ToolExecutionError> {
    let Some(paths) = non_empty_string_array_arg(&args, "paths") else {
        return Err(ToolExecutionError::other(
            "错误：缺少必填参数 paths，且 paths 必须是非空字符串数组",
        ));
    };
    let recursive = boolish_arg(&args, "recursive").unwrap_or(false);
    let max_entries = usize_arg(&args, "max_entries").unwrap_or(200).max(1);

    match task::spawn_blocking(move || {
        let outcomes = paths
            .iter()
            .map(|path| {
                (
                    path,
                    list_dir_entries(path, recursive, max_entries, &sandbox),
                )
            })
            .collect::<Vec<_>>();

        // 单路径也保留根目录标签，便于 Agent 将相对文件名组装为 read_file 定位。
        let display = render_labeled_sections(
            outcomes
                .iter()
                .map(|(path, outcome)| (format!("list_dir path={path}"), outcome.display.clone()))
                .collect(),
        );
        let all_failed =
            !outcomes.is_empty() && outcomes.iter().all(|(_, outcome)| outcome.is_error);
        (display, all_failed)
    })
    .await
    {
        Ok((display, true)) => Err(ToolExecutionError::other(display)),
        Ok((display, false)) => Ok(ToolOutput::text(display)),
        Err(error) => Err(ToolExecutionError::other(format!(
            "错误：读取目录任务失败：{error}"
        ))),
    }
}

struct DirectoryOutcome {
    display: String,
    is_error: bool,
}

impl DirectoryOutcome {
    fn error(message: String) -> Self {
        Self {
            display: message,
            is_error: true,
        }
    }
}

fn list_dir_entries(
    path: &str,
    recursive: bool,
    max_entries: usize,
    sandbox: &FsSandbox,
) -> DirectoryOutcome {
    let dir_path = match sandbox.resolve(path) {
        Ok(path) => path,
        Err(message) => return DirectoryOutcome::error(message),
    };
    if !dir_path.exists() {
        return DirectoryOutcome::error(format!("错误：目录不存在：{path}"));
    }
    if !dir_path.is_dir() {
        return DirectoryOutcome::error(format!("错误：{path} 不是目录"));
    }

    let max_depth = if recursive { 2 } else { 1 };
    let mut listing = DirectoryListing::new(max_entries);
    collect_dir_entries(&dir_path, &dir_path, 0, max_depth, &mut listing);
    let (display, _, _) = listing.finish(max_depth);
    DirectoryOutcome {
        display,
        is_error: false,
    }
}

struct DirectoryListing {
    entries: Vec<DirectoryEntry>,
    max_entries: usize,
    truncated: bool,
}

struct DirectoryEntry {
    display: String,
}

impl DirectoryListing {
    fn new(max_entries: usize) -> Self {
        Self {
            entries: Vec::new(),
            max_entries,
            truncated: false,
        }
    }

    fn is_full(&self) -> bool {
        self.entries.len() >= self.max_entries
    }

    fn push(&mut self, display: String) -> bool {
        if self.is_full() {
            self.truncated = true;
            return false;
        }
        self.entries.push(DirectoryEntry { display });
        true
    }

    fn finish(self, max_depth: usize) -> (String, Vec<String>, bool) {
        // 截断提示与条目列表分离（空行隔开的独立尾注）：条目数严格不超过
        // max_entries，提示行也不会被 Agent 误当作目录条目解析。
        let body = self
            .entries
            .iter()
            .map(|entry| entry.display.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let display = if self.truncated {
            format!(
                "{body}\n\n[目录列表已截断：最多展示 {} 个条目，层级上限为 path 之下 {max_depth} 层]",
                self.max_entries
            )
        } else {
            body
        };
        (
            display,
            self.entries
                .into_iter()
                .map(|entry| entry.display)
                .collect(),
            self.truncated,
        )
    }
}

fn collect_dir_entries(
    root: &std::path::Path,
    current: &std::path::Path,
    current_depth: usize,
    max_depth: usize,
    listing: &mut DirectoryListing,
) {
    if listing.is_full() || current_depth >= max_depth {
        return;
    }

    let read_dir = match fs::read_dir(current) {
        Ok(read_dir) => read_dir,
        Err(error) => {
            let path = rel(current, root);
            listing.push(format!("错误：无法读取目录 {path}: {error}"));
            return;
        }
    };
    let mut items = Vec::new();
    for entry in read_dir {
        match entry {
            Ok(entry) => items.push(entry),
            Err(error) => {
                let path = rel(current, root);
                if !listing.push(format!("错误：目录条目读取失败 {path}: {error}")) {
                    return;
                }
            }
        }
    }
    items.sort_by_key(|entry| entry.file_name());

    for item in items {
        if listing.is_full() {
            listing.truncated = true;
            return;
        }
        if is_noise(&item.file_name()) {
            continue;
        }

        let path = item.path();
        let file_type = match item.file_type() {
            Ok(file_type) => file_type,
            Err(error) => {
                let relative = rel(&path, root);
                listing.push(format!("错误：{relative} (无法读取类型：{error})"));
                continue;
            }
        };
        let entry_depth = current_depth + 1;

        if file_type.is_dir() {
            let relative = rel(&path, root);
            if !listing.push(format!("[dir] {relative}/")) {
                return;
            }
            if entry_depth < max_depth {
                if listing.is_full() {
                    listing.truncated = true;
                    return;
                }
                collect_dir_entries(root, &path, entry_depth, max_depth, listing);
            }
        } else if file_type.is_file() {
            let line_info = match fs::metadata(&path) {
                // 行数统计会完整读文件；超大文件（bundle、日志等）直接跳过，
                // 避免 list_dir 在阻塞线程池上长时间逐字节读取。
                Ok(meta) if meta.len() > LINE_COUNT_MAX_BYTES => {
                    ":行数未统计（文件过大）".to_string()
                }
                _ => match file_total_lines(&path) {
                    Ok(lines) => format!(":{lines}行"),
                    Err(error) => format!(":错误：行数读取失败：{error}"),
                },
            };
            let relative = rel(&path, root);
            if !listing.push(format!("[file] {relative} ({line_info})")) {
                return;
            }
        } else if file_type.is_symlink() {
            let relative = rel(&path, root);
            if !listing.push(format!("[symlink] {relative}")) {
                return;
            }
        }
    }
}

fn file_total_lines(path: &std::path::Path) -> io::Result<usize> {
    let file = fs::File::open(path)?;
    count_reader_lines(file)
}

fn count_reader_lines(mut reader: impl Read) -> io::Result<usize> {
    let mut buffer = [0_u8; 64 * 1024];
    let mut line_count = 0;
    let mut total_bytes = 0;
    let mut last_byte = None;

    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total_bytes += read;
        line_count += buffer[..read].iter().filter(|byte| **byte == b'\n').count();
        last_byte = buffer.get(read - 1).copied();
    }

    if total_bytes > 0 && last_byte != Some(b'\n') {
        line_count += 1;
    }
    Ok(line_count)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Cursor;
    use std::path::{Path, PathBuf};

    use super::{collect_dir_entries, count_reader_lines, DirectoryListing};

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "jkcodingagent-list-dir-test-{}",
                uuid::Uuid::new_v4()
            ));
            fs::create_dir_all(&path).expect("create test directory");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn counts_empty_and_trailing_newline_files_exactly() {
        assert_eq!(count_reader_lines(Cursor::new("")).unwrap(), 0);
        assert_eq!(count_reader_lines(Cursor::new("one")).unwrap(), 1);
        assert_eq!(count_reader_lines(Cursor::new("one\ntwo\n")).unwrap(), 2);
        assert_eq!(
            count_reader_lines(Cursor::new("one\ntwo\nthree")).unwrap(),
            3
        );
    }

    #[test]
    fn directory_listing_stops_at_two_levels_and_appends_file_line_counts() {
        let temp = TestDirectory::new();
        let first = temp.path().join("first");
        let second = first.join("second");
        fs::create_dir_all(&second).unwrap();
        fs::write(temp.path().join("top.rs"), "one\ntwo\n").unwrap();
        fs::write(first.join("child.py"), "one\ntwo\nthree").unwrap();
        fs::write(second.join("deep.ts"), "hidden\n").unwrap();

        let mut listing = DirectoryListing::new(100);
        collect_dir_entries(temp.path(), temp.path(), 0, 2, &mut listing);
        let (rendered, entries, truncated) = listing.finish(2);

        assert!(rendered.contains("[file] top.rs (:2行)"));
        assert!(rendered.contains("child.py (:3行)"));
        assert!(rendered.contains("[dir] first/"));
        assert!(rendered.contains("second/"));
        assert!(!rendered.contains("deep.ts"));
        assert_eq!(entries.len(), 4);
        assert!(!truncated);
    }

    #[test]
    fn non_recursive_directory_listing_returns_only_direct_children() {
        let temp = TestDirectory::new();
        let child = temp.path().join("child");
        fs::create_dir_all(&child).unwrap();
        fs::write(temp.path().join("top.rs"), "one\n").unwrap();
        fs::write(child.join("nested.rs"), "two\n").unwrap();

        let mut listing = DirectoryListing::new(100);
        collect_dir_entries(temp.path(), temp.path(), 0, 1, &mut listing);
        let (rendered, entries, truncated) = listing.finish(1);

        assert!(rendered.contains("[dir] child/"));
        assert!(rendered.contains("[file] top.rs (:1行)"));
        assert!(!rendered.contains("nested.rs"));
        assert_eq!(entries.len(), 2);
        assert!(!truncated);
    }

    #[test]
    fn directory_listing_marks_the_entry_limit_truncation() {
        let temp = TestDirectory::new();
        fs::write(temp.path().join("a.rs"), "one\n").unwrap();
        fs::write(temp.path().join("b.rs"), "two\n").unwrap();

        let mut listing = DirectoryListing::new(1);
        collect_dir_entries(temp.path(), temp.path(), 0, 1, &mut listing);
        let (rendered, entries, truncated) = listing.finish(1);

        assert!(rendered.contains("[file] a.rs (:1行)"));
        assert!(rendered.contains("目录列表已截断"));
        assert!(!rendered.contains("b.rs"));
        assert_eq!(entries.len(), 1);
        assert!(truncated);
    }
}
