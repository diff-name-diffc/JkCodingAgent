use std::collections::HashSet;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Utc;
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{params, types::Type, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::config::DEFAULT_SUMMARY_MODEL;
use super::llm::{ChatMessage, LlmUsage, OutboundToolCall};
use super::summary::ToolArtifactDraft;

const MAX_LLM_DIALOGUES: usize = 5;
const MAX_DIALOGUE_QUERY_LIMIT: usize = 50;
const DEFAULT_CONTEXT_WINDOW_CAPACITY_TOKENS: u64 = 1_000_000;
const DEFAULT_IMAGE_MODEL_URL: &str = "https://dashscope.aliyuncs.com/api/v1";
const DEFAULT_IMAGE_MODEL: &str = "qwen-image-2.0-pro";
const DEFAULT_ASR_MODEL: &str = "fun-asr-realtime";
pub(crate) const TOOL_RETRY_CONTEXT_PREFIX: &str = "[工具调用失败，已交回模型修正重试]";

// ── Content Segments ──────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ContentSegment {
    Text {
        id: String,
        text: String,
    },
    Image {
        id: String,
        image_id: String,
        path: String,
        alt: Option<String>,
        width: Option<u32>,
        height: Option<u32>,
        mime_type: Option<String>,
        source: String,
        generation_prompt: Option<String>,
    },
    File {
        id: String,
        file_id: String,
        path: String,
        file_name: String,
        mime_type: String,
        size: u64,
    },
}

impl ContentSegment {
    pub fn to_markdown(&self) -> String {
        match self {
            ContentSegment::Text { text, .. } => text.clone(),
            ContentSegment::Image {
                alt,
                // `path` is intentionally not included in the rendered markdown
                // output so that raw filesystem paths never appear in the
                // content visible to the user or sent to the LLM.
                image_id,
                ..
            } => {
                format!(
                    "![{}]({})",
                    alt.as_deref().unwrap_or("image"),
                    format!("chat-image://{}", image_id)
                )
            }
            ContentSegment::File { .. } => String::new(),
        }
    }
}

pub fn segments_to_markdown(segments: &[ContentSegment]) -> String {
    segments
        .iter()
        .map(|s| s.to_markdown())
        .collect::<Vec<_>>()
        .join("\n")
}

fn content_to_segments_json(content: &str) -> String {
    let segments = vec![ContentSegment::Text {
        id: Uuid::new_v4().to_string(),
        text: content.to_string(),
    }];
    serde_json::to_string(&segments).unwrap_or_else(|_| "[]".to_string())
}

fn parse_segments_json(segments_json: &str) -> Vec<ContentSegment> {
    match serde_json::from_str(segments_json) {
        Ok(segments) => segments,
        Err(e) => {
            let preview = if segments_json.len() > 500 {
                format!("{}...(truncated, {} bytes)", &segments_json[..500], segments_json.len())
            } else {
                segments_json.to_string()
            };
            eprintln!("parse_segments_json failed: {e}\n  input: {preview}");
            Vec::new()
        }
    }
}

fn safe_absolute_image_path(path: &str) -> Result<PathBuf> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        anyhow::bail!("chat image path is empty");
    }

    let path = Path::new(trimmed);
    if !path.is_absolute() {
        anyhow::bail!("chat image path must be absolute: {trimmed}");
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        anyhow::bail!("chat image path must not contain parent traversal: {trimmed}");
    }

    Ok(path.to_path_buf())
}

fn safe_absolute_plan_path(path: &str) -> Result<PathBuf> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        anyhow::bail!("plan path is empty");
    }

    let path = Path::new(trimmed);
    if !path.is_absolute() {
        anyhow::bail!("plan path must be absolute: {trimmed}");
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        anyhow::bail!("plan path must not contain parent traversal: {trimmed}");
    }
    if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
        anyhow::bail!("plan path must be a Markdown file: {trimmed}");
    }

    Ok(path.to_path_buf())
}

fn insert_chat_images(
    tx: &rusqlite::Transaction<'_>,
    workspace_id: &str,
    message_id: &str,
    segments: &[ContentSegment],
    created_at: &str,
) -> Result<()> {
    for (index, segment) in segments.iter().enumerate() {
        let ContentSegment::Image {
            image_id,
            path,
            alt,
            width,
            height,
            mime_type,
            source,
            generation_prompt,
            ..
        } = segment
        else {
            continue;
        };

        let safe_path = safe_absolute_image_path(path)?;
        tx.execute(
            "INSERT INTO chat_images (
                id, image_id, workspace_id, message_id, segment_index, path, alt, width, height, mime_type, source, generation_prompt, created_at
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
             ON CONFLICT(image_id) DO UPDATE SET
                workspace_id = excluded.workspace_id,
                message_id = excluded.message_id,
                segment_index = excluded.segment_index,
                path = excluded.path,
                alt = excluded.alt,
                width = excluded.width,
                height = excluded.height,
                mime_type = excluded.mime_type,
                source = excluded.source,
                generation_prompt = excluded.generation_prompt,
                created_at = excluded.created_at",
            params![
                Uuid::new_v4().to_string(),
                image_id,
                workspace_id,
                message_id,
                index as i64,
                safe_path.to_string_lossy().as_ref(),
                alt,
                width.map(i64::from),
                height.map(i64::from),
                mime_type,
                source,
                generation_prompt,
                created_at,
            ],
        )
        .context("insert chat image")?;
    }

    Ok(())
}

fn delete_chat_image_resources(tx: &rusqlite::Transaction<'_>, workspace_id: &str) -> Result<()> {
    let mut paths = HashSet::new();
    {
        let mut stmt = tx
            .prepare("SELECT path FROM chat_images WHERE workspace_id = ?1")
            .context("load chat image paths")?;
        let indexed_paths = stmt
            .query_map(params![workspace_id], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("collect chat image paths")?;
        paths.extend(indexed_paths);
    }
    {
        let mut stmt = tx
            .prepare("SELECT segments_json FROM dispatcher_messages WHERE workspace_id = ?1")
            .context("load dispatcher message segments for image cleanup")?;
        let segments_json = stmt
            .query_map(params![workspace_id], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("collect dispatcher message segments for image cleanup")?;
        for json in segments_json {
            for segment in parse_segments_json(&json) {
                if let ContentSegment::Image { path, .. } = segment {
                    paths.insert(path);
                }
            }
        }
    }

    for path in paths {
        let safe_path = safe_absolute_image_path(&path)?;
        match std::fs::remove_file(&safe_path) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("remove chat image {}", safe_path.display()));
            }
        }
    }

    tx.execute(
        "DELETE FROM chat_images WHERE workspace_id = ?1",
        params![workspace_id],
    )
    .context("delete chat image records")?;

    Ok(())
}

fn delete_plan_file_resources(tx: &rusqlite::Transaction<'_>, workspace_id: &str) -> Result<()> {
    let mut paths = HashSet::new();

    let mut stmt = tx
        .prepare("SELECT active_plan_path FROM dispatcher_sessions WHERE id = ?1 AND active_plan_path IS NOT NULL")
        .context("load dispatcher session plan path")?;
    let dispatcher_paths = stmt
        .query_map(params![workspace_id], |row| row.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("collect dispatcher plan paths")?;
    paths.extend(dispatcher_paths);

    let mut stmt = tx
        .prepare("SELECT active_plan_path FROM project_sessions WHERE id = ?1 AND active_plan_path IS NOT NULL")
        .context("load project session plan path")?;
    let project_paths = stmt
        .query_map(params![workspace_id], |row| row.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("collect project plan paths")?;
    paths.extend(project_paths);

    for path in paths {
        let safe_path = match safe_absolute_plan_path(&path) {
            Ok(p) => p,
            Err(error) => {
                eprintln!("skip invalid plan path: {error}");
                continue;
            }
        };
        match std::fs::remove_file(&safe_path) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => {
                eprintln!("remove plan file {} failed: {error}", safe_path.display());
            }
        }
    }

    Ok(())
}

// ── Records ─────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherSessionRecord {
    pub id: String,
    pub project_id: String,
    pub kind: DispatcherSessionKind,
    pub title: String,
    pub mode: DispatcherMode,
    pub active_plan_path: Option<String>,
    pub category: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatSessionRecord {
    pub id: String,
    pub title: String,
    pub category: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSessionRecord {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub mode: DispatcherMode,
    pub active_plan_path: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPage<T> {
    pub items: Vec<T>,
    pub total: i64,
    pub has_more: bool,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatCategory {
    pub id: String,
    pub name: String,
    pub icon: String,
    pub color: String,
    pub sort_order: i32,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherModelConfig {
    pub url: String,
    pub api_key: String,
    pub model: String,
    #[serde(default = "default_model_config_active")]
    pub active: bool,
}

fn default_model_config_active() -> bool {
    true
}

impl DispatcherModelConfig {
    pub fn new(url: &str, api_key: &str, model: &str) -> Self {
        Self {
            url: url.trim().to_string(),
            api_key: api_key.trim().to_string(),
            model: model.trim().to_string(),
            active: true,
        }
    }

    fn trimmed(self) -> Self {
        Self {
            url: self.url.trim().to_string(),
            api_key: self.api_key.trim().to_string(),
            model: self.model.trim().to_string(),
            active: self.active,
        }
    }

    fn is_empty(&self) -> bool {
        self.url.trim().is_empty() && self.api_key.trim().is_empty() && self.model.trim().is_empty()
    }
}

#[derive(Debug, Clone, Default)]
pub struct DispatcherSettingsModelConfigs {
    pub chat_model_config: Option<DispatcherModelConfig>,
    pub summary_model_config: Option<DispatcherModelConfig>,
    pub vision_model_config: Option<DispatcherModelConfig>,
    pub image_model_config: Option<DispatcherModelConfig>,
    pub image_edit_model_config: Option<DispatcherModelConfig>,
    pub asr_model_config: Option<DispatcherModelConfig>,
    pub tts_model_config: Option<DispatcherModelConfig>,
    pub embedding_model_config: Option<DispatcherModelConfig>,
    pub chat_model_configs: Option<Vec<DispatcherModelConfig>>,
    pub summary_model_configs: Option<Vec<DispatcherModelConfig>>,
    pub vision_model_configs: Option<Vec<DispatcherModelConfig>>,
    pub image_model_configs: Option<Vec<DispatcherModelConfig>>,
    pub image_edit_model_configs: Option<Vec<DispatcherModelConfig>>,
    pub asr_model_configs: Option<Vec<DispatcherModelConfig>>,
    pub tts_model_configs: Option<Vec<DispatcherModelConfig>>,
    pub embedding_model_configs: Option<Vec<DispatcherModelConfig>>,
}

struct DispatcherSettingsConfigLists {
    chat_model_configs: Vec<DispatcherModelConfig>,
    summary_model_configs: Vec<DispatcherModelConfig>,
    vision_model_configs: Vec<DispatcherModelConfig>,
    image_model_configs: Vec<DispatcherModelConfig>,
    image_edit_model_configs: Vec<DispatcherModelConfig>,
    asr_model_configs: Vec<DispatcherModelConfig>,
    tts_model_configs: Vec<DispatcherModelConfig>,
    embedding_model_configs: Vec<DispatcherModelConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherSettingsRecord {
    pub api_base: String,
    pub api_key: String,
    pub model: String,
    pub summary_model: String,
    pub vision_model: String,
    pub asr_api_key: String,
    pub asr_websocket_url: String,
    pub auto_approve_dispatch: bool,
    pub context_debug: bool,
    pub image_model_url: String,
    pub image_model_api_key: String,
    pub image_model: String,
    pub image_edit_model: String,
    pub chat_model_config: DispatcherModelConfig,
    pub summary_model_config: DispatcherModelConfig,
    pub vision_model_config: DispatcherModelConfig,
    pub image_model_config: DispatcherModelConfig,
    pub image_edit_model_config: DispatcherModelConfig,
    pub asr_model_config: DispatcherModelConfig,
    pub tts_model_config: DispatcherModelConfig,
    pub embedding_model_config: DispatcherModelConfig,
    pub chat_model_configs: Vec<DispatcherModelConfig>,
    pub summary_model_configs: Vec<DispatcherModelConfig>,
    pub vision_model_configs: Vec<DispatcherModelConfig>,
    pub image_model_configs: Vec<DispatcherModelConfig>,
    pub image_edit_model_configs: Vec<DispatcherModelConfig>,
    pub asr_model_configs: Vec<DispatcherModelConfig>,
    pub tts_model_configs: Vec<DispatcherModelConfig>,
    pub embedding_model_configs: Vec<DispatcherModelConfig>,
}

// ── Model Slot ─────────────────────────────────────────────────
//
// Each model type (chat, summary, vision, …) follows the same pattern:
//   - DB columns: (url, api_key, name) triple + JSON list column
//   - Read fallback: JSON list → structured triple → legacy flat fields
//   - Write: derive all three layers from the canonical list
//
// `ModelSlot` encodes the per-slot metadata so the read/write paths can
// iterate instead of copy-pasting 8×.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub(crate) enum ModelSlot {
    Chat,
    Summary,
    Vision,
    Image,
    ImageEdit,
    Asr,
    Tts,
    Embedding,
}

#[derive(Debug, Clone, Copy)]
struct ModelSlotColumns {
    url: &'static str,
    api_key: &'static str,
    model: &'static str,
    json: &'static str,
}

const SETTINGS_LEGACY_COLUMNS: [&str; 13] = [
    "api_base",
    "api_key",
    "model",
    "summary_model",
    "vision_model",
    "asr_api_key",
    "asr_websocket_url",
    "auto_approve_dispatch",
    "context_debug",
    "image_model_url",
    "image_model_api_key",
    "image_model",
    "image_edit_model",
];

const MODEL_SLOT_COLUMNS: [ModelSlotColumns; 8] = [
    ModelSlotColumns {
        url: "chat_model_url",
        api_key: "chat_model_api_key",
        model: "chat_model_name",
        json: "chat_model_configs_json",
    },
    ModelSlotColumns {
        url: "summary_model_url",
        api_key: "summary_model_api_key",
        model: "summary_model_name",
        json: "summary_model_configs_json",
    },
    ModelSlotColumns {
        url: "vision_model_url",
        api_key: "vision_model_api_key",
        model: "vision_model_name",
        json: "vision_model_configs_json",
    },
    ModelSlotColumns {
        url: "image_model_config_url",
        api_key: "image_model_config_api_key",
        model: "image_model_config_name",
        json: "image_model_configs_json",
    },
    ModelSlotColumns {
        url: "image_edit_model_url",
        api_key: "image_edit_model_api_key",
        model: "image_edit_model_name",
        json: "image_edit_model_configs_json",
    },
    ModelSlotColumns {
        url: "asr_model_url",
        api_key: "asr_model_api_key",
        model: "asr_model_name",
        json: "asr_model_configs_json",
    },
    ModelSlotColumns {
        url: "tts_model_url",
        api_key: "tts_model_api_key",
        model: "tts_model_name",
        json: "tts_model_configs_json",
    },
    ModelSlotColumns {
        url: "embedding_model_url",
        api_key: "embedding_model_api_key",
        model: "embedding_model_name",
        json: "embedding_model_configs_json",
    },
];

impl ModelSlot {
    const ALL: [ModelSlot; 8] = [
        ModelSlot::Chat,
        ModelSlot::Summary,
        ModelSlot::Vision,
        ModelSlot::Image,
        ModelSlot::ImageEdit,
        ModelSlot::Asr,
        ModelSlot::Tts,
        ModelSlot::Embedding,
    ];

    #[allow(dead_code)]
    fn label(self) -> &'static str {
        match self {
            ModelSlot::Chat => "chat",
            ModelSlot::Summary => "summary",
            ModelSlot::Vision => "vision",
            ModelSlot::Image => "image",
            ModelSlot::ImageEdit => "image edit",
            ModelSlot::Asr => "asr",
            ModelSlot::Tts => "tts",
            ModelSlot::Embedding => "embedding",
        }
    }

    fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|slot| *slot == self)
            .expect("model slot must be listed in ModelSlot::ALL")
    }

    fn columns(self) -> &'static ModelSlotColumns {
        &MODEL_SLOT_COLUMNS[self.index()]
    }

    /// 0-based column index where the (url, api_key, name) triple starts
    /// in the generated settings SELECT.
    fn config_col_offset(self) -> usize {
        SETTINGS_LEGACY_COLUMNS.len() + self.index() * 3
    }

    /// 0-based column index for the JSON list column.
    fn json_col(self) -> usize {
        SETTINGS_LEGACY_COLUMNS.len() + MODEL_SLOT_COLUMNS.len() * 3 + self.index()
    }

    /// For `get_settings`: legacy columns are the same chat base (0-2) for
    /// Chat/Summary/Vision; image legacy (9-11) for Image/ImageEdit;
    /// asr legacy (5-6) for Asr; and no legacy fallback for TTS/Embedding.
    fn legacy_fallback(self, row: &rusqlite::Row<'_>) -> rusqlite::Result<DispatcherModelConfig> {
        let (url_col, key_col, model): (usize, usize, String) = match self {
            ModelSlot::Chat => (0, 1, row.get(2)?),
            ModelSlot::Summary => (0, 1, row.get(3)?),
            ModelSlot::Vision => (0, 1, row.get(4)?),
            ModelSlot::Image => (9, 10, row.get(11)?),
            ModelSlot::ImageEdit => {
                let image_model: String = row.get(11)?;
                let image_edit_model: String = row.get(12)?;
                (
                    9,
                    10,
                    fallback_image_edit_model(&image_model, &image_edit_model).to_string(),
                )
            }
            ModelSlot::Asr => (6, 5, DEFAULT_ASR_MODEL.to_string()),
            ModelSlot::Tts | ModelSlot::Embedding => {
                return Ok(DispatcherModelConfig::default());
            }
        };
        let url: String = row.get(url_col)?;
        let key: String = row.get(key_col)?;
        Ok(DispatcherModelConfig::new(&url, &key, &model))
    }

    /// Apply per-slot defaults (url, api_key, model) when the active config
    /// has empty fields.  Returns the (possibly modified) config.
    fn apply_defaults(
        self,
        config: &mut DispatcherModelConfig,
        image_config: &DispatcherModelConfig,
    ) {
        match self {
            ModelSlot::Summary => {
                if !config.is_empty() {
                    config.model = normalize_summary_model(&config.model);
                }
            }
            ModelSlot::Image => {
                if !config.is_empty() && config.url.is_empty() {
                    config.url = DEFAULT_IMAGE_MODEL_URL.to_string();
                }
                if !config.is_empty() && config.model.is_empty() {
                    config.model = DEFAULT_IMAGE_MODEL.to_string();
                }
            }
            ModelSlot::ImageEdit => {
                if !config.is_empty() && config.url.is_empty() {
                    config.url = if image_config.url.is_empty() {
                        DEFAULT_IMAGE_MODEL_URL.to_string()
                    } else {
                        image_config.url.clone()
                    };
                }
                if !config.is_empty() && config.api_key.is_empty() {
                    config.api_key = image_config.api_key.clone();
                }
                if !config.is_empty() && config.model.is_empty() {
                    config.model = if image_config.model.is_empty() {
                        DEFAULT_IMAGE_MODEL.to_string()
                    } else {
                        image_config.model.clone()
                    };
                }
            }
            ModelSlot::Asr => {
                if !config.is_empty() && config.model.is_empty() {
                    config.model = DEFAULT_ASR_MODEL.to_string();
                }
            }
            _ => {}
        }
    }
}

fn settings_select_columns() -> Vec<&'static str> {
    let mut columns =
        Vec::with_capacity(SETTINGS_LEGACY_COLUMNS.len() + MODEL_SLOT_COLUMNS.len() * 4);
    columns.extend(SETTINGS_LEGACY_COLUMNS);
    for slot in ModelSlot::ALL {
        let slot_columns = slot.columns();
        columns.push(slot_columns.url);
        columns.push(slot_columns.api_key);
        columns.push(slot_columns.model);
    }
    for slot in ModelSlot::ALL {
        columns.push(slot.columns().json);
    }
    columns
}

fn settings_column_index(name: &str) -> usize {
    settings_select_columns()
        .iter()
        .position(|column| *column == name)
        .unwrap_or_else(|| panic!("settings column must exist: {name}"))
}

fn settings_select_sql() -> String {
    format!(
        "SELECT {} FROM dispatcher_settings WHERE id = 'default'",
        settings_select_columns().join(", ")
    )
}

fn settings_upsert_sql() -> String {
    let columns = settings_select_columns();
    let placeholders = (1..=columns.len())
        .map(|index| format!("?{index}"))
        .collect::<Vec<_>>()
        .join(", ");
    let updates = columns
        .iter()
        .enumerate()
        .map(|(index, column)| format!("{column} = ?{}", index + 1))
        .collect::<Vec<_>>()
        .join(", ");

    format!(
        "INSERT INTO dispatcher_settings (id, {})
         VALUES ('default', {placeholders})
         ON CONFLICT(id) DO UPDATE SET {updates}",
        columns.join(", ")
    )
}

fn fallback_image_edit_model<'a>(image_model: &'a str, image_edit_model: &'a str) -> &'a str {
    let trimmed = image_edit_model.trim();
    if trimmed.is_empty() {
        image_model.trim()
    } else {
        trimmed
    }
}

fn normalize_model_configs(configs: Vec<DispatcherModelConfig>) -> Vec<DispatcherModelConfig> {
    let mut normalized = configs
        .into_iter()
        .map(DispatcherModelConfig::trimmed)
        .filter(|config| !config.is_empty())
        .collect::<Vec<_>>();

    if let Some(active_index) = normalized.iter().position(|config| config.active) {
        for (index, config) in normalized.iter_mut().enumerate() {
            config.active = index == active_index;
        }
    } else if let Some(first) = normalized.first_mut() {
        first.active = true;
    }
    normalized
}

fn active_config(configs: &[DispatcherModelConfig]) -> Option<DispatcherModelConfig> {
    configs.iter().find(|config| config.active).cloned()
}

/// Read-path: merge JSON list column, structured config triple, and legacy
/// fallback into a single canonical list for the slot.
fn resolve_slot_configs_from_row(
    row: &rusqlite::Row<'_>,
    slot: ModelSlot,
) -> rusqlite::Result<Vec<DispatcherModelConfig>> {
    let col = slot.config_col_offset();
    let structured = DispatcherModelConfig::new(
        &row.get::<_, String>(col)?,
        &row.get::<_, String>(col + 1)?,
        &row.get::<_, String>(col + 2)?,
    );
    let legacy = slot.legacy_fallback(row)?;
    let merged = model_config_or_legacy(
        structured.url,
        structured.api_key,
        structured.model,
        &legacy.url,
        &legacy.api_key,
        &legacy.model,
    );
    let json_col = slot.json_col();
    let json_configs = parse_model_configs_column(row.get(json_col)?, json_col)?;
    Ok(configs_or_single_config(
        Some(json_configs),
        Some(merged.clone()),
        merged,
    ))
}

/// Write-path: merge payload overrides into a canonical list for the slot.
fn resolve_slot_configs_from_payload(
    single_override: Option<DispatcherModelConfig>,
    list_override: Option<Vec<DispatcherModelConfig>>,
    fallback: DispatcherModelConfig,
) -> Vec<DispatcherModelConfig> {
    if let Some(configs) = list_override {
        return normalize_model_configs(configs);
    }
    let single = single_override
        .filter(|config| !config.is_empty())
        .unwrap_or(fallback);
    normalize_model_configs(vec![single])
}

fn model_config_or_legacy(
    url: String,
    api_key: String,
    model: String,
    legacy_url: &str,
    legacy_api_key: &str,
    legacy_model: &str,
) -> DispatcherModelConfig {
    let config = DispatcherModelConfig::new(&url, &api_key, &model);
    if config.is_empty() {
        DispatcherModelConfig::new(legacy_url, legacy_api_key, legacy_model)
    } else {
        config
    }
}

fn configs_or_single_config(
    configs: Option<Vec<DispatcherModelConfig>>,
    single_config: Option<DispatcherModelConfig>,
    fallback: DispatcherModelConfig,
) -> Vec<DispatcherModelConfig> {
    if let Some(configs) = configs {
        let normalized = normalize_model_configs(configs);
        let single_is_empty = single_config
            .as_ref()
            .map(DispatcherModelConfig::is_empty)
            .unwrap_or_else(|| fallback.is_empty());
        if !normalized.is_empty() || single_is_empty {
            return normalized;
        }
    }

    let single = single_config.unwrap_or(fallback);
    normalize_model_configs(vec![single])
}

fn serialize_model_configs(configs: &[DispatcherModelConfig], label: &str) -> Result<String> {
    serde_json::to_string(configs).with_context(|| format!("serialize {label} model configs"))
}

fn parse_model_configs_column(
    raw: Option<String>,
    column_index: usize,
) -> rusqlite::Result<Vec<DispatcherModelConfig>> {
    raw.as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(|value| {
            serde_json::from_str::<Vec<DispatcherModelConfig>>(value)
                .map(normalize_model_configs)
                .map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        column_index,
                        Type::Text,
                        Box::new(error),
                    )
                })
        })
        .transpose()
        .map(Option::unwrap_or_default)
}

fn build_settings_record(
    configs: DispatcherSettingsConfigLists,
    auto_approve_dispatch: bool,
    context_debug: bool,
) -> DispatcherSettingsRecord {
    let normalized = [
        normalize_model_configs(configs.chat_model_configs),
        normalize_model_configs(configs.summary_model_configs),
        normalize_model_configs(configs.vision_model_configs),
        normalize_model_configs(configs.image_model_configs),
        normalize_model_configs(configs.image_edit_model_configs),
        normalize_model_configs(configs.asr_model_configs),
        normalize_model_configs(configs.tts_model_configs),
        normalize_model_configs(configs.embedding_model_configs),
    ];

    let mut active: Vec<DispatcherModelConfig> = normalized
        .iter()
        .map(|c| active_config(c).unwrap_or_default())
        .collect();

    // ImageEdit inherits the Image slot after Image defaults are applied.
    for slot in ModelSlot::ALL {
        if slot != ModelSlot::ImageEdit {
            slot.apply_defaults(&mut active[slot.index()], &DispatcherModelConfig::default());
        }
    }
    let image_config_for_edit = active[ModelSlot::Image.index()].clone();
    ModelSlot::ImageEdit.apply_defaults(
        &mut active[ModelSlot::ImageEdit.index()],
        &image_config_for_edit,
    );

    DispatcherSettingsRecord {
        api_base: active[ModelSlot::Chat.index()].url.clone(),
        api_key: active[ModelSlot::Chat.index()].api_key.clone(),
        model: active[ModelSlot::Chat.index()].model.clone(),
        summary_model: active[ModelSlot::Summary.index()].model.clone(),
        vision_model: active[ModelSlot::Vision.index()].model.clone(),
        asr_api_key: active[ModelSlot::Asr.index()].api_key.clone(),
        asr_websocket_url: active[ModelSlot::Asr.index()].url.clone(),
        auto_approve_dispatch,
        context_debug,
        image_model_url: active[ModelSlot::Image.index()].url.clone(),
        image_model_api_key: active[ModelSlot::Image.index()].api_key.clone(),
        image_model: active[ModelSlot::Image.index()].model.clone(),
        image_edit_model: active[ModelSlot::ImageEdit.index()].model.clone(),
        chat_model_config: active[ModelSlot::Chat.index()].clone(),
        summary_model_config: active[ModelSlot::Summary.index()].clone(),
        vision_model_config: active[ModelSlot::Vision.index()].clone(),
        image_model_config: active[ModelSlot::Image.index()].clone(),
        image_edit_model_config: active[ModelSlot::ImageEdit.index()].clone(),
        asr_model_config: active[ModelSlot::Asr.index()].clone(),
        tts_model_config: active[ModelSlot::Tts.index()].clone(),
        embedding_model_config: active[ModelSlot::Embedding.index()].clone(),
        chat_model_configs: normalized[ModelSlot::Chat.index()].clone(),
        summary_model_configs: normalized[ModelSlot::Summary.index()].clone(),
        vision_model_configs: normalized[ModelSlot::Vision.index()].clone(),
        image_model_configs: normalized[ModelSlot::Image.index()].clone(),
        image_edit_model_configs: normalized[ModelSlot::ImageEdit.index()].clone(),
        asr_model_configs: normalized[ModelSlot::Asr.index()].clone(),
        tts_model_configs: normalized[ModelSlot::Tts.index()].clone(),
        embedding_model_configs: normalized[ModelSlot::Embedding.index()].clone(),
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DispatcherMode {
    Default,
    Plan,
}

impl DispatcherMode {
    pub fn from_wire(value: &str) -> Result<Self> {
        match value.trim() {
            "" | "default" => Ok(Self::Default),
            "plan" => Ok(Self::Plan),
            other => anyhow::bail!("invalid dispatcher mode: {other}"),
        }
    }

    pub fn as_sql_value(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Plan => "plan",
        }
    }

    fn from_sql_value(value: String) -> Self {
        match value.as_str() {
            "plan" => Self::Plan,
            _ => Self::Default,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DispatcherSessionKind {
    Project,
    Chat,
}

impl DispatcherSessionKind {
    pub fn from_wire(value: &str) -> Result<Self> {
        match value.trim() {
            "" | "project" => Ok(Self::Project),
            "chat" => Ok(Self::Chat),
            other => anyhow::bail!("invalid dispatcher session kind: {other}"),
        }
    }

    pub fn as_sql_value(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Chat => "chat",
        }
    }

    fn from_sql_value(value: String) -> Self {
        match value.as_str() {
            "chat" => Self::Chat,
            _ => Self::Project,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChecklistPlanState {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explanation: Option<String>,
    pub items: Vec<ChecklistPlanItem>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChecklistPlanItem {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub step: String,
    pub status: ChecklistStepStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dispatch_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subprocess_task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ChecklistStepStatus {
    Pending,
    InProgress,
    Completed,
}

impl ChecklistStepStatus {
    pub fn from_wire(value: &str) -> Result<Self> {
        match value.trim() {
            "pending" => Ok(Self::Pending),
            "in_progress" => Ok(Self::InProgress),
            "completed" => Ok(Self::Completed),
            other => anyhow::bail!("invalid checklist step status: {other}"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanQuestionOption {
    pub id: String,
    pub label: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PlanInteraction {
    Question {
        id: String,
        question: String,
        options: Vec<PlanQuestionOption>,
    },
    Ready {
        plan_path: String,
        title: String,
        summary: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherSessionRuntimeState {
    pub mode: DispatcherMode,
    pub checklist: Option<ChecklistPlanState>,
    pub plan_interaction: Option<PlanInteraction>,
    pub active_plan_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherMessageUsageStats {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherMessageRecord {
    pub id: String,
    pub workspace_id: String,
    pub role: String,
    pub content: String,
    pub segments_json: String,
    pub thinking_content: Option<String>,
    pub thinking_elapsed_ms: Option<u64>,
    #[serde(skip_serializing)]
    pub context_payload: Option<String>,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
    pub tool_result_mode: Option<String>,
    pub tool_artifacts: Vec<DispatcherToolArtifactRef>,
    pub tool_calls_json: Option<String>,
    pub usage_stats: Option<DispatcherMessageUsageStats>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherToolArtifactRef {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub preview: String,
    pub char_count: usize,
    pub line_count: usize,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherToolArtifactRecord {
    pub id: String,
    pub workspace_id: String,
    pub message_id: Option<String>,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
    pub title: String,
    pub kind: String,
    pub preview: String,
    pub content: String,
    pub char_count: usize,
    pub line_count: usize,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PythonCodeRunRecord {
    pub run_id: String,
    pub workspace_id: String,
    pub message_id: String,
    pub code_block_index: u32,
    pub code_hash: String,
    pub code: String,
    pub status: String,
    pub stdout: String,
    pub stderr: String,
    pub installed_packages_json: String,
    pub tool_events_json: String,
    pub explanation_markdown: String,
    pub error_reason: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DispatcherSessionTokenUsageSource {
    Primary,
    Summary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherSessionTokenUsageRecord {
    pub workspace_id: String,
    pub model: String,
    pub source_kind: DispatcherSessionTokenUsageSource,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub cached_tokens: u64,
    pub context_window_tokens: u64,
    pub context_window_capacity: u64,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct DispatcherDb {
    pool: Pool<SqliteConnectionManager>,
    path: PathBuf,
}

struct NewDispatcherMessage<'a> {
    workspace_id: &'a str,
    role: &'a str,
    content: &'a str,
    segments_json: Option<String>,
    thinking_content: Option<&'a str>,
    thinking_elapsed_ms: u64,
    context_payload: Option<&'a str>,
    tool_call_id: Option<&'a str>,
    tool_name: Option<&'a str>,
    tool_result_mode: Option<&'a str>,
    tool_calls: Option<&'a [OutboundToolCall]>,
    tool_artifacts: &'a [ToolArtifactDraft],
    usage_stats: Option<&'a DispatcherMessageUsageStats>,
    visible: bool,
}

impl DispatcherDb {
    pub fn new(path: PathBuf) -> Result<Self> {
        let manager = SqliteConnectionManager::file(&path).with_init(|conn| {
            conn.execute_batch(
                "PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000; PRAGMA foreign_keys=ON;",
            )?;
            Ok(())
        });
        let pool = Pool::builder()
            .max_size(4)
            .build(manager)
            .with_context(|| format!("创建数据库连接池失败：{}", path.display()))?;
        let db = Self { pool, path };
        db.init()?;
        Ok(db)
    }

    // ── Settings ──────────────────────────────────────────────

    pub fn get_settings(&self) -> Result<Option<DispatcherSettingsRecord>> {
        let conn = self.conn()?;
        let sql = settings_select_sql();
        conn.query_row(&sql, [], |row| {
            // Resolve per-slot configs via the unified helper
            let chat = resolve_slot_configs_from_row(row, ModelSlot::Chat)?;
            let summary = resolve_slot_configs_from_row(row, ModelSlot::Summary)?;
            let vision = resolve_slot_configs_from_row(row, ModelSlot::Vision)?;
            let image = resolve_slot_configs_from_row(row, ModelSlot::Image)?;
            let image_edit = resolve_slot_configs_from_row(row, ModelSlot::ImageEdit)?;
            let asr = resolve_slot_configs_from_row(row, ModelSlot::Asr)?;
            let tts = resolve_slot_configs_from_row(row, ModelSlot::Tts)?;
            let embedding = resolve_slot_configs_from_row(row, ModelSlot::Embedding)?;

            Ok(build_settings_record(
                DispatcherSettingsConfigLists {
                    chat_model_configs: chat,
                    summary_model_configs: summary,
                    vision_model_configs: vision,
                    image_model_configs: image,
                    image_edit_model_configs: image_edit,
                    asr_model_configs: asr,
                    tts_model_configs: tts,
                    embedding_model_configs: embedding,
                },
                row.get::<_, i32>(settings_column_index("auto_approve_dispatch"))? != 0,
                row.get::<_, i32>(settings_column_index("context_debug"))? != 0,
            ))
        })
        .optional()
        .context("load dispatcher settings")
    }

    #[allow(clippy::too_many_arguments)]
    pub fn save_settings_with_model_configs(
        &self,
        api_base: &str,
        api_key: &str,
        model: &str,
        summary_model: &str,
        vision_model: &str,
        asr_api_key: &str,
        asr_websocket_url: &str,
        auto_approve_dispatch: bool,
        context_debug: bool,
        image_model_url: &str,
        image_model_api_key: &str,
        image_model: &str,
        image_edit_model: &str,
        model_configs: DispatcherSettingsModelConfigs,
    ) -> Result<DispatcherSettingsRecord> {
        let conn = self.conn()?;
        let auto_approve_int = if auto_approve_dispatch { 1 } else { 0 };
        let context_debug_int = if context_debug { 1 } else { 0 };

        // Build per-slot legacy fallbacks from the flat arguments
        let chat_fallback = DispatcherModelConfig::new(api_base, api_key, model);
        let summary_fallback =
            DispatcherModelConfig::new(api_base, api_key, &normalize_summary_model(summary_model));
        let vision_fallback = DispatcherModelConfig::new(api_base, api_key, vision_model);
        let image_fallback =
            DispatcherModelConfig::new(image_model_url, image_model_api_key, image_model);
        // image_edit fallback depends on the resolved image config
        let image_resolved = model_configs
            .image_model_config
            .as_ref()
            .filter(|c| !c.is_empty())
            .cloned()
            .map(|c| c.trimmed())
            .unwrap_or_else(|| image_fallback.clone());
        let image_edit_fallback = {
            let fb_url = if image_model_url.trim().is_empty() {
                image_resolved.url.as_str()
            } else {
                image_model_url
            };
            let fb_key = if image_model_api_key.trim().is_empty() {
                image_resolved.api_key.as_str()
            } else {
                image_model_api_key
            };
            let fb_model = fallback_image_edit_model(image_model, image_edit_model);
            let fb_model = if fb_model.is_empty() {
                image_resolved.model.as_str()
            } else {
                fb_model
            };
            DispatcherModelConfig::new(fb_url, fb_key, fb_model)
        };
        let asr_fallback =
            DispatcherModelConfig::new(asr_websocket_url, asr_api_key, DEFAULT_ASR_MODEL);

        // Resolve each slot's single config, then merge into list
        let singles: [DispatcherModelConfig; 8] = [
            model_configs
                .chat_model_config
                .unwrap_or_else(|| chat_fallback.clone())
                .trimmed(),
            model_configs
                .summary_model_config
                .unwrap_or_else(|| summary_fallback.clone())
                .trimmed(),
            model_configs
                .vision_model_config
                .unwrap_or_else(|| vision_fallback.clone())
                .trimmed(),
            model_configs
                .image_model_config
                .unwrap_or_else(|| image_fallback.clone())
                .trimmed(),
            model_configs
                .image_edit_model_config
                .filter(|c| !c.is_empty())
                .unwrap_or_else(|| image_edit_fallback.clone())
                .trimmed(),
            model_configs
                .asr_model_config
                .unwrap_or_else(|| asr_fallback.clone())
                .trimmed(),
            model_configs.tts_model_config.unwrap_or_default().trimmed(),
            model_configs
                .embedding_model_config
                .unwrap_or_default()
                .trimmed(),
        ];
        let mut lists: [Option<Vec<DispatcherModelConfig>>; 8] = [
            model_configs.chat_model_configs,
            model_configs.summary_model_configs,
            model_configs.vision_model_configs,
            model_configs.image_model_configs,
            model_configs.image_edit_model_configs,
            model_configs.asr_model_configs,
            model_configs.tts_model_configs,
            model_configs.embedding_model_configs,
        ];
        let fallbacks: [DispatcherModelConfig; 8] = [
            chat_fallback,
            summary_fallback,
            vision_fallback,
            image_fallback,
            image_edit_fallback,
            asr_fallback,
            DispatcherModelConfig::default(),
            DispatcherModelConfig::default(),
        ];

        let record = build_settings_record(
            DispatcherSettingsConfigLists {
                chat_model_configs: resolve_slot_configs_from_payload(
                    Some(singles[0].clone()),
                    lists[0].take(),
                    fallbacks[0].clone(),
                ),
                summary_model_configs: resolve_slot_configs_from_payload(
                    Some(singles[1].clone()),
                    lists[1].take(),
                    fallbacks[1].clone(),
                ),
                vision_model_configs: resolve_slot_configs_from_payload(
                    Some(singles[2].clone()),
                    lists[2].take(),
                    fallbacks[2].clone(),
                ),
                image_model_configs: resolve_slot_configs_from_payload(
                    Some(singles[3].clone()),
                    lists[3].take(),
                    fallbacks[3].clone(),
                ),
                image_edit_model_configs: resolve_slot_configs_from_payload(
                    Some(singles[4].clone()),
                    lists[4].take(),
                    fallbacks[4].clone(),
                ),
                asr_model_configs: resolve_slot_configs_from_payload(
                    Some(singles[5].clone()),
                    lists[5].take(),
                    fallbacks[5].clone(),
                ),
                tts_model_configs: resolve_slot_configs_from_payload(
                    Some(singles[6].clone()),
                    lists[6].take(),
                    fallbacks[6].clone(),
                ),
                embedding_model_configs: resolve_slot_configs_from_payload(
                    Some(singles[7].clone()),
                    lists[7].take(),
                    fallbacks[7].clone(),
                ),
            },
            auto_approve_dispatch,
            context_debug,
        );

        // Serialize JSON columns
        let json_cols: [String; 8] = [
            serialize_model_configs(&record.chat_model_configs, "chat")?,
            serialize_model_configs(&record.summary_model_configs, "summary")?,
            serialize_model_configs(&record.vision_model_configs, "vision")?,
            serialize_model_configs(&record.image_model_configs, "image")?,
            serialize_model_configs(&record.image_edit_model_configs, "image edit")?,
            serialize_model_configs(&record.asr_model_configs, "asr")?,
            serialize_model_configs(&record.tts_model_configs, "tts")?,
            serialize_model_configs(&record.embedding_model_configs, "embedding")?,
        ];

        let sql = settings_upsert_sql();
        conn.execute(
            &sql,
            params![
                &record.api_base,
                &record.api_key,
                &record.model,
                &record.summary_model,
                &record.vision_model,
                &record.asr_api_key,
                &record.asr_websocket_url,
                auto_approve_int,
                context_debug_int,
                &record.image_model_url,
                &record.image_model_api_key,
                &record.image_model,
                &record.image_edit_model,
                &record.chat_model_config.url,
                &record.chat_model_config.api_key,
                &record.chat_model_config.model,
                &record.summary_model_config.url,
                &record.summary_model_config.api_key,
                &record.summary_model_config.model,
                &record.vision_model_config.url,
                &record.vision_model_config.api_key,
                &record.vision_model_config.model,
                &record.image_model_config.url,
                &record.image_model_config.api_key,
                &record.image_model_config.model,
                &record.image_edit_model_config.url,
                &record.image_edit_model_config.api_key,
                &record.image_edit_model_config.model,
                &record.asr_model_config.url,
                &record.asr_model_config.api_key,
                &record.asr_model_config.model,
                &record.tts_model_config.url,
                &record.tts_model_config.api_key,
                &record.tts_model_config.model,
                &record.embedding_model_config.url,
                &record.embedding_model_config.api_key,
                &record.embedding_model_config.model,
                &json_cols[0],
                &json_cols[1],
                &json_cols[2],
                &json_cols[3],
                &json_cols[4],
                &json_cols[5],
                &json_cols[6],
                &json_cols[7],
            ],
        )
        .context("save dispatcher settings")?;
        Ok(record)
    }

    pub fn set_auto_approve_dispatch(
        &self,
        auto_approve_dispatch: bool,
    ) -> Result<DispatcherSettingsRecord> {
        let conn = self.conn()?;
        let auto_approve_int = if auto_approve_dispatch { 1 } else { 0 };
        conn.execute(
            "INSERT INTO dispatcher_settings (id, auto_approve_dispatch)
             VALUES ('default', ?1)
             ON CONFLICT(id) DO UPDATE SET auto_approve_dispatch = ?1",
            params![auto_approve_int],
        )
        .context("save dispatcher auto-approve setting")?;
        self.get_settings()?
            .context("load dispatcher settings after auto-approve update")
    }

    // ── Sessions ──────────────────────────────────────────────

    pub fn list_sessions(
        &self,
        project_id: &str,
        kind: DispatcherSessionKind,
    ) -> Result<Vec<DispatcherSessionRecord>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, project_id, kind, title, mode, active_plan_path, category, created_at, updated_at
             FROM dispatcher_sessions
             WHERE project_id = ?1 AND kind = ?2
             ORDER BY updated_at DESC",
        )?;
        let rows = stmt.query_map(params![project_id, kind.as_sql_value()], |row| {
            Ok(DispatcherSessionRecord {
                id: row.get(0)?,
                project_id: row.get(1)?,
                kind: DispatcherSessionKind::from_sql_value(row.get(2)?),
                title: row.get(3)?,
                mode: DispatcherMode::from_sql_value(row.get(4)?),
                active_plan_path: row.get(5)?,
                category: row.get(6)?,
                created_at: row.get(7)?,
                updated_at: row.get(8)?,
            })
        })?;

        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("list dispatcher sessions")
    }

    pub fn create_session(
        &self,
        project_id: &str,
        title: &str,
        kind: DispatcherSessionKind,
        mode: DispatcherMode,
        active_plan_path: Option<&str>,
        category: Option<&str>,
    ) -> Result<DispatcherSessionRecord> {
        let category = category.unwrap_or("").to_string();
        let record = DispatcherSessionRecord {
            id: Uuid::new_v4().to_string(),
            project_id: project_id.to_string(),
            kind,
            title: title.to_string(),
            mode,
            active_plan_path: active_plan_path.map(str::to_string),
            category,
            created_at: now(),
            updated_at: now(),
        };

        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO dispatcher_sessions (id, project_id, kind, title, mode, active_plan_path, category, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                record.id,
                record.project_id,
                record.kind.as_sql_value(),
                record.title,
                record.mode.as_sql_value(),
                record.active_plan_path,
                record.category,
                record.created_at,
                record.updated_at
            ],
        )
        .context("insert dispatcher session")?;

        Ok(record)
    }

    pub fn update_session_title(
        &self,
        session_id: &str,
        title: &str,
    ) -> Result<Option<DispatcherSessionRecord>> {
        let updated_at = now();
        let conn = self.conn()?;
        let changed = conn
            .execute(
                "UPDATE dispatcher_sessions
                 SET title = ?1, updated_at = ?2
                 WHERE id = ?3",
                params![title.trim(), &updated_at, session_id],
            )
            .context("update dispatcher session title")?;

        if changed == 0 {
            return Ok(None);
        }

        conn.query_row(
            "SELECT id, project_id, kind, title, mode, active_plan_path, category, created_at, updated_at
             FROM dispatcher_sessions
             WHERE id = ?1",
            params![session_id],
            |row| {
                Ok(DispatcherSessionRecord {
                    id: row.get(0)?,
                    project_id: row.get(1)?,
                    kind: DispatcherSessionKind::from_sql_value(row.get(2)?),
                    title: row.get(3)?,
                    mode: DispatcherMode::from_sql_value(row.get(4)?),
                    active_plan_path: row.get(5)?,
                    category: row.get(6)?,
                    created_at: row.get(7)?,
                    updated_at: row.get(8)?,
                })
            },
        )
        .optional()
        .context("load dispatcher session after title update")
    }

    pub fn delete_session(&self, session_id: &str) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM dispatcher_tool_artifacts WHERE workspace_id = ?1",
            params![session_id],
        )?;
        tx.execute(
            "DELETE FROM dispatcher_session_token_usage WHERE workspace_id = ?1",
            params![session_id],
        )?;
        delete_chat_image_resources(&tx, session_id)?;
        delete_plan_file_resources(&tx, session_id)?;
        tx.execute(
            "DELETE FROM dispatcher_messages WHERE workspace_id = ?1",
            params![session_id],
        )?;
        tx.execute(
            "DELETE FROM chat_sessions WHERE id = ?1",
            params![session_id],
        )?;
        tx.execute(
            "DELETE FROM project_sessions WHERE id = ?1",
            params![session_id],
        )?;
        tx.execute(
            "DELETE FROM dispatcher_sessions WHERE id = ?1",
            params![session_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    // ── Chat Sessions (v6) ────────────────────────────────────────

    pub fn list_chat_sessions_paginated(
        &self,
        category: Option<&str>,
        cursor: Option<&str>,
        page_size: i64,
    ) -> Result<SessionPage<ChatSessionRecord>> {
        let conn = self.conn()?;
        let total: i64 = if let Some(cat) = category {
            conn.query_row(
                "SELECT COUNT(*) FROM chat_sessions WHERE category = ?1",
                params![cat],
                |row| row.get(0),
            )?
        } else {
            conn.query_row("SELECT COUNT(*) FROM chat_sessions", [], |row| row.get(0))?
        };

        let (where_clause, bind): (String, Vec<Box<dyn rusqlite::types::ToSql>>) =
            match (category, cursor) {
                (Some(cat), Some(cur)) => (
                    "WHERE category = ?1 AND updated_at < ?2".into(),
                    vec![Box::new(cat.to_string()), Box::new(cur.to_string())],
                ),
                (Some(cat), None) => (
                    "WHERE category = ?1".into(),
                    vec![Box::new(cat.to_string())],
                ),
                (None, Some(cur)) => (
                    "WHERE updated_at < ?1".into(),
                    vec![Box::new(cur.to_string())],
                ),
                (None, None) => (String::new(), vec![]),
            };

        let sql = format!(
            "SELECT id, title, category, created_at, updated_at
             FROM chat_sessions
             {}
             ORDER BY updated_at DESC
             LIMIT ?{}",
            where_clause,
            bind.len() + 1
        );
        let mut stmt = conn.prepare(&sql)?;
        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            bind.iter().map(|b| b.as_ref()).collect();
        let limit_param: i64 = page_size + 1;
        let mut all_params: Vec<&dyn rusqlite::types::ToSql> = params_refs;
        all_params.push(&limit_param);

        let rows = stmt.query_map(all_params.as_slice(), |row| {
            Ok(ChatSessionRecord {
                id: row.get(0)?,
                title: row.get(1)?,
                category: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
            })
        })?;
        let mut items: Vec<ChatSessionRecord> =
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .context("list chat sessions paginated")?;

        let has_more = items.len() as i64 > page_size;
        if has_more {
            items.pop();
        }
        let next_cursor = items.last().map(|s| s.updated_at.clone());

        Ok(SessionPage {
            items,
            total,
            has_more,
            next_cursor,
        })
    }

    pub fn create_chat_session(
        &self,
        title: &str,
        category: Option<&str>,
    ) -> Result<ChatSessionRecord> {
        let record = ChatSessionRecord {
            id: Uuid::new_v4().to_string(),
            title: title.to_string(),
            category: category.unwrap_or("tech").to_string(),
            created_at: now(),
            updated_at: now(),
        };
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO chat_sessions (id, title, category, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                record.id,
                record.title,
                record.category,
                record.created_at,
                record.updated_at
            ],
        )
        .context("insert chat session")?;
        conn.execute(
            "INSERT INTO dispatcher_sessions (id, project_id, kind, title, mode, category, created_at, updated_at)
             VALUES (?1, '__global_chat__', 'chat', ?2, 'default', ?3, ?4, ?5)",
            params![
                record.id,
                record.title,
                record.category,
                record.created_at,
                record.updated_at
            ],
        )
        .context("insert chat session into dispatcher_sessions")?;
        Ok(record)
    }

    pub fn delete_chat_session(&self, session_id: &str) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM dispatcher_tool_artifacts WHERE workspace_id = ?1",
            params![session_id],
        )?;
        tx.execute(
            "DELETE FROM dispatcher_session_token_usage WHERE workspace_id = ?1",
            params![session_id],
        )?;
        delete_chat_image_resources(&tx, session_id)?;
        delete_plan_file_resources(&tx, session_id)?;
        tx.execute(
            "DELETE FROM dispatcher_messages WHERE workspace_id = ?1",
            params![session_id],
        )?;
        tx.execute(
            "DELETE FROM chat_sessions WHERE id = ?1",
            params![session_id],
        )?;
        tx.execute(
            "UPDATE dispatcher_sessions
             SET checklist_json = NULL,
                 plan_interaction_json = NULL,
                 active_plan_path = NULL,
                 updated_at = ?1
             WHERE id = ?2",
            params![now(), session_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn update_chat_session_updated_at(&self, session_id: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE chat_sessions SET updated_at = ?1 WHERE id = ?2",
            params![now(), session_id],
        )
        .context("update chat session updated_at")?;
        Ok(())
    }

    pub fn update_chat_session_title(
        &self,
        session_id: &str,
        title: &str,
    ) -> Result<Option<ChatSessionRecord>> {
        let updated_at = now();
        let conn = self.conn()?;
        let changed = conn
            .execute(
                "UPDATE chat_sessions SET title = ?1, updated_at = ?2 WHERE id = ?3",
                params![title.trim(), &updated_at, session_id],
            )
            .context("update chat session title")?;
        if changed == 0 {
            return Ok(None);
        }
        conn.execute(
            "UPDATE dispatcher_sessions SET title = ?1, updated_at = ?2 WHERE id = ?3",
            params![title.trim(), &updated_at, session_id],
        )
        .context("reflect chat title in dispatcher_sessions")?;
        conn.query_row(
            "SELECT id, title, category, created_at, updated_at
             FROM chat_sessions WHERE id = ?1",
            params![session_id],
            |row| {
                Ok(ChatSessionRecord {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    category: row.get(2)?,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                })
            },
        )
        .optional()
        .context("load chat session after title update")
    }

    pub fn set_chat_session_category(&self, session_id: &str, category_id: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE chat_sessions SET category = ?1, updated_at = ?2 WHERE id = ?3",
            params![category_id, now(), session_id],
        )
        .context("set chat session category")?;
        conn.execute(
            "UPDATE dispatcher_sessions SET category = ?1 WHERE id = ?2",
            params![category_id, session_id],
        )
        .context("reflect category in dispatcher_sessions")?;
        Ok(())
    }

    // ── Project Sessions (v6) ─────────────────────────────────────

    pub fn list_project_sessions_paginated(
        &self,
        project_id: &str,
        offset: i64,
        page_size: i64,
    ) -> Result<SessionPage<ProjectSessionRecord>> {
        let conn = self.conn()?;
        let total: i64 = conn.query_row(
            "SELECT COUNT(*) FROM project_sessions WHERE project_id = ?1",
            params![project_id],
            |row| row.get(0),
        )?;

        let mut stmt = conn.prepare(
            "SELECT id, project_id, title, mode, active_plan_path, created_at, updated_at
             FROM project_sessions
             WHERE project_id = ?1
             ORDER BY updated_at DESC
             LIMIT ?2 OFFSET ?3",
        )?;
        let rows = stmt.query_map(
            params![project_id, page_size, offset],
            |row| {
                Ok(ProjectSessionRecord {
                    id: row.get(0)?,
                    project_id: row.get(1)?,
                    title: row.get(2)?,
                    mode: DispatcherMode::from_sql_value(row.get(3)?),
                    active_plan_path: row.get(4)?,
                    created_at: row.get(5)?,
                    updated_at: row.get(6)?,
                })
            },
        )?;
        let items: Vec<ProjectSessionRecord> =
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .context("list project sessions paginated")?;

        let has_more = (offset + items.len() as i64) < total;
        let next_cursor = items.last().map(|s| s.updated_at.clone());

        Ok(SessionPage {
            items,
            total,
            has_more,
            next_cursor,
        })
    }

    pub fn create_project_session(
        &self,
        project_id: &str,
        title: &str,
        mode: DispatcherMode,
        active_plan_path: Option<&str>,
    ) -> Result<ProjectSessionRecord> {
        let record = ProjectSessionRecord {
            id: Uuid::new_v4().to_string(),
            project_id: project_id.to_string(),
            title: title.to_string(),
            mode,
            active_plan_path: active_plan_path.map(str::to_string),
            created_at: now(),
            updated_at: now(),
        };
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO project_sessions (id, project_id, title, mode, active_plan_path, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                record.id,
                record.project_id,
                record.title,
                record.mode.as_sql_value(),
                record.active_plan_path,
                record.created_at,
                record.updated_at
            ],
        )
        .context("insert project session")?;
        conn.execute(
            "INSERT INTO dispatcher_sessions (id, project_id, kind, title, mode, active_plan_path, category, created_at, updated_at)
             VALUES (?1, ?2, 'project', ?3, ?4, ?5, '', ?6, ?7)",
            params![
                record.id,
                record.project_id,
                record.title,
                record.mode.as_sql_value(),
                record.active_plan_path,
                record.created_at,
                record.updated_at
            ],
        )
        .context("insert project session into dispatcher_sessions")?;
        Ok(record)
    }

    pub fn delete_project_session(&self, session_id: &str) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM dispatcher_tool_artifacts WHERE workspace_id = ?1",
            params![session_id],
        )?;
        tx.execute(
            "DELETE FROM dispatcher_session_token_usage WHERE workspace_id = ?1",
            params![session_id],
        )?;
        delete_chat_image_resources(&tx, session_id)?;
        delete_plan_file_resources(&tx, session_id)?;
        tx.execute(
            "DELETE FROM dispatcher_messages WHERE workspace_id = ?1",
            params![session_id],
        )?;
        tx.execute(
            "DELETE FROM project_sessions WHERE id = ?1",
            params![session_id],
        )?;
        tx.execute(
            "UPDATE dispatcher_sessions
             SET checklist_json = NULL,
                 plan_interaction_json = NULL,
                 active_plan_path = NULL,
                 updated_at = ?1
             WHERE id = ?2",
            params![now(), session_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn update_project_session_updated_at(&self, session_id: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE project_sessions SET updated_at = ?1 WHERE id = ?2",
            params![now(), session_id],
        )
        .context("update project session updated_at")?;
        Ok(())
    }


    pub fn get_session_title(&self, session_id: &str) -> Result<String> {
        let conn = self.conn()?;
        let title: String = conn
            .query_row(
                "SELECT title FROM dispatcher_sessions WHERE id = ?1",
                rusqlite::params![session_id],
                |row| row.get(0),
            )
            .optional()
            .context("load dispatcher session title")?
            .unwrap_or_else(|| "untitled".to_string());
        Ok(title)
    }

    pub fn get_session_runtime_state(
        &self,
        session_id: &str,
    ) -> Result<DispatcherSessionRuntimeState> {
        let conn = self.conn()?;
        let (mode, active_plan_path, checklist_json, plan_interaction_json) = conn
            .query_row(
                "SELECT mode, active_plan_path, checklist_json, plan_interaction_json
                 FROM dispatcher_sessions
                 WHERE id = ?1",
                params![session_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                },
            )
            .optional()
            .context("load dispatcher session runtime state")?
            .with_context(|| format!("dispatcher session not found: {session_id}"))?;

        Ok(DispatcherSessionRuntimeState {
            mode: DispatcherMode::from_sql_value(mode),
            active_plan_path,
            checklist: parse_optional_json(checklist_json, "checklist_json")?,
            plan_interaction: parse_optional_json(plan_interaction_json, "plan_interaction_json")?,
        })
    }

    pub fn set_session_mode(
        &self,
        session_id: &str,
        mode: DispatcherMode,
    ) -> Result<DispatcherSessionRuntimeState> {
        let conn = self.conn()?;
        let changed = conn
            .execute(
                "UPDATE dispatcher_sessions
                 SET mode = ?1, updated_at = ?2
                 WHERE id = ?3",
                params![mode.as_sql_value(), now(), session_id],
            )
            .context("set dispatcher session mode")?;
        if changed == 0 {
            anyhow::bail!("dispatcher session not found: {session_id}");
        }
        self.get_session_runtime_state(session_id)
    }

    pub fn update_checklist(
        &self,
        session_id: &str,
        checklist: &ChecklistPlanState,
    ) -> Result<DispatcherSessionRuntimeState> {
        let checklist_json = serde_json::to_string(checklist).context("serialize checklist")?;
        let conn = self.conn()?;
        let changed = conn
            .execute(
                "UPDATE dispatcher_sessions
                 SET checklist_json = ?1, updated_at = ?2
                 WHERE id = ?3",
                params![checklist_json, now(), session_id],
            )
            .context("update dispatcher checklist")?;
        if changed == 0 {
            anyhow::bail!("dispatcher session not found: {session_id}");
        }
        self.get_session_runtime_state(session_id)
    }

    pub fn clear_checklist(&self, session_id: &str) -> Result<DispatcherSessionRuntimeState> {
        let conn = self.conn()?;
        let changed = conn
            .execute(
                "UPDATE dispatcher_sessions
                 SET checklist_json = NULL, updated_at = ?1
                 WHERE id = ?2",
                params![now(), session_id],
            )
            .context("clear dispatcher checklist")?;
        if changed == 0 {
            anyhow::bail!("dispatcher session not found: {session_id}");
        }
        self.get_session_runtime_state(session_id)
    }

    pub fn attach_checklist_subprocess(
        &self,
        session_id: &str,
        dispatch_id: &str,
        task_id: &str,
    ) -> Result<DispatcherSessionRuntimeState> {
        let mut state = self.get_session_runtime_state(session_id)?;
        let Some(mut checklist) = state.checklist.take() else {
            return Ok(state);
        };

        if let Some(item) = checklist
            .items
            .iter_mut()
            .find(|item| item.dispatch_id.as_deref() == Some(dispatch_id))
        {
            item.status = ChecklistStepStatus::InProgress;
            item.subprocess_task_id = Some(task_id.to_string());
            checklist.updated_at = now();
            return self.update_checklist(session_id, &checklist);
        }

        Ok(DispatcherSessionRuntimeState {
            checklist: Some(checklist),
            ..state
        })
    }

    pub fn clear_checklist_dispatch(
        &self,
        session_id: &str,
        dispatch_id: &str,
    ) -> Result<DispatcherSessionRuntimeState> {
        let mut state = self.get_session_runtime_state(session_id)?;
        let Some(mut checklist) = state.checklist.take() else {
            return Ok(state);
        };

        let mut changed = false;
        for item in &mut checklist.items {
            if item.dispatch_id.as_deref() == Some(dispatch_id) {
                item.dispatch_id = None;
                item.subprocess_task_id = None;
                item.agent = None;
                item.detail = None;
                if item.status == ChecklistStepStatus::InProgress {
                    item.status = ChecklistStepStatus::Pending;
                }
                changed = true;
            }
        }

        if changed {
            checklist.updated_at = now();
            return self.update_checklist(session_id, &checklist);
        }

        Ok(DispatcherSessionRuntimeState {
            checklist: Some(checklist),
            ..state
        })
    }

    pub fn set_plan_interaction(
        &self,
        session_id: &str,
        interaction: Option<&PlanInteraction>,
    ) -> Result<DispatcherSessionRuntimeState> {
        let interaction_json = interaction
            .map(serde_json::to_string)
            .transpose()
            .context("serialize plan interaction")?;
        let conn = self.conn()?;
        let changed = conn
            .execute(
                "UPDATE dispatcher_sessions
                 SET plan_interaction_json = ?1, updated_at = ?2
                 WHERE id = ?3",
                params![interaction_json, now(), session_id],
            )
            .context("update dispatcher plan interaction")?;
        if changed == 0 {
            anyhow::bail!("dispatcher session not found: {session_id}");
        }
        self.get_session_runtime_state(session_id)
    }

    pub fn set_active_plan_path(
        &self,
        session_id: &str,
        plan_path: Option<&str>,
    ) -> Result<DispatcherSessionRuntimeState> {
        let conn = self.conn()?;
        let changed = conn
            .execute(
                "UPDATE dispatcher_sessions
                 SET active_plan_path = ?1, updated_at = ?2
                 WHERE id = ?3",
                params![plan_path, now(), session_id],
            )
            .context("update dispatcher active plan path")?;
        if changed == 0 {
            anyhow::bail!("dispatcher session not found: {session_id}");
        }
        self.get_session_runtime_state(session_id)
    }

    // ── Messages ──────────────────────────────────────────────

    pub fn add_visible_message(
        &self,
        workspace_id: &str,
        role: &str,
        content: &str,
        segments_json: Option<String>,
    ) -> Result<DispatcherMessageRecord> {
        self.add_message(NewDispatcherMessage {
            workspace_id,
            role,
            content,
            segments_json,
            thinking_content: None,
            thinking_elapsed_ms: 0,
            context_payload: None,
            tool_call_id: None,
            tool_name: None,
            tool_result_mode: None,
            tool_calls: None,
            tool_artifacts: &[],
            usage_stats: None,
            visible: true,
        })
    }

    pub fn add_visible_message_with_usage(
        &self,
        workspace_id: &str,
        role: &str,
        content: &str,
        usage_stats: &DispatcherMessageUsageStats,
    ) -> Result<DispatcherMessageRecord> {
        self.add_visible_message_with_usage_and_thinking(
            workspace_id,
            role,
            content,
            usage_stats,
            None,
            0,
        )
    }

    pub fn add_visible_message_with_usage_and_thinking(
        &self,
        workspace_id: &str,
        role: &str,
        content: &str,
        usage_stats: &DispatcherMessageUsageStats,
        thinking_content: Option<&str>,
        thinking_elapsed_ms: u64,
    ) -> Result<DispatcherMessageRecord> {
        self.add_message(NewDispatcherMessage {
            workspace_id,
            role,
            content,
            segments_json: None,
            thinking_content,
            thinking_elapsed_ms,
            context_payload: None,
            tool_call_id: None,
            tool_name: None,
            tool_result_mode: None,
            tool_calls: None,
            tool_artifacts: &[],
            usage_stats: Some(usage_stats),
            visible: true,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_visible_message_with_tools(
        &self,
        workspace_id: &str,
        role: &str,
        content: &str,
        tool_call_id: Option<&str>,
        tool_name: Option<&str>,
        tool_result_mode: Option<&str>,
        tool_calls: Option<&[OutboundToolCall]>,
    ) -> Result<DispatcherMessageRecord> {
        self.add_visible_message_with_tools_and_thinking(
            workspace_id,
            role,
            content,
            tool_call_id,
            tool_name,
            tool_result_mode,
            tool_calls,
            None,
            0,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_visible_message_with_tools_and_thinking(
        &self,
        workspace_id: &str,
        role: &str,
        content: &str,
        tool_call_id: Option<&str>,
        tool_name: Option<&str>,
        tool_result_mode: Option<&str>,
        tool_calls: Option<&[OutboundToolCall]>,
        thinking_content: Option<&str>,
        thinking_elapsed_ms: u64,
    ) -> Result<DispatcherMessageRecord> {
        self.add_message(NewDispatcherMessage {
            workspace_id,
            role,
            content,
            segments_json: None,
            thinking_content,
            thinking_elapsed_ms,
            context_payload: None,
            tool_call_id,
            tool_name,
            tool_result_mode,
            tool_calls,
            tool_artifacts: &[],
            usage_stats: None,
            visible: true,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_visible_tool_result(
        &self,
        workspace_id: &str,
        content: &str,
        context_payload: &str,
        tool_call_id: Option<&str>,
        tool_name: Option<&str>,
        tool_result_mode: Option<&str>,
        tool_artifacts: &[ToolArtifactDraft],
    ) -> Result<DispatcherMessageRecord> {
        self.add_message(NewDispatcherMessage {
            workspace_id,
            role: "tool",
            content,
            segments_json: None,
            thinking_content: None,
            thinking_elapsed_ms: 0,
            context_payload: Some(context_payload),
            tool_call_id,
            tool_name,
            tool_result_mode,
            tool_calls: None,
            tool_artifacts,
            usage_stats: None,
            visible: true,
        })
    }

    pub fn compact_successful_tool_retry(
        &self,
        workspace_id: &str,
        tool_name: &str,
        current_tool_call_id: &str,
    ) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        let cutoff_rowid = latest_user_message_rowid(&tx, workspace_id)?;
        let retry_context_pattern = format!("{TOOL_RETRY_CONTEXT_PREFIX}%");
        let retry_messages = {
            let mut stmt = tx.prepare(
                "SELECT id, tool_call_id
                 FROM dispatcher_messages
                 WHERE workspace_id = ?1
                   AND role = 'tool'
                   AND tool_name = ?2
                   AND rowid >= ?3
                   AND tool_call_id IS NOT NULL
                   AND tool_call_id <> ?4
                   AND context_payload LIKE ?5",
            )?;
            let rows = stmt.query_map(
                params![
                    workspace_id,
                    tool_name,
                    cutoff_rowid,
                    current_tool_call_id,
                    retry_context_pattern
                ],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };

        if retry_messages.is_empty() {
            tx.commit()
                .context("commit empty dispatcher retry compaction")?;
            return Ok(());
        }

        let failed_tool_call_ids = retry_messages
            .iter()
            .map(|(_, tool_call_id)| tool_call_id.clone())
            .collect::<HashSet<_>>();

        for (message_id, _) in &retry_messages {
            tx.execute(
                "DELETE FROM dispatcher_tool_artifacts WHERE message_id = ?1",
                params![message_id],
            )
            .context("delete compacted retry tool artifacts")?;
            tx.execute(
                "DELETE FROM dispatcher_messages WHERE id = ?1",
                params![message_id],
            )
            .context("delete compacted retry tool message")?;
        }

        let assistant_messages = {
            let mut stmt = tx.prepare(
                "SELECT id, segments_json, tool_calls_json
                 FROM dispatcher_messages
                 WHERE workspace_id = ?1
                   AND role = 'assistant'
                   AND rowid >= ?2
                   AND tool_calls_json IS NOT NULL
                 ORDER BY rowid ASC",
            )?;
            let rows = stmt.query_map(params![workspace_id, cutoff_rowid], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };

        for (message_id, segments_json, tool_calls_json) in assistant_messages {
            let Ok(mut tool_calls) =
                serde_json::from_str::<Vec<OutboundToolCall>>(&tool_calls_json)
            else {
                continue;
            };
            let original_len = tool_calls.len();
            tool_calls.retain(|call| !failed_tool_call_ids.contains(&call.id));
            if tool_calls.len() == original_len {
                continue;
            }

            let content = segments_to_markdown(&parse_segments_json(&segments_json));
            if tool_calls.is_empty() && content.trim().is_empty() {
                tx.execute(
                    "DELETE FROM dispatcher_tool_artifacts WHERE message_id = ?1",
                    params![&message_id],
                )
                .context("delete compacted retry assistant artifacts")?;
                tx.execute(
                    "DELETE FROM dispatcher_messages WHERE id = ?1",
                    params![&message_id],
                )
                .context("delete compacted retry assistant message")?;
            } else {
                let next_tool_calls_json = if tool_calls.is_empty() {
                    None
                } else {
                    Some(
                        serde_json::to_string(&tool_calls)
                            .context("serialize compacted assistant tool calls")?,
                    )
                };
                tx.execute(
                    "UPDATE dispatcher_messages SET tool_calls_json = ?1 WHERE id = ?2",
                    params![next_tool_calls_json, &message_id],
                )
                .context("update compacted assistant tool calls")?;
            }
        }

        tx.commit()
            .context("commit dispatcher successful retry compaction")
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_hidden_message(
        &self,
        workspace_id: &str,
        role: &str,
        content: &str,
        tool_call_id: Option<&str>,
        tool_name: Option<&str>,
        tool_result_mode: Option<&str>,
        tool_calls: Option<&[OutboundToolCall]>,
    ) -> Result<DispatcherMessageRecord> {
        self.add_message(NewDispatcherMessage {
            workspace_id,
            role,
            content,
            segments_json: None,
            thinking_content: None,
            thinking_elapsed_ms: 0,
            context_payload: None,
            tool_call_id,
            tool_name,
            tool_result_mode,
            tool_calls,
            tool_artifacts: &[],
            usage_stats: None,
            visible: false,
        })
    }

    pub fn list_visible_messages(
        &self,
        workspace_id: &str,
    ) -> Result<Vec<DispatcherMessageRecord>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, workspace_id, role, segments_json, thinking_content, thinking_elapsed_ms, context_payload, tool_call_id, tool_name, tool_result_mode, tool_artifacts_json, tool_calls_json, usage_stats_json, created_at
             FROM dispatcher_messages
             WHERE workspace_id = ?1 AND visible = 1
             ORDER BY created_at ASC, rowid ASC",
        )?;
        let rows = stmt.query_map(params![workspace_id], map_dispatcher_message_record)?;

        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("load visible dispatcher messages")
    }

    /// Load recent complete visible dialogues for session title generation.
    ///
    /// The cutoff is based on user-started turns, so the latest user message and its
    /// following assistant/tool messages stay together instead of being clipped by a
    /// raw message count.
    pub fn list_recent_visible_dialogue_messages(
        &self,
        workspace_id: &str,
        max_dialogues: usize,
    ) -> Result<Vec<DispatcherMessageRecord>> {
        let conn = self.conn()?;
        let cutoff_rowid = self.find_dialogue_cutoff_rowid(&conn, workspace_id, max_dialogues)?;
        let mut stmt = conn.prepare(
            "SELECT id, workspace_id, role, segments_json, thinking_content, thinking_elapsed_ms, context_payload, tool_call_id, tool_name, tool_result_mode, tool_artifacts_json, tool_calls_json, usage_stats_json, created_at
             FROM dispatcher_messages
             WHERE workspace_id = ?1
               AND visible = 1
               AND context_cleared = 0
               AND rowid >= ?2
             ORDER BY created_at ASC, rowid ASC",
        )?;
        let rows = stmt.query_map(
            params![workspace_id, cutoff_rowid],
            map_dispatcher_message_record,
        )?;

        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("load recent visible dispatcher dialogue messages")
    }

    /// Load only the recent dialogue window for one dispatcher session.
    ///
    /// Note:
    /// - `workspace_id` here is the dispatcher session id used by the frontend.
    /// - One project can have multiple dispatcher sessions; history is isolated by session id.
    /// - Only the most recent `MAX_LLM_DIALOGUES` user-started dialogues are injected into the LLM.
    pub fn load_llm_history(&self, workspace_id: &str) -> Result<Vec<ChatMessage>> {
        let conn = self.conn()?;
        let cutoff_rowid =
            self.find_dialogue_cutoff_rowid(&conn, workspace_id, MAX_LLM_DIALOGUES)?;

        let mut stmt = conn.prepare(
            "SELECT role, segments_json, context_payload, tool_call_id, tool_name, tool_calls_json, thinking_content
             FROM dispatcher_messages
             WHERE workspace_id = ?1 AND rowid >= ?2 AND context_cleared = 0
             ORDER BY rowid ASC",
        )?;
        let rows = stmt.query_map(params![workspace_id, cutoff_rowid], |row| {
            let role: String = row.get(0)?;
            let segments_json: String = row.get(1)?;
            let context_payload: Option<String> = row.get(2)?;
            let tool_call_id: Option<String> = row.get(3)?;
            let tool_name: Option<String> = row.get(4)?;
            let tool_calls_json: Option<String> = row.get(5)?;
            let thinking_content: Option<String> = row.get(6)?;

            let content = if let Some(payload) = context_payload {
                payload
            } else {
                let segments = parse_segments_json(&segments_json);
                segments_to_markdown(&segments)
            };

            let tool_calls = tool_calls_json
                .as_deref()
                .and_then(|json| serde_json::from_str::<Vec<OutboundToolCall>>(json).ok());

            Ok(ChatMessage {
                reasoning_content: if role == "assistant" {
                    thinking_content.filter(|content| !content.trim().is_empty())
                } else {
                    None
                },
                role,
                content,
                tool_call_id,
                name: tool_name,
                tool_calls,
            })
        })?;

        let mut messages = rows
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("load dispatcher llm history")?;
        messages.retain(should_keep_llm_message);

        while matches!(messages.first().map(|m| m.role.as_str()), Some("tool")) {
            messages.remove(0);
        }

        Ok(messages)
    }

    /// Fetch only the content of the latest visible user message.
    ///
    /// Lighter than `load_llm_history` — skips parsing tool calls, context payloads,
    /// and dialogue cutoff calculations.
    pub fn get_latest_user_message_content(&self, workspace_id: &str) -> Result<Option<String>> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT segments_json
             FROM dispatcher_messages
             WHERE workspace_id = ?1 AND role = 'user' AND visible = 1 AND context_cleared = 0
             ORDER BY rowid DESC
             LIMIT 1",
            params![workspace_id],
            |row| {
                let segments_json: String = row.get(0)?;
                Ok(segments_to_markdown(&parse_segments_json(&segments_json)))
            },
        )
        .optional()
        .context("load latest dispatcher user message content")
    }

    pub fn get_visible_message_content(
        &self,
        workspace_id: &str,
        message_id: &str,
    ) -> Result<Option<String>> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT segments_json
             FROM dispatcher_messages
             WHERE workspace_id = ?1 AND id = ?2 AND visible = 1
             LIMIT 1",
            params![workspace_id, message_id],
            |row| {
                let segments_json: String = row.get(0)?;
                Ok(segments_to_markdown(&parse_segments_json(&segments_json)))
            },
        )
        .optional()
        .context("load dispatcher visible message content")
    }

    /// Fetch recent tool/assistant message content for subprocess context.
    ///
    /// Returns up to `limit` recent non-user messages (tool results and assistant replies),
    /// each as (role, tool_name, content) tuples — enough to build exploration context
    /// without loading the full LLM history.
    pub fn list_recent_exploration_content(
        &self,
        workspace_id: &str,
        limit: usize,
    ) -> Result<Vec<(String, Option<String>, String)>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT role, tool_name, segments_json
             FROM dispatcher_messages
             WHERE workspace_id = ?1
               AND role IN ('tool', 'assistant')
               AND visible = 1
               AND context_cleared = 0
             ORDER BY rowid DESC
             LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(params![workspace_id, limit as i64], |row| {
                let role: String = row.get(0)?;
                let tool_name: Option<String> = row.get(1)?;
                let segments_json: String = row.get(2)?;
                let content = segments_to_markdown(&parse_segments_json(&segments_json));
                Ok((role, tool_name, content))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("load recent dispatcher exploration content")?;
        Ok(rows)
    }

    pub fn clear_context_messages(&self, workspace_id: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE dispatcher_messages
             SET context_cleared = 1
             WHERE workspace_id = ?1 AND context_cleared = 0",
            params![workspace_id],
        )
        .context("logically clear dispatcher messages")?;
        Ok(())
    }

    pub fn clear_messages(&self, workspace_id: &str) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM dispatcher_tool_artifacts WHERE workspace_id = ?1",
            params![workspace_id],
        )
        .context("clear dispatcher tool artifacts")?;
        tx.execute(
            "DELETE FROM dispatcher_session_token_usage WHERE workspace_id = ?1",
            params![workspace_id],
        )
        .context("clear dispatcher session token usage")?;
        delete_chat_image_resources(&tx, workspace_id)?;
        delete_plan_file_resources(&tx, workspace_id)?;
        tx.execute(
            "DELETE FROM dispatcher_messages WHERE workspace_id = ?1",
            params![workspace_id],
        )
        .context("clear dispatcher messages")?;
        tx.execute(
            "UPDATE dispatcher_sessions
             SET checklist_json = NULL,
                 plan_interaction_json = NULL,
                 active_plan_path = NULL,
                 updated_at = ?1
             WHERE id = ?2",
            params![now(), workspace_id],
        )
        .context("clear dispatcher planning state")?;
        tx.execute(
            "UPDATE project_sessions
             SET active_plan_path = NULL,
                 checklist_json = NULL,
                 plan_interaction_json = NULL,
                 updated_at = ?1
             WHERE id = ?2",
            params![now(), workspace_id],
        )
        .context("clear project session planning state")?;
        tx.commit().context("commit dispatcher message cleanup")?;
        Ok(())
    }

    // ── Chat categories ────────────────────────────────────────

    pub fn list_chat_categories(&self) -> Result<Vec<ChatCategory>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, name, icon, color, sort_order, created_at, updated_at
             FROM chat_categories
             ORDER BY sort_order ASC, created_at ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ChatCategory {
                id: row.get(0)?,
                name: row.get(1)?,
                icon: row.get(2)?,
                color: row.get(3)?,
                sort_order: row.get(4)?,
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("list chat categories")
    }

    pub fn create_chat_category(
        &self,
        name: &str,
        icon: &str,
        color: &str,
    ) -> Result<ChatCategory> {
        let now = now();
        let conn = self.conn()?;
        let max_order: i32 = conn
            .query_row(
                "SELECT COALESCE(MAX(sort_order), -1) FROM chat_categories",
                [],
                |row| row.get(0),
            )
            .unwrap_or(-1);
        let next_order = max_order + 1;
        let id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO chat_categories (id, name, icon, color, sort_order, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![id, name.trim(), icon, color, next_order, now, now],
        )
        .context("insert chat category")?;
        Ok(ChatCategory {
            id,
            name: name.trim().to_string(),
            icon: icon.to_string(),
            color: color.to_string(),
            sort_order: next_order,
            created_at: now.clone(),
            updated_at: now,
        })
    }

    pub fn update_chat_category(
        &self,
        category_id: &str,
        name: Option<&str>,
        icon: Option<&str>,
        color: Option<&str>,
    ) -> Result<Option<ChatCategory>> {
        let updated = now();
        let conn = self.conn()?;
        let mut parts: Vec<String> = Vec::new();
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        if let Some(n) = name {
            parts.push(format!("name = ?{}", params_vec.len() + 1));
            params_vec.push(Box::new(n.trim().to_string()));
        }
        if let Some(i) = icon {
            parts.push(format!("icon = ?{}", params_vec.len() + 1));
            params_vec.push(Box::new(i.to_string()));
        }
        if let Some(c) = color {
            parts.push(format!("color = ?{}", params_vec.len() + 1));
            params_vec.push(Box::new(c.to_string()));
        }
        if parts.is_empty() {
            return self.get_chat_category(category_id);
        }
        parts.push(format!("updated_at = ?{}", params_vec.len() + 1));
        params_vec.push(Box::new(updated.clone()));
        params_vec.push(Box::new(category_id.to_string()));

        let sql = format!(
            "UPDATE chat_categories SET {} WHERE id = ?{}",
            parts.join(", "),
            params_vec.len()
        );
        let params_refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();
        let changed = conn
            .execute(&sql, params_refs.as_slice())
            .context("update chat category")?;
        if changed == 0 {
            return Ok(None);
        }
        self.get_chat_category(category_id)
    }

    fn get_chat_category(&self, category_id: &str) -> Result<Option<ChatCategory>> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT id, name, icon, color, sort_order, created_at, updated_at
             FROM chat_categories WHERE id = ?1",
            params![category_id],
            |row| {
                Ok(ChatCategory {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    icon: row.get(2)?,
                    color: row.get(3)?,
                    sort_order: row.get(4)?,
                    created_at: row.get(5)?,
                    updated_at: row.get(6)?,
                })
            },
        )
        .optional()
        .context("get chat category")
    }

    pub fn delete_chat_category(&self, category_id: &str) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction().context("begin delete category transaction")?;
        tx.execute(
            "UPDATE dispatcher_sessions SET category = '' WHERE category = ?1",
            params![category_id],
        )
        .context("reassign uncategorized sessions")?;
        tx.execute(
            "DELETE FROM chat_categories WHERE id = ?1",
            params![category_id],
        )
        .context("delete chat category")?;
        tx.commit().context("commit delete category")?;
        Ok(())
    }

    pub fn set_session_category(&self, session_id: &str, category_id: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE dispatcher_sessions SET category = ?1, updated_at = ?2 WHERE id = ?3",
            params![category_id, now(), session_id],
        )
        .context("set session category")?;
        Ok(())
    }

    pub fn reorder_chat_categories(&self, ordered_ids: &[String]) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction().context("begin reorder categories transaction")?;
        {
            let mut stmt = tx
                .prepare("UPDATE chat_categories SET sort_order = ?1, updated_at = ?2 WHERE id = ?3")
                .context("prepare reorder statement")?;
            let now = now();
            for (order, id) in ordered_ids.iter().enumerate() {
                stmt.execute(params![order as i32, &now, id])
                    .with_context(|| format!("reorder category {id}"))?;
            }
        }
        tx.commit().context("commit reorder categories")?;
        Ok(())
    }

    pub fn upsert_session_token_usage(
        &self,
        workspace_id: &str,
        model: &str,
        source_kind: DispatcherSessionTokenUsageSource,
        usage: &LlmUsage,
    ) -> Result<DispatcherSessionTokenUsageRecord> {
        let updated_at = now();
        let record = DispatcherSessionTokenUsageRecord {
            workspace_id: workspace_id.to_string(),
            model: model.to_string(),
            source_kind,
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
            total_tokens: usage_total_tokens(usage),
            cached_tokens: usage.cached_tokens(),
            context_window_tokens: usage.prompt_tokens,
            context_window_capacity: default_context_window_capacity(model),
            updated_at,
        };
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO dispatcher_session_token_usage (
                workspace_id, model, source_kind, prompt_tokens, completion_tokens, total_tokens,
                cached_tokens, context_window_tokens, context_window_capacity, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(workspace_id, model, source_kind) DO UPDATE SET
                prompt_tokens = dispatcher_session_token_usage.prompt_tokens + excluded.prompt_tokens,
                completion_tokens = dispatcher_session_token_usage.completion_tokens + excluded.completion_tokens,
                total_tokens = dispatcher_session_token_usage.total_tokens + excluded.total_tokens,
                cached_tokens = dispatcher_session_token_usage.cached_tokens + excluded.cached_tokens,
                context_window_tokens = excluded.context_window_tokens,
                context_window_capacity = excluded.context_window_capacity,
                updated_at = excluded.updated_at",
            params![
                &record.workspace_id,
                &record.model,
                source_kind.as_sql_value(),
                record.prompt_tokens as i64,
                record.completion_tokens as i64,
                record.total_tokens as i64,
                record.cached_tokens as i64,
                record.context_window_tokens as i64,
                record.context_window_capacity as i64,
                &record.updated_at,
            ],
        )
        .context("upsert dispatcher session token usage")?;
        self.get_session_token_usage_record(&conn, workspace_id, model, source_kind)
    }

    fn get_session_token_usage_record(
        &self,
        conn: &Connection,
        workspace_id: &str,
        model: &str,
        source_kind: DispatcherSessionTokenUsageSource,
    ) -> Result<DispatcherSessionTokenUsageRecord> {
        conn.query_row(
            "SELECT workspace_id, model, source_kind, prompt_tokens, completion_tokens, total_tokens,
                    cached_tokens, context_window_tokens, context_window_capacity, updated_at
             FROM dispatcher_session_token_usage
             WHERE workspace_id = ?1 AND model = ?2 AND source_kind = ?3",
            params![workspace_id, model, source_kind.as_sql_value()],
            |row| {
                Ok(DispatcherSessionTokenUsageRecord {
                    workspace_id: row.get(0)?,
                    model: row.get(1)?,
                    source_kind: DispatcherSessionTokenUsageSource::from_sql_value(row.get(2)?),
                    prompt_tokens: row.get::<_, i64>(3)? as u64,
                    completion_tokens: row.get::<_, i64>(4)? as u64,
                    total_tokens: row.get::<_, i64>(5)? as u64,
                    cached_tokens: row.get::<_, i64>(6)? as u64,
                    context_window_tokens: row.get::<_, i64>(7)? as u64,
                    context_window_capacity: row.get::<_, i64>(8)? as u64,
                    updated_at: row.get(9)?,
                })
            },
        )
        .context("load dispatcher session token usage after upsert")
    }

    pub fn list_session_token_usage(
        &self,
        workspace_id: &str,
    ) -> Result<Vec<DispatcherSessionTokenUsageRecord>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT workspace_id, model, source_kind, prompt_tokens, completion_tokens, total_tokens,
                    cached_tokens, context_window_tokens, context_window_capacity, updated_at
             FROM dispatcher_session_token_usage
             WHERE workspace_id = ?1
             ORDER BY updated_at DESC, model ASC, source_kind ASC",
        )?;
        let rows = stmt.query_map(params![workspace_id], |row| {
            Ok(DispatcherSessionTokenUsageRecord {
                workspace_id: row.get(0)?,
                model: row.get(1)?,
                source_kind: DispatcherSessionTokenUsageSource::from_sql_value(row.get(2)?),
                prompt_tokens: row.get::<_, i64>(3)? as u64,
                completion_tokens: row.get::<_, i64>(4)? as u64,
                total_tokens: row.get::<_, i64>(5)? as u64,
                cached_tokens: row.get::<_, i64>(6)? as u64,
                context_window_tokens: row.get::<_, i64>(7)? as u64,
                context_window_capacity: row.get::<_, i64>(8)? as u64,
                updated_at: row.get(9)?,
            })
        })?;

        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("list dispatcher session token usage")
    }

    pub fn get_tool_artifact(
        &self,
        workspace_id: &str,
        artifact_id: &str,
    ) -> Result<DispatcherToolArtifactRecord> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT id, workspace_id, message_id, tool_call_id, tool_name, title, kind, preview, content, char_count, line_count, created_at
             FROM dispatcher_tool_artifacts
             WHERE id = ?1 AND workspace_id = ?2",
            params![artifact_id, workspace_id],
            |row| {
                Ok(DispatcherToolArtifactRecord {
                    id: row.get(0)?,
                    workspace_id: row.get(1)?,
                    message_id: row.get(2)?,
                    tool_call_id: row.get(3)?,
                    tool_name: row.get(4)?,
                    title: row.get(5)?,
                    kind: row.get(6)?,
                    preview: row.get(7)?,
                    content: row.get(8)?,
                    char_count: row.get::<_, i64>(9)? as usize,
                    line_count: row.get::<_, i64>(10)? as usize,
                    created_at: row.get(11)?,
                })
            },
        )
        .optional()
        .context("load dispatcher tool artifact")?
        .with_context(|| format!("dispatcher tool artifact not found: {artifact_id}"))
    }

    pub fn list_python_code_runs(
        &self,
        workspace_id: &str,
        message_id: Option<&str>,
    ) -> Result<Vec<PythonCodeRunRecord>> {
        let conn = self.conn()?;
        let sql = if message_id.is_some() {
            "SELECT run_id, workspace_id, message_id, code_block_index, code_hash, code, status,
                    stdout, stderr, installed_packages_json, tool_events_json, explanation_markdown,
                    error_reason, created_at, updated_at
             FROM python_code_runs
             WHERE workspace_id = ?1 AND message_id = ?2
             ORDER BY code_block_index ASC"
        } else {
            "SELECT run_id, workspace_id, message_id, code_block_index, code_hash, code, status,
                    stdout, stderr, installed_packages_json, tool_events_json, explanation_markdown,
                    error_reason, created_at, updated_at
             FROM python_code_runs
             WHERE workspace_id = ?1
             ORDER BY updated_at DESC, message_id ASC, code_block_index ASC"
        };
        let mut stmt = conn.prepare(sql)?;
        let rows = if let Some(message_id) = message_id {
            stmt.query_map(
                params![workspace_id, message_id],
                map_python_code_run_record,
            )?
        } else {
            stmt.query_map(params![workspace_id], map_python_code_run_record)?
        };

        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("list python code runs")
    }

    pub fn upsert_python_code_run(&self, record: &PythonCodeRunRecord) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO python_code_runs (
                run_id, workspace_id, message_id, code_block_index, code_hash, code, status,
                stdout, stderr, installed_packages_json, tool_events_json, explanation_markdown,
                error_reason, created_at, updated_at
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
             ON CONFLICT(workspace_id, message_id, code_block_index) DO UPDATE SET
                run_id = excluded.run_id,
                code_hash = excluded.code_hash,
                code = excluded.code,
                status = excluded.status,
                stdout = excluded.stdout,
                stderr = excluded.stderr,
                installed_packages_json = excluded.installed_packages_json,
                tool_events_json = excluded.tool_events_json,
                explanation_markdown = excluded.explanation_markdown,
                error_reason = excluded.error_reason,
                updated_at = excluded.updated_at",
            params![
                &record.run_id,
                &record.workspace_id,
                &record.message_id,
                record.code_block_index as i64,
                &record.code_hash,
                &record.code,
                &record.status,
                &record.stdout,
                &record.stderr,
                &record.installed_packages_json,
                &record.tool_events_json,
                &record.explanation_markdown,
                &record.error_reason,
                &record.created_at,
                &record.updated_at,
            ],
        )
        .context("upsert python code run")?;
        Ok(())
    }

    pub fn clear_python_code_run(
        &self,
        workspace_id: &str,
        message_id: &str,
        code_block_index: u32,
    ) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM python_code_runs
             WHERE workspace_id = ?1 AND message_id = ?2 AND code_block_index = ?3",
            params![workspace_id, message_id, code_block_index as i64],
        )
        .context("clear python code run")?;
        Ok(())
    }

    fn add_message(&self, params: NewDispatcherMessage<'_>) -> Result<DispatcherMessageRecord> {
        let tool_calls_json = params
            .tool_calls
            .map(serde_json::to_string)
            .transpose()
            .context("serialize tool calls")?;
        let usage_stats_json = params
            .usage_stats
            .map(serde_json::to_string)
            .transpose()
            .context("serialize dispatcher message usage stats")?;

        let segments_json = params
            .segments_json
            .unwrap_or_else(|| content_to_segments_json(params.content));
        let segments = parse_segments_json(&segments_json);
        let content = segments_to_markdown(&segments);

        let mut record = DispatcherMessageRecord {
            id: Uuid::new_v4().to_string(),
            workspace_id: params.workspace_id.to_string(),
            role: params.role.to_string(),
            content,
            segments_json,
            thinking_content: params
                .thinking_content
                .filter(|content| !content.trim().is_empty())
                .map(|s| s.to_string()),
            thinking_elapsed_ms: params
                .thinking_content
                .filter(|content| !content.trim().is_empty())
                .map(|_| params.thinking_elapsed_ms),
            context_payload: params.context_payload.map(|s| s.to_string()),
            tool_call_id: params.tool_call_id.map(|s| s.to_string()),
            tool_name: params.tool_name.map(|s| s.to_string()),
            tool_result_mode: params.tool_result_mode.map(|s| s.to_string()),
            tool_artifacts: Vec::new(),
            tool_calls_json: tool_calls_json.clone(),
            usage_stats: params.usage_stats.cloned(),
            created_at: now(),
        };

        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE dispatcher_sessions SET updated_at = ?1 WHERE id = ?2",
            params![&record.created_at, &record.workspace_id],
        )?;
        tx.execute(
            "UPDATE chat_sessions SET updated_at = ?1 WHERE id = ?2",
            params![&record.created_at, &record.workspace_id],
        )?;
        tx.execute(
            "UPDATE project_sessions SET updated_at = ?1 WHERE id = ?2",
            params![&record.created_at, &record.workspace_id],
        )?;

        tx.execute(
            "INSERT INTO dispatcher_messages (
                id, workspace_id, role, segments_json, thinking_content, thinking_elapsed_ms, context_payload, tool_call_id, tool_name, tool_result_mode, tool_artifacts_json, tool_calls_json, usage_stats_json, visible, created_at
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                &record.id,
                &record.workspace_id,
                &record.role,
                &record.segments_json,
                &record.thinking_content,
                &record.thinking_elapsed_ms,
                &record.context_payload,
                &record.tool_call_id,
                &record.tool_name,
                &record.tool_result_mode,
                Option::<String>::None,
                &record.tool_calls_json,
                &usage_stats_json,
                if params.visible { 1 } else { 0 },
                &record.created_at
            ],
        )
        .context("insert dispatcher message")?;

        insert_chat_images(
            &tx,
            &record.workspace_id,
            &record.id,
            &segments,
            &record.created_at,
        )?;

        if !params.tool_artifacts.is_empty() {
            let artifacts = self.insert_tool_artifacts(
                &tx,
                &record.workspace_id,
                &record.id,
                record.tool_call_id.as_deref(),
                record.tool_name.as_deref(),
                params.tool_artifacts,
                &record.created_at,
            )?;
            let artifacts_json =
                serde_json::to_string(&artifacts).context("serialize dispatcher tool artifacts")?;
            tx.execute(
                "UPDATE dispatcher_messages SET tool_artifacts_json = ?1 WHERE id = ?2",
                params![&artifacts_json, &record.id],
            )
            .context("attach dispatcher tool artifacts to message")?;
            record.tool_artifacts = artifacts;
        }

        tx.commit().context("commit dispatcher message insert")?;

        Ok(record)
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_tool_artifacts(
        &self,
        tx: &rusqlite::Transaction<'_>,
        workspace_id: &str,
        message_id: &str,
        tool_call_id: Option<&str>,
        tool_name: Option<&str>,
        drafts: &[ToolArtifactDraft],
        created_at: &str,
    ) -> Result<Vec<DispatcherToolArtifactRef>> {
        let mut refs = Vec::with_capacity(drafts.len());

        for draft in drafts {
            let artifact_id = Uuid::new_v4().to_string();
            tx.execute(
                "INSERT INTO dispatcher_tool_artifacts (
                    id, workspace_id, message_id, tool_call_id, tool_name, title, kind, preview, content, char_count, line_count, created_at
                 )
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    &artifact_id,
                    workspace_id,
                    message_id,
                    tool_call_id,
                    tool_name,
                    &draft.title,
                    &draft.kind,
                    &draft.preview,
                    &draft.content,
                    draft.char_count as i64,
                    draft.line_count as i64,
                    created_at,
                ],
            )
            .context("insert dispatcher tool artifact")?;

            refs.push(DispatcherToolArtifactRef {
                id: artifact_id,
                title: draft.title.clone(),
                kind: draft.kind.clone(),
                preview: draft.preview.clone(),
                char_count: draft.char_count,
                line_count: draft.line_count,
                created_at: created_at.to_string(),
            });
        }

        Ok(refs)
    }

    fn init(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create db directory {}", parent.display()))?;
        }
        let mut conn = self.conn()?;

        // Fast path: if schema is already at the expected version, skip all DDL.
        const SCHEMA_VERSION: i32 = 6;
        let current_version: i32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap_or(0);
        if current_version >= SCHEMA_VERSION {
            // Still need to set WAL mode on first open of each connection.
            conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")
                .context("set pragmas")?;
            return Ok(());
        }

        conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA foreign_keys = ON;

            CREATE TABLE IF NOT EXISTS dispatcher_sessions (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                kind TEXT NOT NULL DEFAULT 'project',
                title TEXT NOT NULL,
                mode TEXT NOT NULL DEFAULT 'default',
                active_plan_path TEXT,
                checklist_json TEXT,
                plan_interaction_json TEXT,
                category TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_dispatcher_sessions_project
            ON dispatcher_sessions(project_id, updated_at DESC);

            CREATE TABLE IF NOT EXISTS dispatcher_messages (
                id TEXT PRIMARY KEY,
                workspace_id TEXT NOT NULL,
                role TEXT NOT NULL,
                segments_json TEXT NOT NULL DEFAULT '[]',
                thinking_content TEXT,
                thinking_elapsed_ms INTEGER,
                context_payload TEXT,
                tool_call_id TEXT,
                tool_name TEXT,
                tool_result_mode TEXT,
                tool_artifacts_json TEXT,
                tool_calls_json TEXT,
                usage_stats_json TEXT,
                visible INTEGER NOT NULL DEFAULT 1,
                context_cleared INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_dispatcher_messages_workspace_created
            ON dispatcher_messages(workspace_id, created_at);

            CREATE INDEX IF NOT EXISTS idx_dispatcher_messages_workspace_context_role
            ON dispatcher_messages(workspace_id, context_cleared, role, created_at);

            CREATE TABLE IF NOT EXISTS chat_images (
                id TEXT PRIMARY KEY,
                image_id TEXT NOT NULL UNIQUE,
                workspace_id TEXT NOT NULL,
                message_id TEXT NOT NULL,
                segment_index INTEGER NOT NULL,
                path TEXT NOT NULL,
                alt TEXT,
                width INTEGER,
                height INTEGER,
                mime_type TEXT,
                source TEXT,
                generation_prompt TEXT,
                vector_embedding_json TEXT,
                text_description TEXT,
                created_at TEXT NOT NULL,
                FOREIGN KEY (message_id) REFERENCES dispatcher_messages(id) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_chat_images_workspace
            ON chat_images(workspace_id, created_at);

            CREATE INDEX IF NOT EXISTS idx_chat_images_message
            ON chat_images(message_id);

            CREATE TABLE IF NOT EXISTS dispatcher_tool_artifacts (
                id TEXT PRIMARY KEY,
                workspace_id TEXT NOT NULL,
                message_id TEXT,
                tool_call_id TEXT,
                tool_name TEXT,
                title TEXT NOT NULL,
                kind TEXT NOT NULL,
                preview TEXT NOT NULL DEFAULT '',
                content TEXT NOT NULL,
                char_count INTEGER NOT NULL DEFAULT 0,
                line_count INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_dispatcher_tool_artifacts_workspace_created
            ON dispatcher_tool_artifacts(workspace_id, created_at);

            CREATE INDEX IF NOT EXISTS idx_dispatcher_tool_artifacts_message
            ON dispatcher_tool_artifacts(message_id);

            CREATE TABLE IF NOT EXISTS dispatcher_settings (
                id TEXT PRIMARY KEY DEFAULT 'default',
                api_base TEXT NOT NULL DEFAULT '',
                api_key TEXT NOT NULL DEFAULT '',
                model TEXT NOT NULL DEFAULT '',
                summary_model TEXT NOT NULL DEFAULT 'deepseek-v4-flash',
                vision_model TEXT NOT NULL DEFAULT '',
                asr_api_key TEXT NOT NULL DEFAULT '',
                asr_websocket_url TEXT NOT NULL DEFAULT '',
                auto_approve_dispatch INTEGER NOT NULL DEFAULT 0,
                context_debug INTEGER NOT NULL DEFAULT 0,
                image_model_url TEXT NOT NULL DEFAULT 'https://dashscope.aliyuncs.com/api/v1',
                image_model_api_key TEXT NOT NULL DEFAULT '',
                image_model TEXT NOT NULL DEFAULT 'qwen-image-2.0-pro',
                image_edit_model TEXT NOT NULL DEFAULT '',
                chat_model_url TEXT NOT NULL DEFAULT '',
                chat_model_api_key TEXT NOT NULL DEFAULT '',
                chat_model_name TEXT NOT NULL DEFAULT '',
                summary_model_url TEXT NOT NULL DEFAULT '',
                summary_model_api_key TEXT NOT NULL DEFAULT '',
                summary_model_name TEXT NOT NULL DEFAULT '',
                vision_model_url TEXT NOT NULL DEFAULT '',
                vision_model_api_key TEXT NOT NULL DEFAULT '',
                vision_model_name TEXT NOT NULL DEFAULT '',
                image_model_config_url TEXT NOT NULL DEFAULT 'https://dashscope.aliyuncs.com/api/v1',
                image_model_config_api_key TEXT NOT NULL DEFAULT '',
                image_model_config_name TEXT NOT NULL DEFAULT 'qwen-image-2.0-pro',
                image_edit_model_url TEXT NOT NULL DEFAULT 'https://dashscope.aliyuncs.com/api/v1',
                image_edit_model_api_key TEXT NOT NULL DEFAULT '',
                image_edit_model_name TEXT NOT NULL DEFAULT 'qwen-image-2.0-pro',
                asr_model_url TEXT NOT NULL DEFAULT '',
                asr_model_api_key TEXT NOT NULL DEFAULT '',
                asr_model_name TEXT NOT NULL DEFAULT 'fun-asr-realtime',
                tts_model_url TEXT NOT NULL DEFAULT '',
                tts_model_api_key TEXT NOT NULL DEFAULT '',
                tts_model_name TEXT NOT NULL DEFAULT '',
                embedding_model_url TEXT NOT NULL DEFAULT '',
                embedding_model_api_key TEXT NOT NULL DEFAULT '',
                embedding_model_name TEXT NOT NULL DEFAULT '',
                chat_model_configs_json TEXT NOT NULL DEFAULT '[]',
                summary_model_configs_json TEXT NOT NULL DEFAULT '[]',
                vision_model_configs_json TEXT NOT NULL DEFAULT '[]',
                image_model_configs_json TEXT NOT NULL DEFAULT '[]',
                image_edit_model_configs_json TEXT NOT NULL DEFAULT '[]',
                asr_model_configs_json TEXT NOT NULL DEFAULT '[]',
                tts_model_configs_json TEXT NOT NULL DEFAULT '[]',
                embedding_model_configs_json TEXT NOT NULL DEFAULT '[]'
            );

            CREATE TABLE IF NOT EXISTS dispatcher_session_token_usage (
                workspace_id TEXT NOT NULL,
                model TEXT NOT NULL,
                source_kind TEXT NOT NULL DEFAULT 'primary',
                prompt_tokens INTEGER NOT NULL DEFAULT 0,
                completion_tokens INTEGER NOT NULL DEFAULT 0,
                total_tokens INTEGER NOT NULL DEFAULT 0,
                cached_tokens INTEGER NOT NULL DEFAULT 0,
                context_window_tokens INTEGER NOT NULL DEFAULT 0,
                context_window_capacity INTEGER NOT NULL DEFAULT 1000000,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (workspace_id, model, source_kind)
            );

            CREATE INDEX IF NOT EXISTS idx_dispatcher_session_token_usage_workspace_updated
            ON dispatcher_session_token_usage(workspace_id, updated_at DESC);

            CREATE TABLE IF NOT EXISTS python_code_runs (
                run_id TEXT NOT NULL,
                workspace_id TEXT NOT NULL,
                message_id TEXT NOT NULL,
                code_block_index INTEGER NOT NULL,
                code_hash TEXT NOT NULL,
                code TEXT NOT NULL,
                status TEXT NOT NULL,
                stdout TEXT NOT NULL DEFAULT '',
                stderr TEXT NOT NULL DEFAULT '',
                installed_packages_json TEXT NOT NULL DEFAULT '[]',
                tool_events_json TEXT NOT NULL DEFAULT '[]',
                explanation_markdown TEXT NOT NULL DEFAULT '',
                error_reason TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (workspace_id, message_id, code_block_index),
                FOREIGN KEY (message_id) REFERENCES dispatcher_messages(id) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_python_code_runs_workspace_updated
            ON python_code_runs(workspace_id, updated_at DESC);

            CREATE TABLE IF NOT EXISTS chat_categories (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                icon TEXT NOT NULL DEFAULT '',
                color TEXT NOT NULL DEFAULT '',
                sort_order INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            ",
        )
        .context("initialize dispatcher sqlite schema")?;

        // Run all column additions and data migrations in a single transaction
        // so that a failure midway rolls back cleanly instead of leaving partial schema.
        {
            let tx = conn.transaction().context("begin migration transaction")?;

            migrate_session_token_usage_primary_key_on_tx(&tx)?;
            ensure_column_exists_tx(
                &tx,
                "dispatcher_settings",
                "summary_model",
                "TEXT NOT NULL DEFAULT 'deepseek-v4-flash'",
            )?;
            ensure_column_exists_tx(
                &tx,
                "dispatcher_settings",
                "vision_model",
                "TEXT NOT NULL DEFAULT ''",
            )?;
            ensure_column_exists_tx(
                &tx,
                "dispatcher_settings",
                "asr_api_key",
                "TEXT NOT NULL DEFAULT ''",
            )?;
            ensure_column_exists_tx(
                &tx,
                "dispatcher_settings",
                "asr_websocket_url",
                "TEXT NOT NULL DEFAULT ''",
            )?;
            ensure_column_exists_tx(
                &tx,
                "dispatcher_settings",
                "context_debug",
                "INTEGER NOT NULL DEFAULT 0",
            )?;
            ensure_column_exists_tx(
                &tx,
                "dispatcher_settings",
                "image_model_url",
                "TEXT NOT NULL DEFAULT 'https://dashscope.aliyuncs.com/api/v1'",
            )?;
            ensure_column_exists_tx(
                &tx,
                "dispatcher_settings",
                "image_model_api_key",
                "TEXT NOT NULL DEFAULT ''",
            )?;
            ensure_column_exists_tx(
                &tx,
                "dispatcher_settings",
                "image_model",
                "TEXT NOT NULL DEFAULT 'qwen-image-2.0-pro'",
            )?;
            ensure_column_exists_tx(
                &tx,
                "dispatcher_settings",
                "image_edit_model",
                "TEXT NOT NULL DEFAULT ''",
            )?;
            ensure_dispatcher_model_config_columns_tx(&tx)?;
            migrate_dispatcher_model_configs_tx(&tx)?;
            ensure_column_exists_tx(&tx, "dispatcher_messages", "context_payload", "TEXT")?;
            ensure_column_exists_tx(
                &tx,
                "dispatcher_messages",
                "segments_json",
                "TEXT NOT NULL DEFAULT '[]'",
            )?;
            ensure_column_exists_tx(&tx, "dispatcher_messages", "thinking_content", "TEXT")?;
            ensure_column_exists_tx(&tx, "dispatcher_messages", "thinking_elapsed_ms", "INTEGER")?;
            ensure_column_exists_tx(&tx, "dispatcher_messages", "tool_result_mode", "TEXT")?;
            ensure_column_exists_tx(&tx, "dispatcher_messages", "tool_artifacts_json", "TEXT")?;
            ensure_column_exists_tx(&tx, "dispatcher_messages", "usage_stats_json", "TEXT")?;
            ensure_column_exists_tx(
                &tx,
                "dispatcher_messages",
                "context_cleared",
                "INTEGER NOT NULL DEFAULT 0",
            )?;
            ensure_column_exists_tx(
                &tx,
                "dispatcher_sessions",
                "kind",
                "TEXT NOT NULL DEFAULT 'project'",
            )?;
            tx.execute(
                "CREATE INDEX IF NOT EXISTS idx_dispatcher_sessions_project_kind
                 ON dispatcher_sessions(project_id, kind, updated_at DESC)",
                [],
            )
            .context("create dispatcher session project kind index")?;
            ensure_column_exists_tx(
                &tx,
                "dispatcher_sessions",
                "mode",
                "TEXT NOT NULL DEFAULT 'default'",
            )?;
            ensure_column_exists_tx(&tx, "dispatcher_sessions", "active_plan_path", "TEXT")?;
            ensure_column_exists_tx(&tx, "dispatcher_sessions", "checklist_json", "TEXT")?;
            ensure_column_exists_tx(&tx, "dispatcher_sessions", "plan_interaction_json", "TEXT")?;
            ensure_python_code_runs_table_tx(&tx)?;

            ensure_chat_categories_table_tx(&tx)?;

            ensure_column_exists_tx(
                &tx,
                "dispatcher_sessions",
                "category",
                "TEXT NOT NULL DEFAULT ''",
            )?;

            tx.execute(
                "UPDATE dispatcher_sessions SET category = 'tech'
                 WHERE kind = 'chat' AND (category IS NULL OR category = '')",
                [],
            )
            .context("migrate existing chat sessions to tech category")?;

            tx.commit().context("commit migration transaction")?;
        }

        // v5 → v6: split dispatcher_sessions → chat_sessions + project_sessions,
        // delete all project-kind sessions, keep chat data only.
        if conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='chat_sessions'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap_or(0)
            == 0
        {
            let mut paths = HashSet::new();
            {
                let mut stmt = conn
                    .prepare(
                        "SELECT ci.path FROM chat_images ci
                         INNER JOIN dispatcher_sessions ds ON ci.workspace_id = ds.id
                         WHERE ds.kind = 'project'",
                    )
                    .context("v6: load project image paths")?;
                let indexed = stmt
                    .query_map([], |row| row.get::<_, String>(0))?
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .unwrap_or_default();
                paths.extend(indexed);
            }
            conn.execute_batch(
                "
                CREATE TABLE IF NOT EXISTS chat_sessions (
                    id TEXT PRIMARY KEY,
                    title TEXT NOT NULL,
                    category TEXT NOT NULL DEFAULT 'tech',
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_chat_sessions_updated
                    ON chat_sessions(updated_at DESC);
                CREATE INDEX IF NOT EXISTS idx_chat_sessions_category_updated
                    ON chat_sessions(category, updated_at DESC);

                CREATE TABLE IF NOT EXISTS project_sessions (
                    id TEXT PRIMARY KEY,
                    project_id TEXT NOT NULL,
                    title TEXT NOT NULL,
                    mode TEXT NOT NULL DEFAULT 'default',
                    active_plan_path TEXT,
                    checklist_json TEXT,
                    plan_interaction_json TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_project_sessions_project_updated
                    ON project_sessions(project_id, updated_at DESC);

                INSERT OR IGNORE INTO chat_sessions (id, title, category, created_at, updated_at)
                SELECT id, title, category, created_at, updated_at
                FROM dispatcher_sessions
                WHERE kind = 'chat';

                DELETE FROM dispatcher_tool_artifacts
                    WHERE workspace_id IN (SELECT id FROM dispatcher_sessions WHERE kind = 'project');
                DELETE FROM dispatcher_session_token_usage
                    WHERE workspace_id IN (SELECT id FROM dispatcher_sessions WHERE kind = 'project');
                DELETE FROM python_code_runs
                    WHERE workspace_id IN (SELECT id FROM dispatcher_sessions WHERE kind = 'project');
                DELETE FROM chat_images
                    WHERE workspace_id IN (SELECT id FROM dispatcher_sessions WHERE kind = 'project');
                DELETE FROM dispatcher_messages
                    WHERE workspace_id IN (SELECT id FROM dispatcher_sessions WHERE kind = 'project');
                DELETE FROM dispatcher_sessions WHERE kind = 'project';

                UPDATE chat_sessions SET category = 'tech'
                    WHERE category IS NULL OR category = '';
                ",
            )
            .context("v6 migration: split sessions and delete project-kind data")?;
            for path in paths {
                if let Ok(safe) = safe_absolute_image_path(&path) {
                    let _ = std::fs::remove_file(&safe);
                }
            }
        }

        // Mark schema as fully migrated (outside the transaction — PRAGMA is auto-commit).
        conn.execute_batch(&format!("PRAGMA user_version = {};", SCHEMA_VERSION))
            .context("set user_version")?;

        Ok(())
    }

    fn conn(&self) -> Result<r2d2::PooledConnection<SqliteConnectionManager>> {
        self.pool.get().with_context(|| "获取数据库连接")
    }

    // ── Async wrappers for use in async contexts (spawn_blocking) ──

    pub async fn clear_checklist_async(
        &self,
        workspace_id: &str,
    ) -> Result<DispatcherSessionRuntimeState> {
        let db = self.clone();
        let wid = workspace_id.to_string();
        tokio::task::spawn_blocking(move || db.clear_checklist(&wid))
            .await
            .context("clear_checklist spawn_blocking")?
    }

    pub async fn add_visible_message_async(
        &self,
        workspace_id: &str,
        role: &str,
        content: &str,
        segments_json: Option<String>,
    ) -> Result<DispatcherMessageRecord> {
        let db = self.clone();
        let wid = workspace_id.to_string();
        let role = role.to_string();
        let content = content.to_string();
        tokio::task::spawn_blocking(move || {
            db.add_visible_message(&wid, &role, &content, segments_json)
        })
        .await
        .context("add_visible_message spawn_blocking")?
    }

    pub async fn add_visible_message_with_usage_and_thinking_async(
        &self,
        workspace_id: &str,
        role: &str,
        content: &str,
        usage_stats: &DispatcherMessageUsageStats,
        thinking_content: Option<&str>,
        thinking_elapsed_ms: u64,
    ) -> Result<DispatcherMessageRecord> {
        let db = self.clone();
        let wid = workspace_id.to_string();
        let role = role.to_string();
        let content = content.to_string();
        let usage = usage_stats.clone();
        let thinking = thinking_content.map(str::to_string);
        tokio::task::spawn_blocking(move || {
            db.add_visible_message_with_usage_and_thinking(
                &wid,
                &role,
                &content,
                &usage,
                thinking.as_deref(),
                thinking_elapsed_ms,
            )
        })
        .await
        .context("add_visible_message_with_usage_and_thinking spawn_blocking")?
    }

    pub async fn load_llm_history_async(&self, workspace_id: &str) -> Result<Vec<ChatMessage>> {
        let db = self.clone();
        let wid = workspace_id.to_string();
        tokio::task::spawn_blocking(move || db.load_llm_history(&wid))
            .await
            .context("load_llm_history spawn_blocking")?
    }

    pub async fn get_session_runtime_state_async(
        &self,
        workspace_id: &str,
    ) -> Result<DispatcherSessionRuntimeState> {
        let db = self.clone();
        let wid = workspace_id.to_string();
        tokio::task::spawn_blocking(move || db.get_session_runtime_state(&wid))
            .await
            .context("get_session_runtime_state spawn_blocking")?
    }

    pub async fn get_session_title_async(&self, workspace_id: &str) -> Result<String> {
        let db = self.clone();
        let wid = workspace_id.to_string();
        tokio::task::spawn_blocking(move || db.get_session_title(&wid))
            .await
            .context("get_session_title spawn_blocking")?
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn add_visible_message_with_tools_and_thinking_async(
        &self,
        workspace_id: &str,
        role: &str,
        content: &str,
        tool_call_id: Option<&str>,
        tool_name: Option<&str>,
        tool_result_mode: Option<&str>,
        tool_calls: Option<&[OutboundToolCall]>,
        thinking_content: Option<&str>,
        thinking_elapsed_ms: u64,
    ) -> Result<DispatcherMessageRecord> {
        let db = self.clone();
        let wid = workspace_id.to_string();
        let role = role.to_string();
        let content = content.to_string();
        let tool_call_id = tool_call_id.map(str::to_string);
        let tool_name = tool_name.map(str::to_string);
        let tool_result_mode = tool_result_mode.map(str::to_string);
        let tool_calls = tool_calls.map(|c| c.to_vec());
        let thinking = thinking_content.map(str::to_string);
        tokio::task::spawn_blocking(move || {
            db.add_visible_message_with_tools_and_thinking(
                &wid,
                &role,
                &content,
                tool_call_id.as_deref(),
                tool_name.as_deref(),
                tool_result_mode.as_deref(),
                tool_calls.as_deref(),
                thinking.as_deref(),
                thinking_elapsed_ms,
            )
        })
        .await
        .context("add_visible_message_with_tools_and_thinking spawn_blocking")?
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn add_visible_tool_result_async(
        &self,
        workspace_id: &str,
        content: &str,
        context_payload: &str,
        tool_call_id: Option<&str>,
        tool_name: Option<&str>,
        tool_result_mode: Option<&str>,
        tool_artifacts: &[ToolArtifactDraft],
    ) -> Result<DispatcherMessageRecord> {
        let db = self.clone();
        let wid = workspace_id.to_string();
        let content = content.to_string();
        let context_payload = context_payload.to_string();
        let tool_call_id = tool_call_id.map(str::to_string);
        let tool_name = tool_name.map(str::to_string);
        let tool_result_mode = tool_result_mode.map(str::to_string);
        let artifacts = tool_artifacts.to_vec();
        tokio::task::spawn_blocking(move || {
            db.add_visible_tool_result(
                &wid,
                &content,
                &context_payload,
                tool_call_id.as_deref(),
                tool_name.as_deref(),
                tool_result_mode.as_deref(),
                &artifacts,
            )
        })
        .await
        .context("add_visible_tool_result spawn_blocking")?
    }

    fn find_dialogue_cutoff_rowid(
        &self,
        conn: &Connection,
        workspace_id: &str,
        max_dialogues: usize,
    ) -> Result<i64> {
        let max_dialogues = max_dialogues.clamp(1, MAX_DIALOGUE_QUERY_LIMIT);
        let mut stmt = conn.prepare(
            "SELECT rowid
             FROM dispatcher_messages
             WHERE workspace_id = ?1 AND role = 'user' AND context_cleared = 0
             ORDER BY rowid DESC
             LIMIT ?2",
        )?;
        let rowids = stmt
            .query_map(params![workspace_id, max_dialogues as i64], |row| {
                row.get::<_, i64>(0)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("load dispatcher dialogue boundaries")?;

        Ok(rowids.into_iter().min().unwrap_or(0))
    }
}

impl DispatcherSessionTokenUsageSource {
    fn as_sql_value(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Summary => "summary",
        }
    }

    fn from_sql_value(value: String) -> Self {
        match value.as_str() {
            "summary" => Self::Summary,
            _ => Self::Primary,
        }
    }
}

/// Transaction-scoped variant used from within `init()`'s outer migration transaction.
fn migrate_session_token_usage_primary_key_on_tx(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    let primary_key_columns = table_primary_key_columns(tx, "dispatcher_session_token_usage")?;
    if primary_key_columns
        .iter()
        .map(String::as_str)
        .eq(["workspace_id", "model", "source_kind"])
    {
        return Ok(());
    }

    migrate_session_token_usage_primary_key_inner(tx)
}

fn migrate_session_token_usage_primary_key_inner(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        ALTER TABLE dispatcher_session_token_usage RENAME TO dispatcher_session_token_usage_old;

        CREATE TABLE dispatcher_session_token_usage (
            workspace_id TEXT NOT NULL,
            model TEXT NOT NULL,
            source_kind TEXT NOT NULL DEFAULT 'primary',
            prompt_tokens INTEGER NOT NULL DEFAULT 0,
            completion_tokens INTEGER NOT NULL DEFAULT 0,
            total_tokens INTEGER NOT NULL DEFAULT 0,
            cached_tokens INTEGER NOT NULL DEFAULT 0,
            context_window_tokens INTEGER NOT NULL DEFAULT 0,
            context_window_capacity INTEGER NOT NULL DEFAULT 1000000,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (workspace_id, model, source_kind)
        );

        INSERT INTO dispatcher_session_token_usage (
            workspace_id, model, source_kind, prompt_tokens, completion_tokens, total_tokens,
            cached_tokens, context_window_tokens, context_window_capacity, updated_at
        )
        SELECT
            workspace_id,
            model,
            COALESCE(NULLIF(source_kind, ''), 'primary'),
            prompt_tokens,
            completion_tokens,
            total_tokens,
            cached_tokens,
            context_window_tokens,
            context_window_capacity,
            updated_at
        FROM dispatcher_session_token_usage_old;

        DROP TABLE dispatcher_session_token_usage_old;

        CREATE INDEX IF NOT EXISTS idx_dispatcher_session_token_usage_workspace_updated
        ON dispatcher_session_token_usage(workspace_id, updated_at DESC);
        ",
    )
    .context("migrate dispatcher session token usage primary key")
}

fn table_primary_key_columns(conn: &Connection, table: &str) -> Result<Vec<String>> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .with_context(|| format!("read table info for {table}"))?;
    let columns = stmt
        .query_map([], |row| {
            Ok((row.get::<_, i64>(5)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .with_context(|| format!("collect table info for {table}"))?;

    let mut primary_key_columns = columns
        .into_iter()
        .filter(|(position, _)| *position > 0)
        .collect::<Vec<_>>();
    primary_key_columns.sort_by_key(|(position, _)| *position);
    Ok(primary_key_columns
        .into_iter()
        .map(|(_, name)| name)
        .collect())
}

fn ensure_column_exists(
    conn: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<()> {
    match conn.execute(
        &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
        [],
    ) {
        Ok(_) => Ok(()),
        Err(rusqlite::Error::SqliteFailure(_, Some(message)))
            if message.contains("duplicate column name") =>
        {
            Ok(())
        }
        Err(error) => {
            Err(error).with_context(|| format!("ensure column {column} exists on table {table}"))
        }
    }
}

/// Transaction-scoped variant used from within `init()`'s outer migration transaction.
fn ensure_column_exists_tx(
    tx: &rusqlite::Transaction<'_>,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<()> {
    // Transaction deref's to Connection, so delegate directly.
    ensure_column_exists(tx, table, column, definition)
}

fn ensure_python_code_runs_table_tx(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS python_code_runs (
            run_id TEXT NOT NULL,
            workspace_id TEXT NOT NULL,
            message_id TEXT NOT NULL,
            code_block_index INTEGER NOT NULL,
            code_hash TEXT NOT NULL,
            code TEXT NOT NULL,
            status TEXT NOT NULL,
            stdout TEXT NOT NULL DEFAULT '',
            stderr TEXT NOT NULL DEFAULT '',
            installed_packages_json TEXT NOT NULL DEFAULT '[]',
            tool_events_json TEXT NOT NULL DEFAULT '[]',
            explanation_markdown TEXT NOT NULL DEFAULT '',
            error_reason TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (workspace_id, message_id, code_block_index),
            FOREIGN KEY (message_id) REFERENCES dispatcher_messages(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_python_code_runs_workspace_updated
        ON python_code_runs(workspace_id, updated_at DESC);
        ",
    )
    .context("ensure python code runs table")
}

fn ensure_chat_categories_table_tx(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS chat_categories (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            icon TEXT NOT NULL DEFAULT '',
            color TEXT NOT NULL DEFAULT '',
            sort_order INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        ",
    )
    .context("create chat_categories table")?;

    let ts = now();
    let defaults: Vec<(&str, &str, &str, &str, i32)> = vec![
        ("general", "综合", "MessageSquare", "", 0),
        ("life", "生活", "Heart", "#F43F5E", 1),
        ("work", "工作", "Briefcase", "#3B82F6", 2),
        ("tech", "技术", "Code2", "#8B5CF6", 3),
        ("learning", "学习", "GraduationCap", "#22C55E", 4),
    ];
    let mut stmt = tx
        .prepare(
            "INSERT OR IGNORE INTO chat_categories (id, name, icon, color, sort_order, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )
        .context("prepare seed categories statement")?;
    for (id, name, icon, color, sort_order) in defaults {
        stmt.execute(params![id, name, icon, color, sort_order, &ts, &ts])
            .with_context(|| format!("seed chat category {id}"))?;
    }

    Ok(())
}

fn ensure_dispatcher_model_config_columns(conn: &Connection) -> Result<()> {
    for (column, definition) in [
        ("chat_model_url", "TEXT NOT NULL DEFAULT ''"),
        ("chat_model_api_key", "TEXT NOT NULL DEFAULT ''"),
        ("chat_model_name", "TEXT NOT NULL DEFAULT ''"),
        ("summary_model_url", "TEXT NOT NULL DEFAULT ''"),
        ("summary_model_api_key", "TEXT NOT NULL DEFAULT ''"),
        ("summary_model_name", "TEXT NOT NULL DEFAULT ''"),
        ("vision_model_url", "TEXT NOT NULL DEFAULT ''"),
        ("vision_model_api_key", "TEXT NOT NULL DEFAULT ''"),
        ("vision_model_name", "TEXT NOT NULL DEFAULT ''"),
        (
            "image_model_config_url",
            "TEXT NOT NULL DEFAULT 'https://dashscope.aliyuncs.com/api/v1'",
        ),
        ("image_model_config_api_key", "TEXT NOT NULL DEFAULT ''"),
        (
            "image_model_config_name",
            "TEXT NOT NULL DEFAULT 'qwen-image-2.0-pro'",
        ),
        (
            "image_edit_model_url",
            "TEXT NOT NULL DEFAULT 'https://dashscope.aliyuncs.com/api/v1'",
        ),
        ("image_edit_model_api_key", "TEXT NOT NULL DEFAULT ''"),
        (
            "image_edit_model_name",
            "TEXT NOT NULL DEFAULT 'qwen-image-2.0-pro'",
        ),
        ("asr_model_url", "TEXT NOT NULL DEFAULT ''"),
        ("asr_model_api_key", "TEXT NOT NULL DEFAULT ''"),
        ("asr_model_name", "TEXT NOT NULL DEFAULT 'fun-asr-realtime'"),
        ("tts_model_url", "TEXT NOT NULL DEFAULT ''"),
        ("tts_model_api_key", "TEXT NOT NULL DEFAULT ''"),
        ("tts_model_name", "TEXT NOT NULL DEFAULT ''"),
        ("embedding_model_url", "TEXT NOT NULL DEFAULT ''"),
        ("embedding_model_api_key", "TEXT NOT NULL DEFAULT ''"),
        ("embedding_model_name", "TEXT NOT NULL DEFAULT ''"),
        ("chat_model_configs_json", "TEXT NOT NULL DEFAULT '[]'"),
        ("summary_model_configs_json", "TEXT NOT NULL DEFAULT '[]'"),
        ("vision_model_configs_json", "TEXT NOT NULL DEFAULT '[]'"),
        ("image_model_configs_json", "TEXT NOT NULL DEFAULT '[]'"),
        (
            "image_edit_model_configs_json",
            "TEXT NOT NULL DEFAULT '[]'",
        ),
        ("asr_model_configs_json", "TEXT NOT NULL DEFAULT '[]'"),
        ("tts_model_configs_json", "TEXT NOT NULL DEFAULT '[]'"),
        ("embedding_model_configs_json", "TEXT NOT NULL DEFAULT '[]'"),
    ] {
        ensure_column_exists(conn, "dispatcher_settings", column, definition)?;
    }

    Ok(())
}

/// Transaction-scoped variant used from within `init()`'s outer migration transaction.
fn ensure_dispatcher_model_config_columns_tx(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    ensure_dispatcher_model_config_columns(tx)
}

fn migrate_dispatcher_model_configs(conn: &Connection) -> Result<()> {
    conn.execute(
        "UPDATE dispatcher_settings SET
            chat_model_url = CASE WHEN trim(chat_model_url) = '' THEN api_base ELSE chat_model_url END,
            chat_model_api_key = CASE WHEN trim(chat_model_api_key) = '' THEN api_key ELSE chat_model_api_key END,
            chat_model_name = CASE WHEN trim(chat_model_name) = '' THEN model ELSE chat_model_name END,
            summary_model_url = CASE WHEN trim(summary_model_url) = '' THEN api_base ELSE summary_model_url END,
            summary_model_api_key = CASE WHEN trim(summary_model_api_key) = '' THEN api_key ELSE summary_model_api_key END,
            summary_model_name = CASE WHEN trim(summary_model_name) = '' THEN summary_model ELSE summary_model_name END,
            vision_model_url = CASE WHEN trim(vision_model_url) = '' THEN api_base ELSE vision_model_url END,
            vision_model_api_key = CASE WHEN trim(vision_model_api_key) = '' THEN api_key ELSE vision_model_api_key END,
            vision_model_name = CASE WHEN trim(vision_model_name) = '' THEN vision_model ELSE vision_model_name END,
            image_model_config_url = CASE WHEN trim(image_model_config_url) = '' THEN image_model_url ELSE image_model_config_url END,
            image_model_config_api_key = CASE WHEN trim(image_model_config_api_key) = '' THEN image_model_api_key ELSE image_model_config_api_key END,
            image_model_config_name = CASE WHEN trim(image_model_config_name) = '' THEN image_model ELSE image_model_config_name END,
            image_edit_model_url = CASE WHEN trim(image_edit_model_url) = '' THEN image_model_url ELSE image_edit_model_url END,
            image_edit_model_api_key = CASE WHEN trim(image_edit_model_api_key) = '' THEN image_model_api_key ELSE image_edit_model_api_key END,
            image_edit_model_name = CASE WHEN trim(image_edit_model_name) = '' THEN COALESCE(NULLIF(trim(image_edit_model), ''), image_model) ELSE image_edit_model_name END,
            asr_model_url = CASE WHEN trim(asr_model_url) = '' THEN asr_websocket_url ELSE asr_model_url END,
            asr_model_api_key = CASE WHEN trim(asr_model_api_key) = '' THEN asr_api_key ELSE asr_model_api_key END,
            asr_model_name = CASE WHEN trim(asr_model_name) = '' THEN 'fun-asr-realtime' ELSE asr_model_name END",
        [],
    )
    .context("migrate dispatcher model configs")?;

    conn.execute(
        "UPDATE dispatcher_settings SET
            chat_model_configs_json = CASE WHEN trim(chat_model_configs_json) = '' THEN '[]' ELSE chat_model_configs_json END,
            summary_model_configs_json = CASE WHEN trim(summary_model_configs_json) = '' THEN '[]' ELSE summary_model_configs_json END,
            vision_model_configs_json = CASE WHEN trim(vision_model_configs_json) = '' THEN '[]' ELSE vision_model_configs_json END,
            image_model_configs_json = CASE WHEN trim(image_model_configs_json) = '' THEN '[]' ELSE image_model_configs_json END,
            image_edit_model_configs_json = CASE WHEN trim(image_edit_model_configs_json) = '' THEN '[]' ELSE image_edit_model_configs_json END,
            asr_model_configs_json = CASE WHEN trim(asr_model_configs_json) = '' THEN '[]' ELSE asr_model_configs_json END,
            tts_model_configs_json = CASE WHEN trim(tts_model_configs_json) = '' THEN '[]' ELSE tts_model_configs_json END,
            embedding_model_configs_json = CASE WHEN trim(embedding_model_configs_json) = '' THEN '[]' ELSE embedding_model_configs_json END",
        [],
    )
    .context("migrate dispatcher model config lists")?;

    Ok(())
}

/// Transaction-scoped variant used from within `init()`'s outer migration transaction.
fn migrate_dispatcher_model_configs_tx(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    migrate_dispatcher_model_configs(tx)
}

fn now() -> String {
    Utc::now().to_rfc3339()
}

fn default_context_window_capacity(_model: &str) -> u64 {
    DEFAULT_CONTEXT_WINDOW_CAPACITY_TOKENS
}

fn usage_total_tokens(usage: &LlmUsage) -> u64 {
    if usage.total_tokens > 0 {
        usage.total_tokens
    } else {
        usage.prompt_tokens + usage.completion_tokens
    }
}

fn normalize_summary_model(summary_model: &str) -> String {
    let trimmed = summary_model.trim();
    if trimmed.is_empty() {
        DEFAULT_SUMMARY_MODEL.to_string()
    } else {
        trimmed.to_string()
    }
}

fn latest_user_message_rowid(conn: &Connection, workspace_id: &str) -> Result<i64> {
    conn.query_row(
        "SELECT COALESCE(MAX(rowid), 0)
         FROM dispatcher_messages
         WHERE workspace_id = ?1 AND role = 'user'",
        params![workspace_id],
        |row| row.get(0),
    )
    .context("load latest dispatcher user message rowid")
}

fn map_dispatcher_message_record(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<DispatcherMessageRecord> {
    let segments_json: String = row.get(3)?;
    let content = segments_to_markdown(&parse_segments_json(&segments_json));
    Ok(DispatcherMessageRecord {
        id: row.get(0)?,
        workspace_id: row.get(1)?,
        role: row.get(2)?,
        content,
        segments_json,
        thinking_content: row.get(4)?,
        thinking_elapsed_ms: row.get(5)?,
        context_payload: row.get(6)?,
        tool_call_id: row.get(7)?,
        tool_name: row.get(8)?,
        tool_result_mode: row.get(9)?,
        tool_artifacts: parse_tool_artifact_refs(row.get::<_, Option<String>>(10)?),
        tool_calls_json: row.get(11)?,
        usage_stats: parse_message_usage_stats(row.get::<_, Option<String>>(12)?)?,
        created_at: row.get(13)?,
    })
}

fn map_python_code_run_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<PythonCodeRunRecord> {
    Ok(PythonCodeRunRecord {
        run_id: row.get(0)?,
        workspace_id: row.get(1)?,
        message_id: row.get(2)?,
        code_block_index: row.get::<_, i64>(3)? as u32,
        code_hash: row.get(4)?,
        code: row.get(5)?,
        status: row.get(6)?,
        stdout: row.get(7)?,
        stderr: row.get(8)?,
        installed_packages_json: row.get(9)?,
        tool_events_json: row.get(10)?,
        explanation_markdown: row.get(11)?,
        error_reason: row.get(12)?,
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
    })
}

fn parse_tool_artifact_refs(raw: Option<String>) -> Vec<DispatcherToolArtifactRef> {
    raw.as_deref()
        .and_then(|json| serde_json::from_str::<Vec<DispatcherToolArtifactRef>>(json).ok())
        .unwrap_or_default()
}

fn parse_message_usage_stats(
    raw: Option<String>,
) -> rusqlite::Result<Option<DispatcherMessageUsageStats>> {
    raw.as_deref()
        .filter(|json| !json.trim().is_empty())
        .map(|json| {
            serde_json::from_str::<DispatcherMessageUsageStats>(json).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(10, Type::Text, Box::new(error))
            })
        })
        .transpose()
}

fn parse_optional_json<T>(raw: Option<String>, column: &str) -> Result<Option<T>>
where
    T: for<'de> Deserialize<'de>,
{
    raw.as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(|value| {
            serde_json::from_str::<T>(value)
                .with_context(|| format!("parse dispatcher session {column}"))
        })
        .transpose()
}

fn should_keep_llm_message(message: &ChatMessage) -> bool {
    match message.role.as_str() {
        "assistant" => {
            !is_process_only_assistant_message(&message.content)
                && !is_process_only_assistant_tool_call(message)
        }
        "tool" => !message
            .name
            .as_deref()
            .is_some_and(is_dispatch_plumbing_tool_name),
        _ => true,
    }
}

fn is_process_only_assistant_message(content: &str) -> bool {
    let trimmed = content.trim();
    matches!(
        trimmed,
        "🔄 子任务当前轮次已完成"
            | "✅ 子任务进程已结束"
            | "⚠️ 子任务进程已失败退出"
            | "⏹️ 子任务进程已取消"
            | "🔄 子任务当前轮次已完成，执行结果已同步供后续分析。"
            | "✅ 子任务进程已结束，执行结果已同步供后续分析。"
            | "⚠️ 子任务进程已失败退出，执行结果已同步供后续分析。"
            | "⏹️ 子任务进程已取消，执行结果已同步供后续分析。"
    ) || trimmed.starts_with("📋 已自动批准 ")
        || content.starts_with("📋 已提交 ")
        || content.starts_with("📨 已向 ")
        || content.starts_with("⏹️ 已向 ")
}

fn is_process_only_assistant_tool_call(message: &ChatMessage) -> bool {
    message
        .tool_calls
        .as_ref()
        .is_some_and(|calls| !calls.is_empty() && calls.iter().all(is_dispatch_plumbing_tool_call))
}

fn is_dispatch_plumbing_tool_call(call: &OutboundToolCall) -> bool {
    is_dispatch_plumbing_tool_name(&call.function.name)
}

fn is_dispatch_plumbing_tool_name(name: &str) -> bool {
    matches!(
        name,
        "dispatch_claude"
            | "dispatch_codex"
            | "continue_claude_session"
            | "continue_codex_session"
            | "exit_claude_session"
            | "exit_codex_session"
    )
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::{
        ChecklistPlanItem, ChecklistPlanState, ChecklistStepStatus, DispatcherDb,
        DispatcherMessageUsageStats, DispatcherMode, DispatcherModelConfig, DispatcherSessionKind,
        DispatcherSessionTokenUsageSource, DispatcherSettingsModelConfigs, ModelSlot,
        PlanInteraction, PlanQuestionOption, ToolArtifactDraft, DEFAULT_ASR_MODEL,
        DEFAULT_IMAGE_MODEL, DEFAULT_IMAGE_MODEL_URL, SETTINGS_LEGACY_COLUMNS,
    };
    use crate::agent::config::DEFAULT_SUMMARY_MODEL;
    use crate::agent::llm::{FunctionCall, LlmPromptTokensDetails, LlmUsage, OutboundToolCall};

    fn create_test_db() -> (DispatcherDb, PathBuf) {
        let root =
            std::env::temp_dir().join(format!("jkcodingagent-db-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).expect("create temp db root");
        let db = DispatcherDb::new(root.join("dispatcher.sqlite3")).expect("create dispatcher db");
        (db, root)
    }

    fn cleanup_test_db(root: PathBuf) {
        let _ = fs::remove_dir_all(root);
    }

    fn sample_artifact() -> ToolArtifactDraft {
        ToolArtifactDraft {
            kind: "tool_raw_output".to_string(),
            title: "exec 原始结果".to_string(),
            preview: "line 1 / line 2".to_string(),
            content: "line 1\nline 2".to_string(),
            char_count: 13,
            line_count: 2,
        }
    }

    fn sample_usage(prompt_tokens: u64, completion_tokens: u64, cached_tokens: u64) -> LlmUsage {
        LlmUsage {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens + completion_tokens,
            prompt_tokens_details: Some(LlmPromptTokensDetails { cached_tokens }),
        }
    }

    fn sample_message_usage_stats() -> DispatcherMessageUsageStats {
        DispatcherMessageUsageStats {
            prompt_tokens: 120,
            completion_tokens: 45,
            total_tokens: 165,
            elapsed_ms: 12_345,
        }
    }

    #[test]
    fn settings_column_offsets_match_generated_select_columns() {
        let columns = super::settings_select_columns();
        assert_eq!(
            columns.len(),
            SETTINGS_LEGACY_COLUMNS.len() + ModelSlot::ALL.len() * 4
        );
        assert_eq!(
            columns[super::settings_column_index("auto_approve_dispatch")],
            "auto_approve_dispatch"
        );
        assert_eq!(
            columns[super::settings_column_index("context_debug")],
            "context_debug"
        );

        for slot in ModelSlot::ALL {
            let slot_columns = slot.columns();
            let config_col = slot.config_col_offset();
            assert_eq!(columns[config_col], slot_columns.url);
            assert_eq!(columns[config_col + 1], slot_columns.api_key);
            assert_eq!(columns[config_col + 2], slot_columns.model);
            assert_eq!(columns[slot.json_col()], slot_columns.json);
        }
    }

    #[test]
    fn save_settings_maps_legacy_fields_into_model_configs() {
        let (db, root) = create_test_db();

        let saved = db
            .save_settings_with_model_configs(
                " https://chat.example.com/v1 ",
                " sk-chat ",
                " chat-main ",
                " ",
                " vision-main ",
                " sk-asr ",
                " wss://asr.example.com/ws ",
                true,
                true,
                " https://image.example.com/api/v1 ",
                " sk-image ",
                " image-gen ",
                "",
                DispatcherSettingsModelConfigs::default(),
            )
            .unwrap();

        assert_eq!(saved.chat_model_config.url, "https://chat.example.com/v1");
        assert_eq!(saved.chat_model_config.api_key, "sk-chat");
        assert_eq!(saved.chat_model_config.model, "chat-main");
        assert_eq!(saved.summary_model_config.model, DEFAULT_SUMMARY_MODEL);
        assert_eq!(saved.vision_model_config.model, "vision-main");
        assert_eq!(saved.asr_model_config.url, "wss://asr.example.com/ws");
        assert_eq!(saved.asr_model_config.api_key, "sk-asr");
        assert_eq!(saved.asr_model_config.model, DEFAULT_ASR_MODEL);
        assert_eq!(saved.image_edit_model_config.model, "image-gen");
        assert_eq!(saved.image_edit_model, "image-gen");

        let loaded = db.get_settings().unwrap().unwrap();
        assert_eq!(loaded.chat_model_config, saved.chat_model_config);
        assert_eq!(loaded.image_edit_model_config.model, "image-gen");

        cleanup_test_db(root);
    }

    #[test]
    fn save_settings_with_structured_configs_preserves_legacy_compat_fields() {
        let (db, root) = create_test_db();

        let saved = db
            .save_settings_with_model_configs(
                "",
                "",
                "",
                "",
                "",
                "",
                "",
                false,
                false,
                "",
                "",
                "",
                "",
                DispatcherSettingsModelConfigs {
                    chat_model_config: Some(DispatcherModelConfig::new(
                        "https://chat.example.com/v1",
                        "sk-chat",
                        "chat-main",
                    )),
                    summary_model_config: Some(DispatcherModelConfig::new(
                        "https://summary.example.com/v1",
                        "sk-summary",
                        "",
                    )),
                    vision_model_config: Some(DispatcherModelConfig::new(
                        "https://vision.example.com/v1",
                        "sk-vision",
                        "vision-main",
                    )),
                    image_model_config: Some(DispatcherModelConfig::new(
                        "https://image.example.com/api/v1",
                        "sk-image",
                        "image-gen",
                    )),
                    image_edit_model_config: Some(DispatcherModelConfig::new("", "", "")),
                    asr_model_config: Some(DispatcherModelConfig::new(
                        "wss://asr.example.com/ws",
                        "sk-asr",
                        "",
                    )),
                    tts_model_config: Some(DispatcherModelConfig::new(
                        "https://tts.example.com/v1",
                        "sk-tts",
                        "tts-main",
                    )),
                    embedding_model_config: Some(DispatcherModelConfig::new(
                        "https://embed.example.com/v1",
                        "sk-embed",
                        "embed-main",
                    )),
                    ..Default::default()
                },
            )
            .unwrap();

        assert_eq!(saved.api_base, "https://chat.example.com/v1");
        assert_eq!(saved.api_key, "sk-chat");
        assert_eq!(saved.model, "chat-main");
        assert_eq!(saved.summary_model, DEFAULT_SUMMARY_MODEL);
        assert_eq!(saved.vision_model, "vision-main");
        assert_eq!(saved.image_model_url, "https://image.example.com/api/v1");
        assert_eq!(saved.image_model_api_key, "sk-image");
        assert_eq!(saved.image_model, "image-gen");
        assert_eq!(saved.image_edit_model, "image-gen");
        assert_eq!(saved.asr_websocket_url, "wss://asr.example.com/ws");
        assert_eq!(saved.asr_api_key, "sk-asr");
        assert_eq!(saved.asr_model_config.model, DEFAULT_ASR_MODEL);
        assert_eq!(saved.tts_model_config.model, "tts-main");
        assert_eq!(saved.embedding_model_config.model, "embed-main");

        cleanup_test_db(root);
    }

    #[test]
    fn save_settings_uses_active_provider_from_model_config_lists() {
        let (db, root) = create_test_db();

        let saved = db
            .save_settings_with_model_configs(
                "",
                "",
                "",
                "",
                "",
                "",
                "",
                false,
                false,
                "",
                "",
                "",
                "",
                DispatcherSettingsModelConfigs {
                    chat_model_configs: Some(vec![
                        DispatcherModelConfig {
                            url: "https://chat-a.example.com/v1".to_string(),
                            api_key: "sk-a".to_string(),
                            model: "chat-a".to_string(),
                            active: false,
                        },
                        DispatcherModelConfig {
                            url: "https://chat-b.example.com/v1".to_string(),
                            api_key: "sk-b".to_string(),
                            model: "chat-b".to_string(),
                            active: true,
                        },
                    ]),
                    image_model_configs: Some(vec![DispatcherModelConfig::new(
                        "https://image.example.com/api/v1",
                        "sk-image",
                        "image-gen",
                    )]),
                    ..Default::default()
                },
            )
            .unwrap();

        assert_eq!(saved.api_base, "https://chat-b.example.com/v1");
        assert_eq!(saved.api_key, "sk-b");
        assert_eq!(saved.model, "chat-b");
        assert_eq!(saved.chat_model_config.model, "chat-b");
        assert_eq!(saved.chat_model_configs.len(), 2);
        assert!(!saved.chat_model_configs[0].active);
        assert!(saved.chat_model_configs[1].active);

        let loaded = db.get_settings().unwrap().unwrap();
        assert_eq!(loaded.model, "chat-b");
        assert_eq!(loaded.chat_model_configs.len(), 2);
        assert!(loaded.chat_model_configs[1].active);

        cleanup_test_db(root);
    }

    #[test]
    fn model_config_lists_activate_first_entry_when_none_is_active() {
        let (db, root) = create_test_db();

        let saved = db
            .save_settings_with_model_configs(
                "",
                "",
                "",
                "",
                "",
                "",
                "",
                false,
                false,
                "",
                "",
                "",
                "",
                DispatcherSettingsModelConfigs {
                    chat_model_configs: Some(vec![
                        DispatcherModelConfig {
                            url: "https://chat-a.example.com/v1".to_string(),
                            api_key: "sk-a".to_string(),
                            model: "chat-a".to_string(),
                            active: false,
                        },
                        DispatcherModelConfig {
                            url: "https://chat-b.example.com/v1".to_string(),
                            api_key: "sk-b".to_string(),
                            model: "chat-b".to_string(),
                            active: false,
                        },
                    ]),
                    ..Default::default()
                },
            )
            .unwrap();

        assert_eq!(saved.model, "chat-a");
        assert!(saved.chat_model_configs[0].active);
        assert!(!saved.chat_model_configs[1].active);

        cleanup_test_db(root);
    }

    #[test]
    fn image_edit_defaults_use_image_config_after_image_defaults() {
        let (db, root) = create_test_db();

        let saved = db
            .save_settings_with_model_configs(
                "",
                "",
                "",
                "",
                "",
                "",
                "",
                false,
                false,
                "",
                "",
                "",
                "",
                DispatcherSettingsModelConfigs {
                    image_model_config: Some(DispatcherModelConfig::new("", "sk-image", "")),
                    image_edit_model_config: Some(DispatcherModelConfig::new("", "", "")),
                    ..Default::default()
                },
            )
            .unwrap();

        assert_eq!(saved.image_model_config.url, DEFAULT_IMAGE_MODEL_URL);
        assert_eq!(saved.image_model_config.model, DEFAULT_IMAGE_MODEL);
        assert_eq!(saved.image_edit_model_config.url, DEFAULT_IMAGE_MODEL_URL);
        assert_eq!(saved.image_edit_model_config.api_key, "sk-image");
        assert_eq!(saved.image_edit_model_config.model, DEFAULT_IMAGE_MODEL);

        cleanup_test_db(root);
    }

    #[test]
    fn update_session_title_persists_latest_title() {
        let (db, root) = create_test_db();
        let session = db
            .create_session(
                "project-1",
                "新会话",
                DispatcherSessionKind::Project,
                DispatcherMode::Default,
                None,
                None,
            )
            .unwrap();

        let updated = db
            .update_session_title(&session.id, "修复会话命名")
            .unwrap()
            .expect("session should exist");

        assert_eq!(updated.title, "修复会话命名");
        assert_eq!(updated.project_id, "project-1");
        assert!(updated.updated_at >= session.updated_at);
        assert_eq!(
            db.list_sessions("project-1", DispatcherSessionKind::Project)
                .unwrap()[0]
                .title,
            "修复会话命名"
        );

        cleanup_test_db(root);
    }

    #[test]
    fn session_kind_isolates_project_and_plain_chat_sessions() {
        let (db, root) = create_test_db();
        db.create_session(
            "project-1",
            "项目会话",
            DispatcherSessionKind::Project,
            DispatcherMode::Default,
            None,
            None,
        )
        .unwrap();
        db.create_session(
            "__global_chat__",
            "普通聊天",
            DispatcherSessionKind::Chat,
            DispatcherMode::Default,
            None,
            None,
        )
        .unwrap();

        let project_sessions = db
            .list_sessions("project-1", DispatcherSessionKind::Project)
            .unwrap();
        let chat_sessions = db
            .list_sessions("__global_chat__", DispatcherSessionKind::Chat)
            .unwrap();

        assert_eq!(project_sessions.len(), 1);
        assert_eq!(project_sessions[0].kind, DispatcherSessionKind::Project);
        assert_eq!(project_sessions[0].title, "项目会话");
        assert_eq!(chat_sessions.len(), 1);
        assert_eq!(chat_sessions[0].kind, DispatcherSessionKind::Chat);
        assert_eq!(chat_sessions[0].title, "普通聊天");
        assert!(db
            .list_sessions("project-1", DispatcherSessionKind::Chat)
            .unwrap()
            .is_empty());

        cleanup_test_db(root);
    }

    #[test]
    fn session_kind_migration_defaults_legacy_rows_to_project() {
        let root =
            std::env::temp_dir().join(format!("jkcodingagent-db-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).expect("create temp db root");
        let path = root.join("dispatcher.sqlite3");
        let conn = rusqlite::Connection::open(&path).expect("open legacy db");
        conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA foreign_keys = ON;
            CREATE TABLE dispatcher_sessions (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                kind TEXT NOT NULL DEFAULT 'project',
                title TEXT NOT NULL,
                mode TEXT NOT NULL DEFAULT 'default',
                active_plan_path TEXT,
                checklist_json TEXT,
                plan_interaction_json TEXT,
                category TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE TABLE dispatcher_messages (
                id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL, role TEXT NOT NULL,
                segments_json TEXT NOT NULL DEFAULT '[]',
                thinking_content TEXT, thinking_elapsed_ms INTEGER,
                context_payload TEXT, tool_call_id TEXT, tool_name TEXT,
                tool_result_mode TEXT, tool_artifacts_json TEXT, tool_calls_json TEXT,
                usage_stats_json TEXT,
                visible INTEGER NOT NULL DEFAULT 1,
                context_cleared INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL
            );
            CREATE TABLE dispatcher_tool_artifacts (
                id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL,
                message_id TEXT, tool_call_id TEXT, tool_name TEXT,
                title TEXT NOT NULL, kind TEXT NOT NULL,
                preview TEXT NOT NULL DEFAULT '', content TEXT NOT NULL,
                char_count INTEGER NOT NULL DEFAULT 0, line_count INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL
            );
            CREATE TABLE dispatcher_session_token_usage (
                workspace_id TEXT NOT NULL, model TEXT NOT NULL,
                source_kind TEXT NOT NULL DEFAULT 'primary',
                prompt_tokens INTEGER NOT NULL DEFAULT 0,
                completion_tokens INTEGER NOT NULL DEFAULT 0,
                total_tokens INTEGER NOT NULL DEFAULT 0,
                cached_tokens INTEGER NOT NULL DEFAULT 0,
                context_window_tokens INTEGER NOT NULL DEFAULT 0,
                context_window_capacity INTEGER NOT NULL DEFAULT 1000000,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (workspace_id, model, source_kind)
            );
            CREATE TABLE chat_images (
                id TEXT PRIMARY KEY, image_id TEXT NOT NULL UNIQUE,
                workspace_id TEXT NOT NULL, message_id TEXT NOT NULL,
                segment_index INTEGER NOT NULL, path TEXT NOT NULL,
                alt TEXT, width INTEGER, height INTEGER, mime_type TEXT,
                source TEXT, generation_prompt TEXT, vector_embedding_json TEXT,
                text_description TEXT, created_at TEXT NOT NULL,
                FOREIGN KEY (message_id) REFERENCES dispatcher_messages(id) ON DELETE CASCADE
            );
            CREATE TABLE python_code_runs (
                run_id TEXT NOT NULL, workspace_id TEXT NOT NULL,
                message_id TEXT NOT NULL, code_block_index INTEGER NOT NULL,
                code_hash TEXT NOT NULL, code TEXT NOT NULL,
                status TEXT NOT NULL, stdout TEXT NOT NULL DEFAULT '',
                stderr TEXT NOT NULL DEFAULT '',
                installed_packages_json TEXT NOT NULL DEFAULT '[]',
                tool_events_json TEXT NOT NULL DEFAULT '[]',
                explanation_markdown TEXT NOT NULL DEFAULT '',
                error_reason TEXT,
                created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
                PRIMARY KEY (workspace_id, message_id, code_block_index),
                FOREIGN KEY (message_id) REFERENCES dispatcher_messages(id) ON DELETE CASCADE
            );
            CREATE TABLE chat_categories (
                id TEXT PRIMARY KEY, name TEXT NOT NULL UNIQUE,
                icon TEXT NOT NULL DEFAULT '', color TEXT NOT NULL DEFAULT '',
                sort_order INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL, updated_at TEXT NOT NULL
            );
            CREATE TABLE dispatcher_settings (
                id TEXT PRIMARY KEY DEFAULT 'default',
                api_base TEXT NOT NULL DEFAULT '',
                api_key TEXT NOT NULL DEFAULT '',
                model TEXT NOT NULL DEFAULT '',
                summary_model TEXT NOT NULL DEFAULT 'deepseek-v4-flash',
                vision_model TEXT NOT NULL DEFAULT '',
                asr_api_key TEXT NOT NULL DEFAULT '',
                asr_websocket_url TEXT NOT NULL DEFAULT '',
                auto_approve_dispatch INTEGER NOT NULL DEFAULT 0,
                context_debug INTEGER NOT NULL DEFAULT 0,
                image_model_url TEXT NOT NULL DEFAULT '',
                image_model_api_key TEXT NOT NULL DEFAULT '',
                image_model TEXT NOT NULL DEFAULT '',
                image_edit_model TEXT NOT NULL DEFAULT '',
                chat_model_url TEXT NOT NULL DEFAULT '',
                chat_model_api_key TEXT NOT NULL DEFAULT '',
                chat_model_name TEXT NOT NULL DEFAULT '',
                summary_model_url TEXT NOT NULL DEFAULT '',
                summary_model_api_key TEXT NOT NULL DEFAULT '',
                summary_model_name TEXT NOT NULL DEFAULT '',
                vision_model_url TEXT NOT NULL DEFAULT '',
                vision_model_api_key TEXT NOT NULL DEFAULT '',
                vision_model_name TEXT NOT NULL DEFAULT '',
                image_model_config_url TEXT NOT NULL DEFAULT '',
                image_model_config_api_key TEXT NOT NULL DEFAULT '',
                image_model_config_name TEXT NOT NULL DEFAULT '',
                image_edit_model_url TEXT NOT NULL DEFAULT '',
                image_edit_model_api_key TEXT NOT NULL DEFAULT '',
                image_edit_model_name TEXT NOT NULL DEFAULT '',
                asr_model_url TEXT NOT NULL DEFAULT '',
                asr_model_api_key TEXT NOT NULL DEFAULT '',
                asr_model_name TEXT NOT NULL DEFAULT '',
                tts_model_url TEXT NOT NULL DEFAULT '',
                tts_model_api_key TEXT NOT NULL DEFAULT '',
                tts_model_name TEXT NOT NULL DEFAULT '',
                embedding_model_url TEXT NOT NULL DEFAULT '',
                embedding_model_api_key TEXT NOT NULL DEFAULT '',
                embedding_model_name TEXT NOT NULL DEFAULT '',
                chat_model_configs_json TEXT NOT NULL DEFAULT '[]',
                summary_model_configs_json TEXT NOT NULL DEFAULT '[]',
                vision_model_configs_json TEXT NOT NULL DEFAULT '[]',
                image_model_configs_json TEXT NOT NULL DEFAULT '[]',
                image_edit_model_configs_json TEXT NOT NULL DEFAULT '[]',
                asr_model_configs_json TEXT NOT NULL DEFAULT '[]',
                tts_model_configs_json TEXT NOT NULL DEFAULT '[]',
                embedding_model_configs_json TEXT NOT NULL DEFAULT '[]'
            );
            INSERT INTO dispatcher_sessions (
                id, project_id, kind, title, created_at, updated_at
            ) VALUES (
                'legacy-project', 'project-1', 'project', '旧项目会话',
                '2026-05-09T00:00:00Z', '2026-05-09T00:00:00Z'
            );
            INSERT INTO dispatcher_sessions (
                id, project_id, kind, title, category, created_at, updated_at
            ) VALUES (
                'legacy-chat', '__global_chat__', 'chat', '旧聊天',
                'tech',
                '2026-05-09T00:00:00Z', '2026-05-09T00:00:00Z'
            );
            ",
        )
        .expect("create legacy dispatcher sessions table");
        drop(conn);

        let db = DispatcherDb::new(path).expect("migrate dispatcher db");

        // v6 migration: project sessions deleted from dispatcher_sessions
        let project_sessions = db
            .list_sessions("project-1", DispatcherSessionKind::Project)
            .unwrap();
        assert_eq!(
            project_sessions.len(),
            0,
            "v6 should delete all project-kind rows from dispatcher_sessions"
        );

        let chat_sessions = db
            .list_sessions("__global_chat__", DispatcherSessionKind::Chat)
            .unwrap();
        assert_eq!(chat_sessions.len(), 1);
        assert_eq!(chat_sessions[0].id, "legacy-chat");

        // chat_sessions table should contain the migrated chat session
        let paginated = db
            .list_chat_sessions_paginated(None, None, 10)
            .unwrap();
        assert_eq!(paginated.items.len(), 1);
        assert_eq!(paginated.items[0].id, "legacy-chat");
        assert_eq!(paginated.items[0].category, "tech");

        cleanup_test_db(root);
    }

    #[test]
    fn session_token_usage_accumulates_per_model_and_source() {
        let (db, root) = create_test_db();
        let workspace_id = "session-usage";

        db.upsert_session_token_usage(
            workspace_id,
            "deepseek-chat",
            DispatcherSessionTokenUsageSource::Primary,
            &sample_usage(100, 20, 12),
        )
        .unwrap();
        let primary = db
            .upsert_session_token_usage(
                workspace_id,
                "deepseek-chat",
                DispatcherSessionTokenUsageSource::Primary,
                &sample_usage(80, 15, 8),
            )
            .unwrap();
        db.upsert_session_token_usage(
            workspace_id,
            "deepseek-chat",
            DispatcherSessionTokenUsageSource::Summary,
            &sample_usage(30, 5, 3),
        )
        .unwrap();

        assert_eq!(primary.prompt_tokens, 180);
        assert_eq!(primary.completion_tokens, 35);
        assert_eq!(primary.total_tokens, 215);
        assert_eq!(primary.cached_tokens, 20);
        assert_eq!(primary.context_window_tokens, 80);

        let entries = db.list_session_token_usage(workspace_id).unwrap();
        assert_eq!(entries.len(), 2);
        let summary = entries
            .iter()
            .find(|entry| entry.source_kind == DispatcherSessionTokenUsageSource::Summary)
            .expect("summary usage should be tracked separately");
        assert_eq!(summary.prompt_tokens, 30);
        assert_eq!(summary.total_tokens, 35);

        cleanup_test_db(root);
    }

    #[test]
    fn session_token_usage_derives_total_when_provider_omits_it() {
        let (db, root) = create_test_db();
        let usage = LlmUsage {
            prompt_tokens: 12,
            completion_tokens: 3,
            total_tokens: 0,
            prompt_tokens_details: None,
        };

        let record = db
            .upsert_session_token_usage(
                "session-usage",
                "minimal-compat-model",
                DispatcherSessionTokenUsageSource::Primary,
                &usage,
            )
            .unwrap();

        assert_eq!(record.total_tokens, 15);

        cleanup_test_db(root);
    }

    #[test]
    fn session_token_usage_migration_preserves_existing_rows() {
        let root =
            std::env::temp_dir().join(format!("jkcodingagent-db-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).expect("create temp db root");
        let path = root.join("dispatcher.sqlite3");
        let conn = rusqlite::Connection::open(&path).expect("open legacy db");
        conn.execute_batch(
            "
            CREATE TABLE dispatcher_session_token_usage (
                workspace_id TEXT NOT NULL,
                model TEXT NOT NULL,
                source_kind TEXT NOT NULL DEFAULT 'primary',
                prompt_tokens INTEGER NOT NULL DEFAULT 0,
                completion_tokens INTEGER NOT NULL DEFAULT 0,
                total_tokens INTEGER NOT NULL DEFAULT 0,
                cached_tokens INTEGER NOT NULL DEFAULT 0,
                context_window_tokens INTEGER NOT NULL DEFAULT 0,
                context_window_capacity INTEGER NOT NULL DEFAULT 1000000,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (workspace_id, model)
            );
            INSERT INTO dispatcher_session_token_usage (
                workspace_id, model, source_kind, prompt_tokens, completion_tokens, total_tokens,
                cached_tokens, context_window_tokens, context_window_capacity, updated_at
            ) VALUES (
                'session-usage', 'same-model', 'primary', 100, 20, 120, 10, 100, 1000000,
                '2026-05-09T00:00:00Z'
            );
            ",
        )
        .expect("create legacy token usage table");
        drop(conn);

        let db = DispatcherDb::new(path).expect("migrate dispatcher db");
        db.upsert_session_token_usage(
            "session-usage",
            "same-model",
            DispatcherSessionTokenUsageSource::Summary,
            &sample_usage(30, 5, 3),
        )
        .unwrap();

        let entries = db.list_session_token_usage("session-usage").unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().any(|entry| entry.source_kind
            == DispatcherSessionTokenUsageSource::Primary
            && entry.prompt_tokens == 100));
        assert!(entries.iter().any(|entry| entry.source_kind
            == DispatcherSessionTokenUsageSource::Summary
            && entry.prompt_tokens == 30));

        cleanup_test_db(root);
    }

    #[test]
    fn session_runtime_state_defaults_to_default_mode() {
        let (db, root) = create_test_db();
        let session = db
            .create_session(
                "project-1",
                "新会话",
                DispatcherSessionKind::Project,
                DispatcherMode::Default,
                None,
                None,
            )
            .unwrap();

        let state = db.get_session_runtime_state(&session.id).unwrap();
        assert_eq!(state.mode, DispatcherMode::Default);
        assert!(state.checklist.is_none());
        assert!(state.plan_interaction.is_none());
        assert!(state.active_plan_path.is_none());

        let state = db
            .set_session_mode(&session.id, DispatcherMode::Plan)
            .unwrap();
        assert_eq!(state.mode, DispatcherMode::Plan);

        cleanup_test_db(root);
    }

    #[test]
    fn visible_message_usage_stats_round_trip() {
        let (db, root) = create_test_db();
        let session = db
            .create_session(
                "project-1",
                "测试会话",
                DispatcherSessionKind::Project,
                DispatcherMode::Default,
                None,
                None,
            )
            .unwrap();
        let stats = sample_message_usage_stats();

        let message = db
            .add_visible_message_with_usage(&session.id, "assistant", "完成", &stats)
            .unwrap();

        assert_eq!(message.usage_stats, Some(stats.clone()));
        let visible_messages = db.list_visible_messages(&session.id).unwrap();
        assert_eq!(visible_messages.len(), 1);
        assert_eq!(visible_messages[0].usage_stats, Some(stats));

        cleanup_test_db(root);
    }

    #[test]
    fn user_image_segments_are_indexed_in_chat_images() {
        let (db, root) = create_test_db();
        let session = db
            .create_session(
                "project-1",
                "图片会话",
                DispatcherSessionKind::Project,
                DispatcherMode::Default,
                None,
                None,
            )
            .unwrap();
        let image_path = root.join("pasted.png");
        fs::write(&image_path, b"image-bytes").expect("write image");
        let segments_json = serde_json::json!([
            {
                "type": "image",
                "id": "seg-image-1",
                "imageId": "image-1",
                "path": image_path.to_string_lossy(),
                "alt": "截图",
                "mimeType": "image/png",
                "source": "user_paste"
            },
            {
                "type": "text",
                "id": "seg-text-1",
                "text": "请看这张图"
            }
        ])
        .to_string();

        let message = db
            .add_visible_message(&session.id, "user", "请看这张图", Some(segments_json))
            .unwrap();

        let conn = db.conn().unwrap();
        let indexed: (String, String, String, i64) = conn
            .query_row(
                "SELECT image_id, message_id, path, segment_index FROM chat_images WHERE workspace_id = ?1",
                rusqlite::params![&session.id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(indexed.0, "image-1");
        assert_eq!(indexed.1, message.id);
        assert_eq!(indexed.2, image_path.to_string_lossy());
        assert_eq!(indexed.3, 0);

        cleanup_test_db(root);
    }

    #[test]
    fn clear_messages_removes_chat_image_files_and_records() {
        let (db, root) = create_test_db();
        let session = db
            .create_session(
                "project-1",
                "图片清理会话",
                DispatcherSessionKind::Project,
                DispatcherMode::Default,
                None,
                None,
            )
            .unwrap();
        let image_path = root.join("pasted.png");
        fs::write(&image_path, b"image-bytes").expect("write image");
        let segments_json = serde_json::json!([
            {
                "type": "image",
                "id": "seg-image-1",
                "imageId": "image-1",
                "path": image_path.to_string_lossy(),
                "mimeType": "image/png",
                "source": "user_paste"
            }
        ])
        .to_string();
        db.add_visible_message(&session.id, "user", "", Some(segments_json))
            .unwrap();

        db.clear_messages(&session.id).unwrap();

        assert!(!image_path.exists());
        let conn = db.conn().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM chat_images WHERE workspace_id = ?1",
                rusqlite::params![&session.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);

        cleanup_test_db(root);
    }

    #[test]
    fn clear_messages_removes_legacy_unindexed_chat_image_files() {
        let (db, root) = create_test_db();
        let session = db
            .create_session(
                "project-1",
                "旧图片清理会话",
                DispatcherSessionKind::Project,
                DispatcherMode::Default,
                None,
                None,
            )
            .unwrap();
        let image_path = root.join("legacy.png");
        fs::write(&image_path, b"legacy-image").expect("write image");
        let segments_json = serde_json::json!([
            {
                "type": "image",
                "id": "seg-image-legacy",
                "imageId": "legacy-image-1",
                "path": image_path.to_string_lossy(),
                "mimeType": "image/png",
                "source": "user_paste"
            }
        ])
        .to_string();
        db.add_visible_message(&session.id, "user", "", Some(segments_json))
            .unwrap();
        let conn = db.conn().unwrap();
        conn.execute(
            "DELETE FROM chat_images WHERE workspace_id = ?1",
            rusqlite::params![&session.id],
        )
        .unwrap();

        db.clear_messages(&session.id).unwrap();

        assert!(!image_path.exists());

        cleanup_test_db(root);
    }

    #[test]
    fn tool_result_separates_display_content_from_context_payload() {
        let (db, root) = create_test_db();
        let session = db
            .create_session(
                "project-1",
                "测试会话",
                DispatcherSessionKind::Project,
                DispatcherMode::Default,
                None,
                None,
            )
            .unwrap();
        db.add_visible_message(&session.id, "user", "检查工具结果", None)
            .unwrap();
        let message = db
            .add_visible_tool_result(
                &session.id,
                "展示摘要",
                "上下文压缩",
                Some("call-1"),
                Some("exec"),
                Some("conservative_summary"),
                &[sample_artifact()],
            )
            .unwrap();

        assert_eq!(message.content, "展示摘要");
        assert_eq!(message.tool_artifacts.len(), 1);

        let visible_messages = db.list_visible_messages(&session.id).unwrap();
        assert_eq!(visible_messages.last().unwrap().content, "展示摘要");
        assert_eq!(visible_messages.last().unwrap().tool_artifacts.len(), 1);

        let llm_history = db.load_llm_history(&session.id).unwrap();
        let tool_message = llm_history
            .iter()
            .find(|message| message.role == "tool")
            .expect("tool message should exist in llm history");
        assert_eq!(tool_message.content, "上下文压缩");

        let artifact = db
            .get_tool_artifact(&session.id, &message.tool_artifacts[0].id)
            .unwrap();
        assert_eq!(artifact.content, "line 1\nline 2");

        cleanup_test_db(root);
    }

    #[test]
    fn load_llm_history_preserves_assistant_reasoning_content() {
        let (db, root) = create_test_db();
        let session = db
            .create_session(
                "project-1",
                "思考模式会话",
                DispatcherSessionKind::Project,
                DispatcherMode::Default,
                None,
                None,
            )
            .unwrap();

        db.add_visible_message(&session.id, "user", "需要工具调用", None)
            .unwrap();
        db.add_visible_message_with_tools_and_thinking(
            &session.id,
            "assistant",
            "准备读取文件",
            None,
            None,
            None,
            None,
            Some("先确认路径和工具约束"),
            42,
        )
        .unwrap();

        let history = db.load_llm_history(&session.id).unwrap();
        let assistant = history
            .iter()
            .find(|message| message.role == "assistant")
            .expect("assistant message should be in llm history");

        assert_eq!(
            assistant.reasoning_content.as_deref(),
            Some("先确认路径和工具约束")
        );

        cleanup_test_db(root);
    }

    #[test]
    fn llm_history_preserves_assistant_reasoning_content_for_tool_calls() {
        let (db, root) = create_test_db();
        let session = db
            .create_session(
                "project-1",
                "思考工具会话",
                DispatcherSessionKind::Project,
                DispatcherMode::Default,
                None,
                None,
            )
            .unwrap();
        db.add_visible_message(&session.id, "user", "查当前时间", None)
            .unwrap();
        let tool_calls = vec![OutboundToolCall {
            id: "call-1".to_string(),
            kind: "function".to_string(),
            function: FunctionCall {
                name: "get_current_time".to_string(),
                arguments: "{}".to_string(),
            },
        }];
        db.add_visible_message_with_tools_and_thinking(
            &session.id,
            "assistant",
            "",
            None,
            None,
            None,
            Some(&tool_calls),
            Some("需要调用时间工具获取准确信息。"),
            42,
        )
        .unwrap();

        let history = db.load_llm_history(&session.id).unwrap();
        let assistant = history
            .iter()
            .find(|message| message.role == "assistant")
            .expect("assistant tool call message should be in llm history");

        assert_eq!(
            assistant.reasoning_content.as_deref(),
            Some("需要调用时间工具获取准确信息。")
        );
        assert_eq!(
            assistant.tool_calls.as_ref().map(Vec::len),
            Some(1),
            "tool calls must stay paired with the reasoning content"
        );

        cleanup_test_db(root);
    }

    #[test]
    fn clear_messages_removes_tool_artifacts_for_session() {
        let (db, root) = create_test_db();
        let session = db
            .create_session(
                "project-1",
                "测试会话",
                DispatcherSessionKind::Project,
                DispatcherMode::Default,
                None,
                None,
            )
            .unwrap();
        db.update_checklist(
            &session.id,
            &ChecklistPlanState {
                explanation: Some("规划中".to_string()),
                items: vec![ChecklistPlanItem {
                    id: Some("step_1".to_string()),
                    step: "实现状态机".to_string(),
                    status: ChecklistStepStatus::InProgress,
                    agent: Some("claude".to_string()),
                    dispatch_id: Some("dispatch-1".to_string()),
                    subprocess_task_id: Some("task-1".to_string()),
                    detail: Some("子任务".to_string()),
                }],
                updated_at: "2026-05-09T00:00:00Z".to_string(),
            },
        )
        .unwrap();
        db.set_active_plan_path(&session.id, Some("/repo/.jkcodingagent/plan/demo.md"))
            .unwrap();
        db.set_plan_interaction(
            &session.id,
            Some(&PlanInteraction::Question {
                id: "q1".to_string(),
                question: "怎么做？".to_string(),
                options: vec![PlanQuestionOption {
                    id: "a".to_string(),
                    label: "A".to_string(),
                    description: "方案 A".to_string(),
                }],
            }),
        )
        .unwrap();
        let message = db
            .add_visible_tool_result(
                &session.id,
                "展示摘要",
                "上下文压缩",
                Some("call-1"),
                Some("exec"),
                Some("conservative_summary"),
                &[sample_artifact()],
            )
            .unwrap();

        db.clear_messages(&session.id).unwrap();

        assert!(db.list_visible_messages(&session.id).unwrap().is_empty());
        assert!(db
            .get_tool_artifact(&session.id, &message.tool_artifacts[0].id)
            .is_err());
        let state = db.get_session_runtime_state(&session.id).unwrap();
        assert!(state.checklist.is_none());
        assert!(state.plan_interaction.is_none());
        assert!(state.active_plan_path.is_none());

        cleanup_test_db(root);
    }

    #[test]
    fn clear_messages_removes_plan_file_from_disk() {
        let (db, root) = create_test_db();
        let session = db
            .create_session(
                "project-1",
                "计划清理会话",
                DispatcherSessionKind::Project,
                DispatcherMode::Plan,
                None,
                None,
            )
            .unwrap();
        let plan_dir = root.join(".jkcodingagent").join("plan");
        fs::create_dir_all(&plan_dir).expect("create plan dir");
        let plan_path = plan_dir.join("20260604-demo.md");
        fs::write(&plan_path, "# 计划书\n内容").expect("write plan file");
        db.set_active_plan_path(&session.id, Some(plan_path.to_str().unwrap()))
            .unwrap();

        assert!(plan_path.exists());

        db.clear_messages(&session.id).unwrap();

        assert!(!plan_path.exists());
        let state = db.get_session_runtime_state(&session.id).unwrap();
        assert!(state.active_plan_path.is_none());

        cleanup_test_db(root);
    }

    #[test]
    fn clear_context_keeps_visible_messages_but_excludes_them_from_llm_history() {
        let (db, root) = create_test_db();
        let session = db
            .create_session(
                "project-1",
                "测试会话",
                DispatcherSessionKind::Project,
                DispatcherMode::Default,
                None,
                None,
            )
            .unwrap();

        db.add_visible_message(&session.id, "user", "旧需求", None)
            .unwrap();
        db.add_visible_message(&session.id, "assistant", "旧回复", None)
            .unwrap();
        db.clear_context_messages(&session.id).unwrap();
        db.add_visible_message(&session.id, "user", "新需求", None)
            .unwrap();

        let visible_messages = db.list_visible_messages(&session.id).unwrap();
        assert_eq!(visible_messages.len(), 3);
        assert_eq!(visible_messages[0].content, "旧需求");
        assert_eq!(visible_messages[2].content, "新需求");

        let llm_history = db.load_llm_history(&session.id).unwrap();
        assert_eq!(llm_history.len(), 1);
        assert_eq!(llm_history[0].role, "user");
        assert_eq!(llm_history[0].content, "新需求");

        cleanup_test_db(root);
    }

    #[test]
    fn recent_visible_dialogue_messages_keep_complete_latest_turns() {
        let (db, root) = create_test_db();
        let session = db
            .create_session(
                "project-1",
                "测试会话",
                DispatcherSessionKind::Project,
                DispatcherMode::Default,
                None,
                None,
            )
            .unwrap();

        db.add_visible_message(&session.id, "user", "第一轮需求", None)
            .unwrap();
        db.add_visible_message(&session.id, "assistant", "第一轮回复", None)
            .unwrap();
        db.add_visible_message(&session.id, "user", "第二轮需求", None)
            .unwrap();
        db.add_visible_tool_result(
            &session.id,
            "第二轮工具摘要",
            "第二轮工具上下文",
            Some("call-1"),
            Some("exec"),
            Some("summary"),
            &[],
        )
        .unwrap();
        db.add_visible_message(&session.id, "assistant", "第二轮回复", None)
            .unwrap();
        db.add_visible_message(&session.id, "user", "第三轮需求", None)
            .unwrap();

        let recent = db
            .list_recent_visible_dialogue_messages(&session.id, 2)
            .unwrap();
        let contents = recent
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            contents,
            vec!["第二轮需求", "第二轮工具摘要", "第二轮回复", "第三轮需求"]
        );

        cleanup_test_db(root);
    }

    #[test]
    fn clear_checklist_removes_only_step_plan_state() {
        let (db, root) = create_test_db();
        let session = db
            .create_session(
                "project-1",
                "测试会话",
                DispatcherSessionKind::Project,
                DispatcherMode::Default,
                None,
                None,
            )
            .unwrap();
        db.update_checklist(
            &session.id,
            &ChecklistPlanState {
                explanation: Some("旧规划".to_string()),
                items: vec![ChecklistPlanItem {
                    id: Some("step_1".to_string()),
                    step: "旧步骤".to_string(),
                    status: ChecklistStepStatus::Completed,
                    agent: None,
                    dispatch_id: None,
                    subprocess_task_id: None,
                    detail: None,
                }],
                updated_at: "2026-05-09T00:00:00Z".to_string(),
            },
        )
        .unwrap();
        db.set_active_plan_path(&session.id, Some("/repo/.jkcodingagent/plan/demo.md"))
            .unwrap();
        db.set_plan_interaction(
            &session.id,
            Some(&PlanInteraction::Ready {
                plan_path: "/repo/.jkcodingagent/plan/demo.md".to_string(),
                title: "Demo".to_string(),
                summary: "ready".to_string(),
            }),
        )
        .unwrap();

        let state = db.clear_checklist(&session.id).unwrap();

        assert!(state.checklist.is_none());
        assert!(state.active_plan_path.is_some());
        assert!(state.plan_interaction.is_some());

        cleanup_test_db(root);
    }

    #[test]
    fn delete_session_removes_tool_artifacts_with_session() {
        let (db, root) = create_test_db();
        let session = db
            .create_session(
                "project-1",
                "测试会话",
                DispatcherSessionKind::Project,
                DispatcherMode::Default,
                None,
                None,
            )
            .unwrap();
        let message = db
            .add_visible_tool_result(
                &session.id,
                "展示摘要",
                "上下文压缩",
                Some("call-1"),
                Some("exec"),
                Some("conservative_summary"),
                &[sample_artifact()],
            )
            .unwrap();

        db.delete_session(&session.id).unwrap();

        assert!(db
            .list_sessions("project-1", DispatcherSessionKind::Project)
            .unwrap()
            .is_empty());
        assert!(db
            .get_tool_artifact(&session.id, &message.tool_artifacts[0].id)
            .is_err());

        cleanup_test_db(root);
    }

    /// Regression test: user pastes image without typing text.
    /// The `content` passed to `add_visible_message` is "", but `segments_json`
    /// carries the image segment.  `parse_segments_json` must succeed so that
    /// the derived content becomes `![image](path)`, NOT an empty string.
    /// If `parse_segments_json` silently fails (unwrap_or_default → empty vec),
    /// the LLM receives an empty message — which is the bug we're guarding against.
    #[test]
    fn image_only_segment_produces_markdown_content() {
        let (db, root) = create_test_db();
        let session = db
            .create_session(
                "project-1",
                "纯图片会话",
                DispatcherSessionKind::Project,
                DispatcherMode::Default,
                None,
                None,
            )
            .unwrap();

        let image_path = root.join("pasted.png");
        fs::write(&image_path, b"\x89PNG\r\n\x1a\nimage-bytes").expect("write image");

        // This matches exactly what the frontend sends when the user pastes
        // an image without typing any text:
        //   content: ""  (no text)
        //   segmentsJson: [{type:"image", imageId:"...", path:"...", source:"user_paste", mimeType:"image/png"}]
        let segments_json = serde_json::json!([
            {
                "id": "seg-1",
                "type": "image",
                "imageId": "img-1",
                "path": image_path.to_string_lossy(),
                "source": "user_paste",
                "mimeType": "image/png"
            }
        ])
        .to_string();

        let message = db
            .add_visible_message(&session.id, "user", "", Some(segments_json))
            .unwrap();

        // The derived content must be markdown image syntax, NOT empty.
        assert!(
            !message.content.is_empty(),
            "image-only message content should NOT be empty, got: {:?}",
            message.content
        );
        assert!(
            message.content.contains("![image]("),
            "expected markdown image syntax, got: {:?}",
            message.content
        );

        // Also verify load_llm_history produces the same content
        let history = db.load_llm_history(&session.id).unwrap();
        let user_msg = history.iter().find(|m| m.role == "user").unwrap();
        assert!(
            !user_msg.content.is_empty(),
            "LLM history user message content should NOT be empty"
        );
        assert!(
            user_msg.content.contains("![image]("),
            "LLM history should contain markdown image syntax, got: {:?}",
            user_msg.content
        );

        cleanup_test_db(root);
    }
}
