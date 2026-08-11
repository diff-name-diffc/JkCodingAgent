//! Aha 智能体设置：dispatcher_settings_v2（共享/项目/聊天上下文配置）的读写。

use anyhow::{Context, Result};
use rusqlite::params;
use serde::{Deserialize, Serialize};

use super::DispatcherDb;

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

/// 分类模型库条目：按模型调用方式（text/vision/image/...）分类，
/// 每个条目独立持有 url/apiKey/model，供「模型用途」页按分类引用。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModelLibraryEntry {
    pub id: String,
    pub category: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub alias: String,
    #[serde(default = "default_library_entry_enabled")]
    pub enabled: bool,
}

fn default_library_entry_enabled() -> bool {
    true
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
    #[serde(default)]
    pub model_library: Vec<ModelLibraryEntry>,
    #[serde(default)]
    pub graph: GraphExecutionConfig,
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
            model_library: Vec::new(),
            graph: GraphExecutionConfig::default(),
        }
    }
}

/// 执行图编排的运行期设置（设置中心「执行图」页）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GraphExecutionConfig {
    /// 高危写检查点：每个 run 首个 coding 节点启动前暂停，等待用户在图面板恢复。
    #[serde(default)]
    pub pause_before_write: bool,
}

impl Default for GraphExecutionConfig {
    fn default() -> Self {
        Self {
            pause_before_write: false,
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
    fn parse_model_configs_json(raw: &str) -> Vec<DispatcherModelConfig> {
        normalize_model_configs(
            serde_json::from_str::<Vec<DispatcherModelConfig>>(raw).unwrap_or_default(),
        )
    }

    fn without_model_system_prompts(
        configs: &[DispatcherModelConfig],
    ) -> Vec<DispatcherModelConfig> {
        configs
            .iter()
            .cloned()
            .map(|mut config| {
                config.system_prompt.clear();
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
            review_system_prompt,
            model_library_json,
            graph_execution_config_json
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
                    chat_model_configs: Self::parse_model_configs_json(&row.get::<_, String>(9)?),
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
                model_library: {
                    let raw: String = row.get(16).unwrap_or_default();
                    serde_json::from_str::<Vec<ModelLibraryEntry>>(&raw).unwrap_or_default()
                },
                graph: {
                    let raw: String = row.get(17).unwrap_or_default();
                    serde_json::from_str::<GraphExecutionConfig>(&raw).unwrap_or_default()
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

        let chat_agent_chat = Self::serialize_json(&Self::without_model_system_prompts(
            &chat.chat_model_configs,
        ));
        let chat_agent_summary = Self::serialize_json(&chat.summary_model_configs);
        let chat_agent_tools =
            serde_json::to_string(&chat.allowed_tools).unwrap_or_else(|_| "[]".to_string());

        let review_model = serde_json::to_string(&settings.review.model_config)
            .unwrap_or_else(|_| "{}".to_string());
        let review_prompt = settings.review.system_prompt.trim().to_string();
        let model_library =
            serde_json::to_string(&settings.model_library).unwrap_or_else(|_| "[]".to_string());
        let graph_config =
            serde_json::to_string(&settings.graph).unwrap_or_else(|_| "{}".to_string());

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
            review_system_prompt,
            model_library_json,
            graph_execution_config_json
        ) VALUES (
            'default', ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18
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
            review_system_prompt = ?16,
            model_library_json = ?17,
            graph_execution_config_json = ?18";

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
                &model_library,
                &graph_config,
            ],
        )
        .context("save dispatcher settings v2")?;

        self.get_settings_v2()
    }
}
