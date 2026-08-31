//! Aha 智能体设置：dispatcher_settings（共享/项目/聊天上下文配置）的读写。

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
    /// 模型库引用：非空时 url/api_key/model 运行期由库条目解析（读取时回填、
    /// 保存时剥离），消除「库条目更新、用途槽位保留旧凭据」的漂移。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub library_id: String,
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
            library_id: self.library_id.trim().to_string(),
        }
    }

    fn is_empty(&self) -> bool {
        self.library_id.trim().is_empty()
            && self.url.trim().is_empty()
            && self.api_key.trim().is_empty()
            && self.model.trim().is_empty()
    }

    /// 库引用解析：引用条目从库回填凭据；引用指向的条目缺失或停用时清空
    /// 凭据，由运行入口的完整性校验显式报错。
    fn resolve_from_library(&mut self, library: &[ModelLibraryEntry]) {
        if self.library_id.trim().is_empty() {
            return;
        }
        match library
            .iter()
            .find(|entry| entry.id == self.library_id && entry.enabled)
        {
            Some(entry) => {
                self.url = entry.url.trim().to_string();
                self.api_key = entry.api_key.trim().to_string();
                self.model = entry.model.trim().to_string();
            }
            None => {
                self.url.clear();
                self.api_key.clear();
                self.model.clear();
            }
        }
    }
}

fn resolve_model_configs_from_library(
    configs: Vec<DispatcherModelConfig>,
    library: &[ModelLibraryEntry],
) -> Vec<DispatcherModelConfig> {
    configs
        .into_iter()
        .map(|mut config| {
            config.resolve_from_library(library);
            config
        })
        .collect()
}

/// 引用条目剥离解析出的凭据：落库只保留 library_id + active + system_prompt。
fn strip_library_config_credentials(mut config: DispatcherModelConfig) -> DispatcherModelConfig {
    if !config.library_id.trim().is_empty() {
        config.url.clear();
        config.api_key.clear();
        config.model.clear();
    }
    config
}

fn strip_library_credentials(configs: Vec<DispatcherModelConfig>) -> Vec<DispatcherModelConfig> {
    configs
        .into_iter()
        .map(strip_library_config_credentials)
        .collect()
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

/// 图片生成/编辑工具的运行期凭据（active 条目；库引用已在 get_settings_v2
/// 读取路径解析为完整凭据）。edit_model 仅补充编辑用途的模型名——生成与
/// 编辑共用 image_model_configs 的网关与密钥（见 builtin/image_edit.rs）。
#[derive(Debug, Clone, Default)]
pub(crate) struct ImageModelCredentials {
    pub url: String,
    pub api_key: String,
    pub model: String,
    pub edit_model: String,
}

impl AhaSharedModels {
    pub(crate) fn image_model_credentials(&self) -> ImageModelCredentials {
        fn active(configs: &[DispatcherModelConfig]) -> Option<&DispatcherModelConfig> {
            configs
                .iter()
                .find(|config| config.active)
                .or_else(|| configs.first())
        }
        let mut credentials = ImageModelCredentials::default();
        if let Some(config) = active(&self.image_model_configs) {
            credentials.url = config.url.trim().to_string();
            credentials.api_key = config.api_key.trim().to_string();
            credentials.model = config.model.trim().to_string();
        }
        if let Some(config) = active(&self.image_edit_model_configs) {
            credentials.edit_model = config.model.trim().to_string();
        }
        credentials
    }
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AhaSettingsV2 {
    pub shared: AhaSharedModels,
    pub project: AhaContextConfig,
    pub chat: AhaContextConfig,
    pub context_debug: bool,
    #[serde(default)]
    pub review: SshReviewConfig,
    #[serde(default)]
    pub model_library: Vec<ModelLibraryEntry>,
    #[serde(default)]
    pub graph: GraphExecutionConfig,
    /// 外观主题偏好（system / light / dark）。应用级偏好，随设置统一存取；
    /// 前端 `lib/theme.ts` 据此切换根节点 `.dark` 类。
    #[serde(default = "default_theme_preference")]
    pub theme: String,
}

fn default_theme_preference() -> String {
    "system".to_string()
}

/// 主题偏好规范化：仅接受 system / light / dark，其余回落 system
/// （与前端 `normalizeThemePreference` 语义一致）。
fn normalize_theme_preference(raw: &str) -> String {
    match raw.trim() {
        value @ ("system" | "light" | "dark") => value.to_string(),
        _ => default_theme_preference(),
    }
}

/// 执行图编排的运行期设置（设置中心「执行图」页）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GraphExecutionConfig {
    /// 高危写检查点：每个 run 首个 coding 节点启动前暂停，等待用户在图面板恢复。
    #[serde(default = "default_pause_before_write")]
    pub pause_before_write: bool,
}

const fn default_pause_before_write() -> bool {
    true
}

impl Default for GraphExecutionConfig {
    fn default() -> Self {
        Self {
            pause_before_write: default_pause_before_write(),
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

    /// 落库序列化：引用条目剥离解析出的凭据，DB 只存 library_id 引用。
    fn stored_json(configs: &[DispatcherModelConfig]) -> String {
        Self::serialize_json(&strip_library_credentials(configs.to_vec()))
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
            context_debug,
            review_model_config_json,
            review_system_prompt,
            model_library_json,
            graph_execution_config_json,
            theme
        FROM dispatcher_settings WHERE id = 'default'";

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
                context_debug: row.get::<_, i32>(12)? != 0,
                review: {
                    let model_raw: String = row.get(13)?;
                    let prompt_raw: String = row.get(14).unwrap_or_default();
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
                    let raw: String = row.get(15).unwrap_or_default();
                    serde_json::from_str::<Vec<ModelLibraryEntry>>(&raw).unwrap_or_default()
                },
                graph: {
                    let raw: String = row.get(16).unwrap_or_default();
                    serde_json::from_str::<GraphExecutionConfig>(&raw).unwrap_or_default()
                },
                theme: row
                    .get::<_, String>(17)
                    .unwrap_or_else(|_| default_theme_preference()),
            })
        }) {
            Ok(mut settings) => {
                // 库引用解析：所有用途槽位与审查模型从库回填凭据。
                let library = settings.model_library.clone();
                settings.shared.vision_model_configs = resolve_model_configs_from_library(
                    settings.shared.vision_model_configs,
                    &library,
                );
                settings.shared.image_model_configs = resolve_model_configs_from_library(
                    settings.shared.image_model_configs,
                    &library,
                );
                settings.shared.image_edit_model_configs = resolve_model_configs_from_library(
                    settings.shared.image_edit_model_configs,
                    &library,
                );
                settings.shared.asr_model_configs =
                    resolve_model_configs_from_library(settings.shared.asr_model_configs, &library);
                settings.shared.tts_model_configs =
                    resolve_model_configs_from_library(settings.shared.tts_model_configs, &library);
                settings.shared.embedding_model_configs = resolve_model_configs_from_library(
                    settings.shared.embedding_model_configs,
                    &library,
                );
                settings.project.chat_model_configs = resolve_model_configs_from_library(
                    settings.project.chat_model_configs,
                    &library,
                );
                settings.project.summary_model_configs = resolve_model_configs_from_library(
                    settings.project.summary_model_configs,
                    &library,
                );
                settings.chat.chat_model_configs =
                    resolve_model_configs_from_library(settings.chat.chat_model_configs, &library);
                settings.chat.summary_model_configs = resolve_model_configs_from_library(
                    settings.chat.summary_model_configs,
                    &library,
                );
                settings.review.model_config.resolve_from_library(&library);
                Ok(settings)
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(AhaSettingsV2::default()),
            Err(e) => Err(e).context("load dispatcher settings v2"),
        }
    }

    pub fn save_settings_v2(&self, settings: &AhaSettingsV2) -> Result<AhaSettingsV2> {
        let conn = self.conn()?;

        // 保存前统一规范化，确保与读取端（get_settings_v2）语义对称：
        // - 全部模型配置列表经 normalize_model_configs（trim、过滤空条目、active 唯一化）；
        // - 聊天对话模型配置按既有约定清除 system_prompt 后再规范化；
        // - 审查模型 trim，空提示词回落默认文案（与读取端一致）；
        // - 主题偏好收敛为 system/light/dark，非法值回落 system。
        // 落盘的就是规范化结果，函数直接返回它，写后读回不再漂移。
        let shared = AhaSharedModels {
            vision_model_configs: normalize_model_configs(
                settings.shared.vision_model_configs.clone(),
            ),
            image_model_configs: normalize_model_configs(
                settings.shared.image_model_configs.clone(),
            ),
            image_edit_model_configs: normalize_model_configs(
                settings.shared.image_edit_model_configs.clone(),
            ),
            asr_model_configs: normalize_model_configs(settings.shared.asr_model_configs.clone()),
            tts_model_configs: normalize_model_configs(settings.shared.tts_model_configs.clone()),
            embedding_model_configs: normalize_model_configs(
                settings.shared.embedding_model_configs.clone(),
            ),
        };
        let project = AhaContextConfig {
            chat_model_configs: normalize_model_configs(
                settings.project.chat_model_configs.clone(),
            ),
            summary_model_configs: normalize_model_configs(
                settings.project.summary_model_configs.clone(),
            ),
            allowed_tools: settings.project.allowed_tools.clone(),
        };
        let chat = AhaContextConfig {
            chat_model_configs: normalize_model_configs(Self::without_model_system_prompts(
                &settings.chat.chat_model_configs,
            )),
            summary_model_configs: normalize_model_configs(
                settings.chat.summary_model_configs.clone(),
            ),
            allowed_tools: settings.chat.allowed_tools.clone(),
        };
        let review = SshReviewConfig {
            model_config: settings.review.model_config.clone().trimmed(),
            system_prompt: {
                let prompt = settings.review.system_prompt.trim().to_string();
                if prompt.is_empty() {
                    default_review_system_prompt()
                } else {
                    prompt
                }
            },
        };

        let context_debug_int = if settings.context_debug { 1 } else { 0 };

        let shared_vision = Self::stored_json(&shared.vision_model_configs);
        let shared_image = Self::stored_json(&shared.image_model_configs);
        let shared_image_edit = Self::stored_json(&shared.image_edit_model_configs);
        let shared_asr = Self::stored_json(&shared.asr_model_configs);
        let shared_tts = Self::stored_json(&shared.tts_model_configs);
        let shared_embedding = Self::stored_json(&shared.embedding_model_configs);

        let project_chat = Self::stored_json(&project.chat_model_configs);
        let project_summary = Self::stored_json(&project.summary_model_configs);
        let project_tools =
            serde_json::to_string(&project.allowed_tools).unwrap_or_else(|_| "[]".to_string());

        let chat_agent_chat = Self::stored_json(&chat.chat_model_configs);
        let chat_agent_summary = Self::stored_json(&chat.summary_model_configs);
        let chat_agent_tools =
            serde_json::to_string(&chat.allowed_tools).unwrap_or_else(|_| "[]".to_string());

        let review_model = serde_json::to_string(&strip_library_config_credentials(
            review.model_config.clone(),
        ))
        .unwrap_or_else(|_| "{}".to_string());
        let review_prompt = review.system_prompt.clone();
        let model_library =
            serde_json::to_string(&settings.model_library).unwrap_or_else(|_| "[]".to_string());
        let graph_config =
            serde_json::to_string(&settings.graph).unwrap_or_else(|_| "{}".to_string());
        let theme = normalize_theme_preference(&settings.theme);

        let sql = "INSERT INTO dispatcher_settings (
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
            context_debug,
            review_model_config_json,
            review_system_prompt,
            model_library_json,
            graph_execution_config_json,
            theme
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
            context_debug = ?13,
            review_model_config_json = ?14,
            review_system_prompt = ?15,
            model_library_json = ?16,
            graph_execution_config_json = ?17,
            theme = ?18";

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
                context_debug_int,
                &review_model,
                &review_prompt,
                &model_library,
                &graph_config,
                &theme,
            ],
        )
        .context("save dispatcher settings")?;

        // 直接返回落盘的规范化结果，保证返回值与 DB 状态一致。
        Ok(AhaSettingsV2 {
            shared,
            project,
            chat,
            context_debug: settings.context_debug,
            review,
            model_library: settings.model_library.clone(),
            graph: settings.graph,
            theme,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> DispatcherDb {
        let path = std::env::temp_dir().join(format!(
            "aha-settings-{}-{}.sqlite3",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        DispatcherDb::new(path).unwrap()
    }

    fn library_entry(id: &str, enabled: bool) -> ModelLibraryEntry {
        ModelLibraryEntry {
            id: id.to_string(),
            category: "text".to_string(),
            url: "https://api.example.com/v1".to_string(),
            api_key: "sk-lib".to_string(),
            model: "lib-model".to_string(),
            alias: "库条目".to_string(),
            enabled,
        }
    }

    #[test]
    fn reference_entries_strip_credentials_on_store_and_resolve_on_load() {
        let db = test_db();
        let mut settings = AhaSettingsV2 {
            model_library: vec![library_entry("e1", true)],
            ..Default::default()
        };
        settings.project.chat_model_configs = vec![DispatcherModelConfig {
            library_id: "e1".to_string(),
            active: true,
            ..Default::default()
        }];
        settings.review.model_config = DispatcherModelConfig {
            library_id: "e1".to_string(),
            ..Default::default()
        };
        db.save_settings_v2(&settings).unwrap();

        // 落库形态：引用条目不含凭据。
        let conn = db.conn().unwrap();
        let raw: String = conn
            .query_row(
                "SELECT project_chat_model_configs_json FROM dispatcher_settings WHERE id='default'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(raw.contains("\"libraryId\":\"e1\""));
        assert!(!raw.contains("sk-lib"));
        drop(conn);

        // 读取形态：凭据由库回填，运行期消费方无感知。
        let loaded = db.get_settings_v2().unwrap();
        let chat = &loaded.project.chat_model_configs[0];
        assert_eq!(chat.library_id, "e1");
        assert_eq!(chat.api_key, "sk-lib");
        assert_eq!(chat.model, "lib-model");
        assert_eq!(loaded.review.model_config.api_key, "sk-lib");
        assert!(loaded.review.is_configured());
    }

    #[test]
    fn reference_to_disabled_entry_resolves_empty() {
        let db = test_db();
        let mut settings = AhaSettingsV2 {
            model_library: vec![library_entry("e1", false)],
            ..Default::default()
        };
        settings.chat.chat_model_configs = vec![DispatcherModelConfig {
            library_id: "e1".to_string(),
            active: true,
            ..Default::default()
        }];
        db.save_settings_v2(&settings).unwrap();

        let loaded = db.get_settings_v2().unwrap();
        let entry = &loaded.chat.chat_model_configs[0];
        assert_eq!(entry.library_id, "e1");
        assert!(entry.url.is_empty() && entry.api_key.is_empty() && entry.model.is_empty());
    }
}
