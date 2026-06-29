//! 模型配置与设置：dispatcher_settings（传统扁平列 + 每槽位模型配置）与
//! dispatcher_settings_v2（共享/项目/聊天上下文配置）的读写与转换。

use anyhow::{Context, Result};
use rusqlite::{params, types::Type, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::sessions::AgentContext;
use super::util::normalize_summary_model;
use super::DispatcherDb;

const DEFAULT_IMAGE_MODEL_URL: &str = "https://dashscope.aliyuncs.com/api/v1";
const DEFAULT_IMAGE_MODEL: &str = "qwen-image-2.0-pro";
const DEFAULT_ASR_MODEL: &str = "fun-asr-realtime";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DispatcherModelConfig {
    pub url: String,
    pub api_key: String,
    pub model: String,
    #[serde(default = "default_model_config_active")]
    pub active: bool,
    #[serde(default)]
    pub system_prompt: String,
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
            system_prompt: String::new(),
        }
    }

    fn trimmed(self) -> Self {
        Self {
            url: self.url.trim().to_string(),
            api_key: self.api_key.trim().to_string(),
            model: self.model.trim().to_string(),
            active: self.active,
            system_prompt: self.system_prompt.trim().to_string(),
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
    #[serde(default)]
    pub allowed_tools: Vec<String>,
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
        Vec::with_capacity(SETTINGS_LEGACY_COLUMNS.len() + MODEL_SLOT_COLUMNS.len() * 4 + 1);
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
    columns.push("allowed_tools_json");
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
    allowed_tools: Vec<String>,
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
        allowed_tools,
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AhaContextConfig {
    #[serde(default)]
    pub chat_model_configs: Vec<DispatcherModelConfig>,
    #[serde(default)]
    pub summary_model_configs: Vec<DispatcherModelConfig>,
    #[serde(default)]
    pub allowed_tools: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AhaSharedModels {
    #[serde(default)]
    pub vision_model_configs: Vec<DispatcherModelConfig>,
    #[serde(default)]
    pub image_model_configs: Vec<DispatcherModelConfig>,
    #[serde(default)]
    pub image_edit_model_configs: Vec<DispatcherModelConfig>,
    #[serde(default)]
    pub asr_model_configs: Vec<DispatcherModelConfig>,
    #[serde(default)]
    pub tts_model_configs: Vec<DispatcherModelConfig>,
    #[serde(default)]
    pub embedding_model_configs: Vec<DispatcherModelConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AhaSettingsV2 {
    pub shared: AhaSharedModels,
    pub project: AhaContextConfig,
    pub chat: AhaContextConfig,
    pub auto_approve_dispatch: bool,
    pub context_debug: bool,
    #[serde(default)]
    pub review: SshReviewConfig,
}

impl Default for AhaSettingsV2 {
    fn default() -> Self {
        Self {
            shared: AhaSharedModels::default(),
            project: AhaContextConfig::default(),
            chat: AhaContextConfig::default(),
            auto_approve_dispatch: false,
            context_debug: false,
            review: SshReviewConfig::default(),
        }
    }
}

/// 命令安全审查 AI 的默认系统提示词（前后端共用同一文案）。
pub const DEFAULT_REVIEW_SYSTEM_PROMPT: &str = "你是命令安全审查员。依据用户的任务、当前意图、目标环境信息和待执行命令，判断该命令是否可安全执行。\n\n判定原则：\n- 拒绝：不可逆或高危操作，如删除/覆盖系统文件或关键数据（rm -rf 指向根目录或家目录、mkfs、dd 覆写块设备、清空数据库/表）、关机重启、提权后执行破坏性操作、fork 炸弹/资源耗尽、关闭防火墙或清空路由、向外部批量外传敏感数据。\n- 允许：常规只读巡检、查询状态、在用户明确指定目录内的受控写操作。\n- 必须结合「任务」「意图」和「目标环境」综合判断：同一命令在不同上下文风险不同（如 rm 清理临时目录可允许，针对根目录或用户家目录则拒绝）。无法确认影响范围或意图不明时，倾向拒绝。\n\n输出格式：仅一行。`ALLOW` 表示允许；`DENY: <简短中文原因>` 表示拒绝。不要输出任何多余内容。";

fn default_review_system_prompt() -> String {
    DEFAULT_REVIEW_SYSTEM_PROMPT.to_string()
}

/// 命令安全审查 AI 配置：单个 OpenAI 兼容模型 + 可编辑系统提示词。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshReviewConfig {
    #[serde(default)]
    pub model_config: DispatcherModelConfig,
    #[serde(default = "default_review_system_prompt")]
    pub system_prompt: String,
}

impl Default for SshReviewConfig {
    fn default() -> Self {
        Self {
            model_config: DispatcherModelConfig::default(),
            system_prompt: default_review_system_prompt(),
        }
    }
}

impl SshReviewConfig {
    /// 是否已配置可用的审查模型（url/api_key/model 均非空）。
    pub fn is_configured(&self) -> bool {
        !self.model_config.is_empty()
    }
}

impl DispatcherDb {
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

            let allowed_tools: Vec<String> = row
                .get::<_, Option<String>>(settings_column_index("allowed_tools_json"))?
                .and_then(|json| serde_json::from_str(&json).ok())
                .unwrap_or_default();

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
                allowed_tools,
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
        allowed_tools: Vec<String>,
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
            allowed_tools.clone(),
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
                &serde_json::to_string(&allowed_tools).context("serialize allowed_tools")?,
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

    // ── Settings v2 (project / chat split) ─────────────────────

    fn parse_model_configs_json(raw: &str) -> Vec<DispatcherModelConfig> {
        serde_json::from_str::<Vec<DispatcherModelConfig>>(raw)
            .unwrap_or_default()
            .into_iter()
            .map(DispatcherModelConfig::trimmed)
            .filter(|c| !c.is_empty())
            .collect()
    }

    fn with_default_plain_chat_prompt(
        configs: Vec<DispatcherModelConfig>,
    ) -> Vec<DispatcherModelConfig> {
        configs
            .into_iter()
            .map(|mut config| {
                if config.system_prompt.trim().is_empty() {
                    config.system_prompt =
                        crate::agent::config::DEFAULT_PLAIN_CHAT_SYSTEM_PROMPT.to_string();
                }
                config
            })
            .collect()
    }

    fn serialize_json(configs: &[DispatcherModelConfig]) -> String {
        serde_json::to_string(configs).unwrap_or_else(|_| "[]".to_string())
    }

    fn parse_review_model_config_json(raw: &str) -> DispatcherModelConfig {
        serde_json::from_str::<DispatcherModelConfig>(raw)
            .unwrap_or_default()
            .trimmed()
    }

    pub fn get_settings_v2(&self) -> Result<AhaSettingsV2> {
        let conn = self.conn()?;
        let sql = "SELECT
            shared_vision_model_configs_json,
            shared_image_model_configs_json,
            shared_image_edit_model_configs_json,
            shared_asr_model_configs_json,
            shared_tts_model_configs_json,
            shared_embedding_model_configs_json,
            project_chat_model_configs_json,
            project_summary_model_configs_json,
            project_allowed_tools_json,
            chat_agent_chat_model_configs_json,
            chat_agent_summary_model_configs_json,
            chat_agent_allowed_tools_json,
            auto_approve_dispatch,
            context_debug,
            review_model_config_json,
            review_system_prompt
        FROM dispatcher_settings_v2 WHERE id = 'default'";

        match conn.query_row(sql, [], |row| {
            Ok(AhaSettingsV2 {
                shared: AhaSharedModels {
                    vision_model_configs: Self::parse_model_configs_json(&row.get::<_, String>(0)?),
                    image_model_configs: Self::parse_model_configs_json(&row.get::<_, String>(1)?),
                    image_edit_model_configs: Self::parse_model_configs_json(
                        &row.get::<_, String>(2)?,
                    ),
                    asr_model_configs: Self::parse_model_configs_json(&row.get::<_, String>(3)?),
                    tts_model_configs: Self::parse_model_configs_json(&row.get::<_, String>(4)?),
                    embedding_model_configs: Self::parse_model_configs_json(
                        &row.get::<_, String>(5)?,
                    ),
                },
                project: AhaContextConfig {
                    chat_model_configs: Self::parse_model_configs_json(&row.get::<_, String>(6)?),
                    summary_model_configs: Self::parse_model_configs_json(
                        &row.get::<_, String>(7)?,
                    ),
                    allowed_tools: {
                        let raw: String = row.get(8)?;
                        serde_json::from_str(&raw).unwrap_or_default()
                    },
                },
                chat: AhaContextConfig {
                    chat_model_configs: Self::with_default_plain_chat_prompt(
                        Self::parse_model_configs_json(&row.get::<_, String>(9)?),
                    ),
                    summary_model_configs: Self::parse_model_configs_json(
                        &row.get::<_, String>(10)?,
                    ),
                    allowed_tools: {
                        let raw: String = row.get(11)?;
                        serde_json::from_str(&raw).unwrap_or_default()
                    },
                },
                auto_approve_dispatch: row.get::<_, i32>(12)? != 0,
                context_debug: row.get::<_, i32>(13)? != 0,
                review: {
                    let model_raw: String = row.get(14)?;
                    let prompt_raw: String = row.get(15).unwrap_or_default();
                    let model_config = Self::parse_review_model_config_json(&model_raw);
                    let system_prompt = prompt_raw.trim().to_string();
                    SshReviewConfig {
                        model_config,
                        system_prompt: if system_prompt.is_empty() {
                            default_review_system_prompt()
                        } else {
                            system_prompt
                        },
                    }
                },
            })
        }) {
            Ok(settings) => Ok(settings),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(AhaSettingsV2::default()),
            Err(e) => Err(e).context("load dispatcher settings v2"),
        }
    }

    pub fn save_settings_v2(&self, settings: &AhaSettingsV2) -> Result<AhaSettingsV2> {
        let conn = self.conn()?;
        let shared = &settings.shared;
        let project = &settings.project;
        let chat = &settings.chat;
        let auto_approve_int = if settings.auto_approve_dispatch { 1 } else { 0 };
        let context_debug_int = if settings.context_debug { 1 } else { 0 };

        let shared_vision = Self::serialize_json(&shared.vision_model_configs);
        let shared_image = Self::serialize_json(&shared.image_model_configs);
        let shared_image_edit = Self::serialize_json(&shared.image_edit_model_configs);
        let shared_asr = Self::serialize_json(&shared.asr_model_configs);
        let shared_tts = Self::serialize_json(&shared.tts_model_configs);
        let shared_embedding = Self::serialize_json(&shared.embedding_model_configs);

        let project_chat = Self::serialize_json(&project.chat_model_configs);
        let project_summary = Self::serialize_json(&project.summary_model_configs);
        let project_tools =
            serde_json::to_string(&project.allowed_tools).unwrap_or_else(|_| "[]".to_string());

        let chat_agent_chat = Self::serialize_json(&chat.chat_model_configs);
        let chat_agent_summary = Self::serialize_json(&chat.summary_model_configs);
        let chat_agent_tools =
            serde_json::to_string(&chat.allowed_tools).unwrap_or_else(|_| "[]".to_string());

        let review_model = serde_json::to_string(&settings.review.model_config)
            .unwrap_or_else(|_| "{}".to_string());
        let review_prompt = settings.review.system_prompt.trim().to_string();

        let sql = "INSERT INTO dispatcher_settings_v2 (
            id,
            shared_vision_model_configs_json,
            shared_image_model_configs_json,
            shared_image_edit_model_configs_json,
            shared_asr_model_configs_json,
            shared_tts_model_configs_json,
            shared_embedding_model_configs_json,
            project_chat_model_configs_json,
            project_summary_model_configs_json,
            project_allowed_tools_json,
            chat_agent_chat_model_configs_json,
            chat_agent_summary_model_configs_json,
            chat_agent_allowed_tools_json,
            auto_approve_dispatch,
            context_debug,
            review_model_config_json,
            review_system_prompt
        ) VALUES (
            'default', ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16
        )
        ON CONFLICT(id) DO UPDATE SET
            shared_vision_model_configs_json = ?1,
            shared_image_model_configs_json = ?2,
            shared_image_edit_model_configs_json = ?3,
            shared_asr_model_configs_json = ?4,
            shared_tts_model_configs_json = ?5,
            shared_embedding_model_configs_json = ?6,
            project_chat_model_configs_json = ?7,
            project_summary_model_configs_json = ?8,
            project_allowed_tools_json = ?9,
            chat_agent_chat_model_configs_json = ?10,
            chat_agent_summary_model_configs_json = ?11,
            chat_agent_allowed_tools_json = ?12,
            auto_approve_dispatch = ?13,
            context_debug = ?14,
            review_model_config_json = ?15,
            review_system_prompt = ?16";

        conn.execute(
            sql,
            params![
                &shared_vision,
                &shared_image,
                &shared_image_edit,
                &shared_asr,
                &shared_tts,
                &shared_embedding,
                &project_chat,
                &project_summary,
                &project_tools,
                &chat_agent_chat,
                &chat_agent_summary,
                &chat_agent_tools,
                auto_approve_int,
                context_debug_int,
                &review_model,
                &review_prompt,
            ],
        )
        .context("save dispatcher settings v2")?;

        self.get_settings_v2()
    }

    pub fn get_settings_for_context(&self, context: AgentContext) -> Result<AhaContextConfig> {
        let settings = self.get_settings_v2()?;
        Ok(match context {
            AgentContext::Project => settings.project,
            AgentContext::Chat => settings.chat,
        })
    }

    pub fn get_shared_models(&self) -> Result<AhaSharedModels> {
        let settings = self.get_settings_v2()?;
        Ok(settings.shared)
    }
}
