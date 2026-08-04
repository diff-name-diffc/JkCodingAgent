use std::fs;
use std::io::{self, Read};

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::task;

use super::super::common::{
    boolish_arg, is_noise, non_empty_string_array_arg, rel, render_labeled_sections, resolve_path,
    usize_arg, with_compression_parameters,
};
use crate::agent::tools::context::ToolContext;
use crate::agent::tools::registry::AgentTool;

pub(super) fn list_dir_tool() -> Box<dyn AgentTool> {
    Box::new(ListDirTool)
}

struct ListDirTool;

#[async_trait]
impl AgentTool for ListDirTool {
    fn name(&self) -> &'static str {
        "list_dir"
    }

    fn description(&self) -> &'static str {
        "列出指定目录下的有界文件结构。recursive=false 只返回直接子项；recursive=true 也最多返回指定 path 之下两个层级，绝不展开整棵工程目录树。文件条目会附带精确总行数，例如 [file] src/app.rs (:128行)，可直接配合 read_file 的 path:start-end 协议继续探索。"
    }

    fn parameters(&self) -> Value {
        with_compression_parameters(
            json!({
                "type": "object",
                "properties": {
                    "paths": {
                        "type": "array",
                        "description": "要查看的目录路径列表。即使只查看一个目录，也必须传单元素数组。传入多个路径时，结果会按目录路径分段返回。",
                        "minItems": 1,
                        "items": { "type": "string" }
                    },
                    "recursive": { "type": "string", "description": "是否包含第二层子项，默认 false。开启后仍严格限制为 path 之下最多两层，不会递归整棵目录树。", "enum": ["true", "false"] },
                    "max_entries": { "type": "integer", "description": "最多返回多少个文件/目录条目，默认 200", "minimum": 1 }
                },
                "required": ["paths"]
            }),
            false,
            "目录结果最多只有两层，文件名后带总行数。需要精确文件清单和行数时保持关闭；只从超长列表中提取结构概览时可开启并写明 compress_intent。",
        )
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> String {
        let Some(paths) = non_empty_string_array_arg(args, "paths") else {
            return "错误：缺少必填参数 paths，且 paths 必须是非空字符串数组".to_string();
        };
        let recursive = boolish_arg(args, "recursive").unwrap_or(false);
        let max_entries = usize_arg(args, "max_entries").unwrap_or(200).max(1);
        let context = context.clone();

        match task::spawn_blocking(move || {
            let sections = paths
                .iter()
                .map(|path| {
                    (
                        format!("list_dir path={path}"),
                        list_dir_entries(path, recursive, max_entries, &context),
                    )
                })
                .collect::<Vec<_>>();

            // 单路径也保留根目录标签，便于 Agent 将相对文件名组装为 read_file 定位。
            render_labeled_sections(sections)
        })
        .await
        {
            Ok(output) => output,
            Err(error) => format!("读取目录任务失败：{error}"),
        }
    }
}

fn list_dir_entries(
    path: &str,
    recursive: bool,
    max_entries: usize,
    context: &ToolContext,
) -> String {
    let dir_path = match resolve_path(context, path) {
        Ok(path) => path,
        Err(message) => return message,
    };
    if !dir_path.exists() {
        return format!("错误：目录不存在：{path}");
    }
    if !dir_path.is_dir() {
        return format!("错误：{path} 不是目录");
    }

    let max_depth = if recursive { 2 } else { 1 };
    let mut listing = DirectoryListing::new(max_entries);
    collect_dir_entries(&dir_path, &dir_path, 0, max_depth, &mut listing);
    listing.finish(max_depth)
}

struct DirectoryListing {
    entries: Vec<String>,
    max_entries: usize,
    truncated: bool,
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

    fn push(&mut self, entry: String) -> bool {
        if self.is_full() {
            self.truncated = true;
            return false;
        }
        self.entries.push(entry);
        true
    }

    fn finish(mut self, max_depth: usize) -> String {
        if self.truncated {
            self.entries.push(format!(
                "... [目录列表已截断：最多展示 {} 个条目，层级上限为 path 之下 {max_depth} 层]",
                self.max_entries
            ));
        }
        self.entries.join("\n")
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
            listing.push(format!(
                "[错误] 无法读取目录 {}: {error}",
                rel(current, root)
            ));
            return;
        }
    };
    let mut items = Vec::new();
    for entry in read_dir {
        match entry {
            Ok(entry) => items.push(entry),
            Err(error) => {
                if !listing.push(format!(
                    "[错误] 目录条目读取失败 {}: {error}",
                    rel(current, root)
                )) {
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
                listing.push(format!(
                    "[错误] {} (:无法读取类型：{error})",
                    rel(&path, root)
                ));
                continue;
            }
        };
        let entry_depth = current_depth + 1;

        if file_type.is_dir() {
            if !listing.push(format!("[dir] {}/", rel(&path, root))) {
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
            let line_info = match file_total_lines(&path) {
                Ok(lines) => format!(":{lines}行"),
                Err(error) => format!(":行数读取失败：{error}"),
            };
            if !listing.push(format!("[file] {} ({line_info})", rel(&path, root))) {
                return;
            }
        } else if file_type.is_symlink() && !listing.push(format!("[symlink] {}", rel(&path, root)))
        {
            return;
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
        let rendered = listing.finish(2);

        assert!(rendered.contains("[file] top.rs (:2行)"));
        assert!(rendered.contains("child.py (:3行)"));
        assert!(rendered.contains("[dir] first/"));
        assert!(rendered.contains("second/"));
        assert!(!rendered.contains("deep.ts"));
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
        let rendered = listing.finish(1);

        assert!(rendered.contains("[dir] child/"));
        assert!(rendered.contains("[file] top.rs (:1行)"));
        assert!(!rendered.contains("nested.rs"));
    }

    #[test]
    fn directory_listing_marks_the_entry_limit_truncation() {
        let temp = TestDirectory::new();
        fs::write(temp.path().join("a.rs"), "one\n").unwrap();
        fs::write(temp.path().join("b.rs"), "two\n").unwrap();

        let mut listing = DirectoryListing::new(1);
        collect_dir_entries(temp.path(), temp.path(), 0, 1, &mut listing);
        let rendered = listing.finish(1);

        assert!(rendered.contains("[file] a.rs (:1行)"));
        assert!(rendered.contains("目录列表已截断"));
        assert!(!rendered.contains("b.rs"));
    }
}
