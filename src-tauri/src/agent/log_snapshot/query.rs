use serde::{Deserialize, Serialize};

use super::model::{SnapshotFile, UpdateRecord};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MatchMode {
    Fixed,
    Regex,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OutputMode {
    Files,
    Count,
    Matches,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SearchEngine {
    Auto,
    Scan,
}

#[derive(Clone, Debug)]
pub(crate) struct LogSearchRequest {
    pub snapshot_id: String,
    pub patterns: Vec<String>,
    pub match_mode: MatchMode,
    pub case_sensitive: bool,
    pub output_mode: OutputMode,
    pub engine: SearchEngine,
    pub file_ids: Vec<u64>,
    pub context_before: usize,
    pub context_after: usize,
    pub max_matches: usize,
    pub max_files: usize,
    pub max_line_chars: usize,
    pub cursor: Option<String>,
}

impl LogSearchRequest {
    pub(crate) fn validate(&self) -> Result<(), String> {
        super::model::validate_snapshot_id(&self.snapshot_id)?;
        if self.patterns.is_empty()
            || self.patterns.len() > 4
            || self.patterns.iter().any(|pattern| pattern.is_empty())
        {
            return Err("patterns 必须包含 1 到 4 个非空模式".into());
        }
        if self
            .patterns
            .iter()
            .any(|pattern| pattern.len() > 4096 || pattern.contains('\0'))
        {
            return Err("单个搜索模式不能超过 4096 字节且不能包含 NUL".into());
        }
        if !(1..=200).contains(&self.max_matches) || !(1..=200).contains(&self.max_files) {
            return Err("max_matches 和 max_files 必须在 1 到 200 之间".into());
        }
        if self.context_before > 20 || self.context_after > 20 {
            return Err("前后上下文行数不能超过 20".into());
        }
        if !(32..=20_000).contains(&self.max_line_chars) {
            return Err("max_line_chars 必须在 32 到 20000 之间".into());
        }
        if self.file_ids.len() > super::model::MAX_SOURCES || self.file_ids.contains(&0) {
            return Err("file_ids 数量或内容不合法".into());
        }
        if self.cursor.is_some() && self.output_mode != OutputMode::Matches {
            return Err("cursor 仅支持 output_mode=matches".into());
        }
        Ok(())
    }

    pub(crate) fn can_use_index(&self) -> bool {
        self.engine == SearchEngine::Auto
            && self.match_mode == MatchMode::Fixed
            && self
                .patterns
                .iter()
                .all(|pattern| pattern.chars().count() >= 3)
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LogLine {
    pub file_id: u64,
    pub line_number: u64,
    pub byte_offset: u64,
    pub byte_length: u64,
    pub text: String,
    pub text_truncated: bool,
    pub context: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LogMatch {
    #[serde(flatten)]
    pub line: LogLine,
    pub before: Vec<LogLine>,
    pub after: Vec<LogLine>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FileMatchCount {
    pub file_id: u64,
    pub source_label: String,
    pub match_count: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LogSearchResult {
    pub snapshot_id: String,
    pub generation: u64,
    pub engine: String,
    pub output_mode: OutputMode,
    // 完整性标志与续页游标放在主体（matches/files）之前：结果被字符级截断时
    // 保住尾部会丢失的分页信息（工具结果按字段顺序序列化）。
    pub scan_complete: bool,
    pub results_truncated: bool,
    pub next_cursor: Option<String>,
    pub warnings: Vec<String>,
    pub total_match_lines: Option<u64>,
    pub matches: Vec<LogMatch>,
    pub files: Vec<FileMatchCount>,
    pub update: Option<UpdateRecord>,
}

impl LogSearchResult {
    pub(crate) fn empty(
        snapshot_id: String,
        generation: u64,
        engine: String,
        output_mode: OutputMode,
    ) -> Self {
        Self {
            snapshot_id,
            generation,
            engine,
            output_mode,
            matches: Vec::new(),
            files: Vec::new(),
            total_match_lines: None,
            scan_complete: true,
            results_truncated: false,
            next_cursor: None,
            warnings: Vec::new(),
            update: None,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct LogReadRequest {
    pub snapshot_id: String,
    pub file_id: u64,
    pub line_number: Option<u64>,
    pub before: usize,
    pub after: usize,
    pub byte_offset: Option<u64>,
    pub max_bytes: usize,
    pub max_line_chars: usize,
}

impl LogReadRequest {
    pub(crate) fn validate(&self) -> Result<(), String> {
        super::model::validate_snapshot_id(&self.snapshot_id)?;
        if self.file_id == 0 {
            return Err("file_id 必须大于 0".into());
        }
        if self.before > 100 || self.after > 499 || self.before + self.after + 1 > 500 {
            return Err("单次最多读取 500 行，before 最大 100、after 最大 499".into());
        }
        if !(1024..=1024 * 1024).contains(&self.max_bytes) {
            return Err("max_bytes 必须在 1024 到 1048576 之间".into());
        }
        if !(32..=20_000).contains(&self.max_line_chars) {
            return Err("max_line_chars 必须在 32 到 20000 之间".into());
        }
        if self.line_number == Some(0) || self.byte_offset.is_some() && self.line_number.is_some() {
            return Err("line_number 必须从 1 开始，且不能与 byte_offset 同时指定".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LogReadResult {
    pub snapshot_id: String,
    pub generation: u64,
    // 分页游标与截断标志先于正文（lines/text）：理由同 LogSearchResult。
    pub next_line: Option<u64>,
    pub next_byte_offset: Option<u64>,
    pub truncated: bool,
    pub file: SnapshotFile,
    pub lines: Vec<LogLine>,
    pub text: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct LoadedGeneration {
    pub manifest: super::model::GenerationManifest,
    pub root: std::path::PathBuf,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct CursorPosition {
    /// 游标所属快照：不同快照的 generation 各自从 1 递增，不校验会把 A 的
    /// 游标误用到同代次的 B 上，导致命中被静默跳过。
    #[serde(default)]
    pub snapshot_id: String,
    pub generation: u64,
    pub query_hash: String,
    pub row_id: u64,
    #[serde(default)]
    pub file_id: Option<u64>,
    #[serde(default)]
    pub line_number: Option<u64>,
}

pub(crate) fn encode_cursor(position: &CursorPosition) -> Result<String, String> {
    use base64::Engine;
    let bytes = serde_json::to_vec(position).map_err(|error| error.to_string())?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

pub(crate) fn decode_cursor(raw: &str) -> Result<CursorPosition, String> {
    use base64::Engine;
    if raw.len() > 1024 {
        return Err("cursor 过长".into());
    }
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(raw)
        .map_err(|_| "cursor 无法解码".to_string())?;
    serde_json::from_slice(&bytes).map_err(|_| "cursor 格式无效".to_string())
}

pub(crate) fn query_hash(request: &LogSearchRequest) -> String {
    use sha2::{Digest, Sha256};
    let value = (
        &request.patterns,
        request.match_mode,
        request.case_sensitive,
        request.output_mode,
        request.engine,
        &request.file_ids,
    );
    let digest = Sha256::digest(serde_json::to_vec(&value).unwrap_or_default());
    digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
