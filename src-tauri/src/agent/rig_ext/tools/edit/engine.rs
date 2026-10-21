//! apply-patch 引擎：解析 apply-patch 格式补丁并应用到本地文件系统。
//!
//! 完整移植自 openai/codex 的 `codex-rs/apply-patch` crate（Apache-2.0）：
//! - 补丁格式与解析语义同 `parser.rs` / `streaming_parser.rs`（含宽松 heredoc
//!   包裹剥离与 `*** End of File` 标记）；
//! - 匹配算法同 `seek_sequence.rs`：精确 → 忽略行尾空白 → 双侧 trim →
//!   常见 Unicode 标点归一化，逐级放宽；
//! - 行尾保真的文件模型同 `text_file.rs`：CRLF/CR/LF 混排文件中未触及的行
//!   保留原行尾，插入行用文件首个行尾风格；
//! - 替换计算同 `file_update.rs::compute_replacements`（change_context 定位、
//!   尾部空行哨兵重试、上下文行原地保留）。
//!
//! 相对 codex 原实现的适配与收敛：
//! - 文件系统层是本地 `std::fs`（codex 抽象了 ExecutorFileSystem / 沙箱上下文）；
//! - 路径解析交给调用方（工具层经 `common::resolve_path` 沙箱化，本引擎只收
//!   已解析的绝对路径）；
//! - 应用策略改为「先全量计算、后统一提交」：所有 Update 的目标内容先在内存
//!   推导成功才开始写盘，任何一块匹配失败都不会改动文件（codex 顺序应用、
//!   失败时可能留下部分变更并靠 delta 追踪；本项目没有回滚台账，计算期
//!   一票否决更安全）。提交期写盘失败仍可能部分落盘，结果里明确列出已
//!   写入的文件；
//! - `*** Environment ID:` 前导行接受并忽略（单环境工具）。

use std::path::{Path, PathBuf};

pub(crate) const BEGIN_PATCH_MARKER: &str = "*** Begin Patch";
pub(crate) const END_PATCH_MARKER: &str = "*** End Patch";
const ADD_FILE_MARKER: &str = "*** Add File: ";
const DELETE_FILE_MARKER: &str = "*** Delete File: ";
const UPDATE_FILE_MARKER: &str = "*** Update File: ";
const MOVE_TO_MARKER: &str = "*** Move to: ";
const EOF_MARKER: &str = "*** End of File";
const ENVIRONMENT_ID_MARKER: &str = "*** Environment ID: ";

/// 单个补丁块的解析产物（语义同 codex `parser::Hunk`）。
#[derive(Debug, PartialEq, Clone)]
#[allow(clippy::enum_variant_names)] // AddFile/DeleteFile/UpdateFile 与 codex 命名保持一致
pub(crate) enum Hunk {
    AddFile {
        path: PathBuf,
        contents: String,
    },
    DeleteFile {
        path: PathBuf,
    },
    UpdateFile {
        path: PathBuf,
        move_path: Option<PathBuf>,
        /// 按出现顺序排列；后一个块的定位必须晚于前一个块。
        chunks: Vec<UpdateFileChunk>,
    },
}

impl Hunk {
    /// 该块影响的目标路径（重命名块取移动目的地）。
    pub(crate) fn path(&self) -> &Path {
        match self {
            Hunk::AddFile { path, .. } => path,
            Hunk::DeleteFile { path } => path,
            Hunk::UpdateFile {
                move_path: Some(dest),
                ..
            } => dest,
            Hunk::UpdateFile { path, .. } => path,
        }
    }
}

#[derive(Debug, Default, PartialEq, Clone)]
pub(crate) struct UpdateFileChunk {
    /// 缩小定位范围的单行上下文（通常是类/函数定义行），块内容必须出现在
    /// 该行之后。
    pub(crate) change_context: Option<String>,
    /// 被替换的连续行；必须逐行匹配文件现状（经 seek_sequence 模糊匹配）。
    pub(crate) old_lines: Vec<String>,
    pub(crate) new_lines: Vec<String>,
    /// (old_lines 索引, new_lines 索引)：来自补丁 ` ` 前缀上下文行的配对，
    /// 这些行应用时保留文件原文（保行尾），不参与改写。
    pub(crate) context_line_indices: Vec<(usize, usize)>,
    /// 为真时 `old_lines` 必须出现在文件末尾（对末尾换行宽容）。
    pub(crate) is_end_of_file: bool,
}

impl UpdateFileChunk {
    fn push_context_line(&mut self, line: String) {
        self.context_line_indices
            .push((self.old_lines.len(), self.new_lines.len()));
        self.old_lines.push(line.clone());
        self.new_lines.push(line);
    }
}

#[derive(Debug, PartialEq, Clone)]
pub(crate) enum ParseError {
    InvalidPatchError(String),
    InvalidHunkError { message: String, line_number: usize },
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::InvalidPatchError(message) => write!(f, "invalid patch: {message}"),
            ParseError::InvalidHunkError {
                message,
                line_number,
            } => write!(f, "invalid hunk at line {line_number}, {message}"),
        }
    }
}

/// 解析补丁文本为块列表（宽松模式：接受 heredoc 包裹，见模块文档）。
pub(crate) fn parse_patch(patch: &str) -> Result<Vec<Hunk>, ParseError> {
    let lines: Vec<&str> = patch.trim().lines().collect();
    let lines = unwrap_heredoc(lines)?;
    check_patch_boundaries(&lines)?;

    let mut hunks = Vec::new();
    // 行号从 1 计（含 Begin Patch 行），报错能对上补丁原文。
    let mut index = 1usize;
    while index < lines.len().saturating_sub(1) {
        let line = lines[index].trim_start();
        if line.is_empty() {
            index += 1;
            continue;
        }
        if let Some(path) = line.strip_prefix(ADD_FILE_MARKER) {
            let (contents, next) = parse_add_file_body(&lines, index + 1, path)?;
            hunks.push(Hunk::AddFile {
                path: PathBuf::from(path),
                contents,
            });
            index = next;
        } else if let Some(path) = line.strip_prefix(DELETE_FILE_MARKER) {
            hunks.push(Hunk::DeleteFile {
                path: PathBuf::from(path),
            });
            index += 1;
        } else if let Some(path) = line.strip_prefix(UPDATE_FILE_MARKER) {
            let (hunk, next) = parse_update_file(&lines, index + 1, path)?;
            hunks.push(hunk);
            index = next;
        } else if line.starts_with(ENVIRONMENT_ID_MARKER) {
            // 单环境工具：接受并忽略（codex 用于多环境路由）。
            index += 1;
        } else {
            return Err(ParseError::InvalidHunkError {
                message: format!("unexpected line: {line}"),
                line_number: index + 1,
            });
        }
    }
    Ok(hunks)
}

/// 宽松模式边界：优先按严格 Begin/End Patch 校验；失败时尝试剥离
/// `<<EOF ... EOF` / `<<'EOF' ... EOF` heredoc 包裹（模型偶发沿用
/// codex 命令行形态的补丁）。
fn unwrap_heredoc(lines: Vec<&str>) -> Result<Vec<&str>, ParseError> {
    let strict_error = match check_start_and_end(&lines) {
        Ok(()) => return Ok(lines),
        Err(error) => error,
    };
    match lines.as_slice() {
        [first, .., last]
            if matches!(*first, "<<EOF" | "<<'EOF'" | "<<\"EOF\"")
                && last.ends_with("EOF")
                && lines.len() >= 4 =>
        {
            let inner = lines[1..lines.len() - 1].to_vec();
            check_start_and_end(&inner).map_err(|_| strict_error)?;
            Ok(inner)
        }
        _ => Err(strict_error),
    }
}

fn check_patch_boundaries(lines: &[&str]) -> Result<(), ParseError> {
    check_start_and_end(lines)
}

fn check_start_and_end(lines: &[&str]) -> Result<(), ParseError> {
    let (first, last) = match lines {
        [] => (None, None),
        [first] => (Some(first), Some(first)),
        [first, .., last] => (Some(first), Some(last)),
    };
    match (first.map(|line| line.trim()), last.map(|line| line.trim())) {
        (Some(first), Some(last)) if first == BEGIN_PATCH_MARKER && last == END_PATCH_MARKER => {
            Ok(())
        }
        (Some(first), _) if first != BEGIN_PATCH_MARKER => Err(ParseError::InvalidPatchError(
            "The first line of the patch must be '*** Begin Patch'".to_string(),
        )),
        _ => Err(ParseError::InvalidPatchError(
            "The last line of the patch must be '*** End Patch'".to_string(),
        )),
    }
}

/// 解析 Add File 正文：连续的 `+` 行拼接为文件内容（每行补换行）。
fn parse_add_file_body(
    lines: &[&str],
    mut index: usize,
    path: &str,
) -> Result<(String, usize), ParseError> {
    let mut contents = String::new();
    while index < lines.len().saturating_sub(1) {
        let line = lines[index];
        if let Some(text) = line.strip_prefix('+') {
            contents.push_str(text);
            contents.push('\n');
            index += 1;
        } else {
            break;
        }
    }
    if contents.is_empty() {
        return Err(ParseError::InvalidHunkError {
            message: format!("Add file hunk for path '{path}' is empty"),
            line_number: index,
        });
    }
    Ok((contents, index))
}

/// 解析 Update File 正文：可选 Move to + 若干 chunk。
fn parse_update_file(
    lines: &[&str],
    mut index: usize,
    path: &str,
) -> Result<(Hunk, usize), ParseError> {
    let mut move_path = None;
    if let Some(dest) = lines
        .get(index)
        .map(|line| line.trim_start())
        .and_then(|line| line.strip_prefix(MOVE_TO_MARKER))
    {
        move_path = Some(PathBuf::from(dest));
        index += 1;
    }

    let mut chunks: Vec<UpdateFileChunk> = Vec::new();
    let end = lines.len().saturating_sub(1);
    while index < end {
        let line = lines[index];
        let trimmed = line.trim_start();
        if trimmed.starts_with(ADD_FILE_MARKER)
            || trimmed.starts_with(DELETE_FILE_MARKER)
            || trimmed.starts_with(UPDATE_FILE_MARKER)
        {
            break;
        }
        if line == EOF_MARKER || line.trim() == EOF_MARKER {
            if let Some(chunk) = chunks.last_mut() {
                chunk.is_end_of_file = true;
            }
            index += 1;
            continue;
        }
        if let Some(context) = trimmed.strip_prefix("@@ ") {
            chunks.push(UpdateFileChunk {
                change_context: Some(context.to_string()),
                ..Default::default()
            });
            index += 1;
            continue;
        }
        if trimmed == "@@" {
            chunks.push(UpdateFileChunk::default());
            index += 1;
            continue;
        }
        // 补丁行前缀：` ` 上下文 / `+` 新增 / `-` 删除。首块允许无 @@ 头。
        if chunks.is_empty() {
            chunks.push(UpdateFileChunk::default());
        }
        let chunk = chunks.last_mut().expect("chunks 刚被确保非空");
        if let Some(text) = line.strip_prefix(' ') {
            chunk.push_context_line(text.to_string());
        } else if let Some(text) = line.strip_prefix('+') {
            chunk.new_lines.push(text.to_string());
        } else if let Some(text) = line.strip_prefix('-') {
            chunk.old_lines.push(text.to_string());
        } else if line.is_empty() {
            // 空行按空上下文行处理（宽松：严格文法要求前导空格）。
            chunk.push_context_line(String::new());
        } else {
            return Err(ParseError::InvalidHunkError {
                message: format!("unexpected line in update hunk: {line}"),
                line_number: index + 1,
            });
        }
        index += 1;
    }

    if chunks.is_empty() {
        return Err(ParseError::InvalidHunkError {
            message: format!("Update file hunk for path '{path}' is empty"),
            line_number: index,
        });
    }
    Ok((
        Hunk::UpdateFile {
            path: PathBuf::from(path),
            move_path,
            chunks,
        },
        index,
    ))
}

// ── 模糊定位（同 codex seek_sequence.rs）──────────────────────────────

/// 在 `lines` 中从 `start` 起查找 `pattern` 连续行的起始下标；逐级放宽：
/// 精确 → 忽略行尾空白 → 双侧 trim → 常见 Unicode 标点归一化。
/// `eof=true` 时先尝试锚定文件末尾，失败再从头搜索。
pub(crate) fn seek_sequence(
    lines: &[String],
    pattern: &[String],
    start: usize,
    eof: bool,
) -> Option<usize> {
    if pattern.is_empty() {
        return Some(start);
    }
    if pattern.len() > lines.len() {
        return None;
    }
    let search_start = if eof && lines.len() >= pattern.len() {
        (lines.len() - pattern.len()).max(start)
    } else {
        start
    };
    let last = lines.len() - pattern.len();
    for i in search_start..=last {
        if lines[i..i + pattern.len()] == *pattern {
            return Some(i);
        }
    }
    for i in search_start..=last {
        if lines[i..i + pattern.len()]
            .iter()
            .zip(pattern)
            .all(|(line, pat)| line.trim_end() == pat.trim_end())
        {
            return Some(i);
        }
    }
    for i in search_start..=last {
        if lines[i..i + pattern.len()]
            .iter()
            .zip(pattern)
            .all(|(line, pat)| line.trim() == pat.trim())
        {
            return Some(i);
        }
    }
    for i in search_start..=last {
        if lines[i..i + pattern.len()]
            .iter()
            .zip(pattern)
            .all(|(line, pat)| normalize_punctuation(line) == normalize_punctuation(pat))
        {
            return Some(i);
        }
    }
    None
}

/// 常见 Unicode 排版标点归一化为 ASCII 等价物（mirrors git apply 的宽容）：
/// 各类破折/连字符 → `-`，弯引号 → 直引号，特殊空格 → 普通空格。
fn normalize_punctuation(value: &str) -> String {
    value
        .trim()
        .chars()
        .map(|c| match c {
            '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2015}'
            | '\u{2212}' => '-',
            '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => '\'',
            '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' => '"',
            '\u{00A0}' | '\u{2002}' | '\u{2003}' | '\u{2004}' | '\u{2005}' | '\u{2006}'
            | '\u{2007}' | '\u{2008}' | '\u{2009}' | '\u{200A}' | '\u{202F}' | '\u{205F}'
            | '\u{3000}' => ' ',
            other => other,
        })
        .collect()
}

// ── 行尾保真的文件模型（同 codex text_file.rs）───────────────────────

#[derive(Clone, Copy, PartialEq)]
enum LineEnding {
    Lf,
    CrLf,
    Cr,
}

impl LineEnding {
    fn as_str(self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::CrLf => "\r\n",
            LineEnding::Cr => "\r",
        }
    }
}

struct SourceLine {
    text: String,
    ending: Option<LineEnding>,
}

pub(super) struct SourceFile {
    lines: Vec<SourceLine>,
    preferred_ending: LineEnding,
}

type Replacement = (usize, usize, Vec<String>);

impl SourceFile {
    /// 按行切分并保留每行的行尾；首个行尾成为插入行的首选风格。
    pub(super) fn parse(contents: &str) -> Self {
        let mut lines = Vec::new();
        let mut preferred_ending = None;
        let mut line_start = 0;
        let mut cursor = 0;
        let bytes = contents.as_bytes();
        while cursor < contents.len() {
            let (ending, ending_len) = match bytes[cursor] {
                b'\r' if bytes.get(cursor + 1) == Some(&b'\n') => (LineEnding::CrLf, 2),
                b'\r' => (LineEnding::Cr, 1),
                b'\n' => (LineEnding::Lf, 1),
                _ => {
                    cursor += 1;
                    continue;
                }
            };
            preferred_ending.get_or_insert(ending);
            lines.push(SourceLine {
                text: contents[line_start..cursor].to_string(),
                ending: Some(ending),
            });
            cursor += ending_len;
            line_start = cursor;
        }
        if line_start < contents.len() {
            lines.push(SourceLine {
                text: contents[line_start..].to_string(),
                ending: None,
            });
        }
        Self {
            lines,
            preferred_ending: preferred_ending.unwrap_or(LineEnding::Lf),
        }
    }

    pub(super) fn line_texts(&self) -> Vec<String> {
        self.lines.iter().map(|line| line.text.clone()).collect()
    }

    /// 应用按源序排列、互不重叠的替换：未触及行保留原行尾，插入行用首选
    /// 行尾，所有行最终都有行尾（对齐 apply-patch 历史上的末尾换行行为）。
    pub(super) fn apply_replacements(&mut self, replacements: &[Replacement]) {
        let mut source_lines = std::mem::take(&mut self.lines).into_iter();
        let mut new_lines = Vec::new();
        let mut source_index = 0;
        let preferred = self.preferred_ending;
        for (start_idx, old_len, new_segment) in replacements {
            let (start_idx, old_len) = (*start_idx, *old_len);
            for line in source_lines
                .by_ref()
                .take(start_idx.saturating_sub(source_index))
            {
                new_lines.push(line);
            }
            for _ in source_lines.by_ref().take(old_len) {}
            new_lines.extend(new_segment.iter().map(|text| SourceLine {
                text: text.clone(),
                ending: Some(preferred),
            }));
            source_index = start_idx + old_len;
        }
        new_lines.extend(source_lines);
        self.lines = new_lines;
        for line in &mut self.lines {
            line.ending.get_or_insert(preferred);
        }
    }

    pub(super) fn into_contents(self) -> String {
        let mut contents = String::new();
        for line in self.lines {
            contents.push_str(&line.text);
            if let Some(ending) = line.ending {
                contents.push_str(ending.as_str());
            }
        }
        contents
    }
}

// ── 替换计算（同 codex file_update.rs::compute_replacements）─────────

/// 计算把 `original_lines` 变换为目标内容所需的替换列表，
/// 每项为 `(起始行, 旧行数, 新行列表)`。
fn compute_replacements(
    original_lines: &[String],
    path: &str,
    chunks: &[UpdateFileChunk],
) -> Result<Vec<Replacement>, String> {
    let mut replacements: Vec<Replacement> = Vec::new();
    let mut line_index: usize = 0;

    for chunk in chunks {
        if let Some(ctx_line) = &chunk.change_context {
            if let Some(idx) = seek_sequence(
                original_lines,
                std::slice::from_ref(ctx_line),
                line_index,
                false,
            ) {
                line_index = idx + 1;
            } else {
                return Err(format!("Failed to find context '{ctx_line}' in {path}"));
            }
        }

        if chunk.old_lines.is_empty() {
            replacements.push((original_lines.len(), 0, chunk.new_lines.clone()));
            continue;
        }

        // 尾部空行哨兵：真实 diff 中 old_lines 末尾的空串代表被替换区域的
        // 终止换行；SourceFile 把行尾挂在上一行而非独立元素，直接搜索失败
        // 时去掉末尾空串重试（new_lines 同步去掉）。
        let mut pattern: &[String] = &chunk.old_lines;
        let mut found = seek_sequence(original_lines, pattern, line_index, chunk.is_end_of_file);
        let mut new_slice: &[String] = &chunk.new_lines;

        if found.is_none() && pattern.last().is_some_and(String::is_empty) {
            pattern = &pattern[..pattern.len() - 1];
            if new_slice.last().is_some_and(String::is_empty) {
                new_slice = &new_slice[..new_slice.len() - 1];
            }
            found = seek_sequence(original_lines, pattern, line_index, chunk.is_end_of_file);
        }

        if let Some(start_idx) = found {
            // 上下文行双侧都在：保留文件原文原行尾，只在非上下文段做替换。
            let mut old_start = 0;
            let mut new_start = 0;
            for &(old_context, new_context) in &chunk.context_line_indices {
                if old_context >= pattern.len() || new_context >= new_slice.len() {
                    break;
                }
                if old_start != old_context || new_start != new_context {
                    replacements.push((
                        start_idx + old_start,
                        old_context - old_start,
                        new_slice[new_start..new_context].to_vec(),
                    ));
                }
                old_start = old_context + 1;
                new_start = new_context + 1;
            }
            if old_start != pattern.len() || new_start != new_slice.len() {
                replacements.push((
                    start_idx + old_start,
                    pattern.len() - old_start,
                    new_slice[new_start..].to_vec(),
                ));
            }
            line_index = start_idx + pattern.len();
        } else {
            return Err(format!(
                "Failed to find expected lines in {}:\n{}",
                path,
                chunk.old_lines.join("\n"),
            ));
        }
    }

    replacements.sort_by_key(|(index, _, _)| *index);
    Ok(replacements)
}

/// 由补丁块推导文件新内容（不写盘）。路径仅为错误信息展示用。
pub(super) fn derive_new_contents(
    original_contents: &str,
    path: &str,
    chunks: &[UpdateFileChunk],
) -> Result<String, String> {
    let mut source_file = SourceFile::parse(original_contents);
    let original_lines = source_file.line_texts();
    let replacements = compute_replacements(&original_lines, path, chunks)?;
    source_file.apply_replacements(&replacements);
    Ok(source_file.into_contents())
}

#[cfg(test)]
#[path = "engine_tests.rs"]
mod tests;
