//! Aha 智能体设置：dispatcher_settings（共享/项目/聊天上下文配置）的读写。

use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension};
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
    /// 模型库引用：非空时 url/api_key/model 运行期由库条目解析（读取时回填、
    /// 保存时剥离），消除「库条目更新、用途槽位保留旧凭据」的漂移。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub library_id: String,
    /// 输出预算（请求体 max_tokens）与上下文窗口容量（tokens）：与凭据同规则，
    /// 运行期由引用库条目回填（读取回填、保存剥离），库条目为唯一权威源。
    /// max_tokens 为 None 时请求体完全省略该字段，由服务端默认预算接管。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
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
            library_id: self.library_id.trim().to_string(),
            max_tokens: self.max_tokens,
            context_window: self.context_window,
        }
    }

    fn is_empty(&self) -> bool {
        self.library_id.trim().is_empty()
            && self.url.trim().is_empty()
            && self.api_key.trim().is_empty()
            && self.model.trim().is_empty()
    }

    /// 库引用解析：引用条目从库回填凭据与容量；引用指向的条目缺失或停用时
    /// 一并清空，由运行入口的完整性校验显式报错。
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
                self.max_tokens = entry.max_tokens;
                self.context_window = entry.context_window;
            }
            None => {
                self.url.clear();
                self.api_key.clear();
                self.model.clear();
                self.max_tokens = None;
                self.context_window = None;
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

/// 引用条目剥离解析出的凭据与容量：落库只保留 library_id + active
/// （容量与凭据同规则，读取时由库条目回填，防止槽位漂移）。
fn strip_library_config_credentials(mut config: DispatcherModelConfig) -> DispatcherModelConfig {
    if !config.library_id.trim().is_empty() {
        config.url.clear();
        config.api_key.clear();
        config.model.clear();
        config.max_tokens = None;
        config.context_window = None;
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
    /// 验收模型（项目上下文专用）：执行图 run 收尾验收评审的独立槽位；
    /// 未配置时 verifier 回退摘要槽位。仅 project 落库存列，chat 侧恒空
    /// （serde default 兜底历史数据）。
    #[serde(default)]
    pub verifier_model_configs: Vec<DispatcherModelConfig>,
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
/// 每个条目独立持有 url/apiKey/model 与容量（maxTokens/contextWindow），
/// 供「模型用途」页按分类引用——容量与凭据一样以库条目为唯一权威源。
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
    /// 输出预算（请求体 max_tokens）。None → 请求体完全省略该字段，由服务端
    /// 默认预算接管：1M 上下文时代显式小上限（历史硬编码 8192）易与服务端
    /// 预算互相挤压——推理模型的思考 token 还与可见输出共享该预算。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// 上下文窗口容量（tokens）。None → 消费方回退
    /// DEFAULT_CONTEXT_WINDOW_CAPACITY_TOKENS（1M）；驱动会话容量展示、
    /// 上下文占用告警与子智能体滑窗裁剪阈值。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
}

fn default_library_entry_enabled() -> bool {
    true
}

/// 容量字段合法区间（与前端 ModelEntryCard 数字输入约束一致）：
/// 输出预算对齐子智能体 max_output_tokens 的既有校验先例。
const ENTRY_MAX_TOKENS_RANGE: (u32, u32) = (1024, 1_048_576);
const ENTRY_CONTEXT_WINDOW_RANGE: (u32, u32) = (1024, 100_000_000);

/// 容量归一化（服务端防线）：越界值视同未配置（None），避免异常输入
/// 把请求预算或裁剪阈值推到不可用区间。
fn normalize_capacity(value: Option<u32>, (min, max): (u32, u32)) -> Option<u32> {
    value.filter(|v| *v >= min && *v <= max)
}

fn normalized_library_entries(library: &[ModelLibraryEntry]) -> Vec<ModelLibraryEntry> {
    library
        .iter()
        .map(|entry| ModelLibraryEntry {
            max_tokens: normalize_capacity(entry.max_tokens, ENTRY_MAX_TOKENS_RANGE),
            context_window: normalize_capacity(entry.context_window, ENTRY_CONTEXT_WINDOW_RANGE),
            ..entry.clone()
        })
        .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AhaSettingsV2 {
    /// 整对象 JSON 落库（v12 起）：字段级 serde default 兜底历史/手改数据
    /// 的缺字段，不因单字段缺失拒绝整份设置。
    #[serde(default)]
    pub shared: AhaSharedModels,
    #[serde(default)]
    pub project: AhaContextConfig,
    #[serde(default)]
    pub chat: AhaContextConfig,
    #[serde(default)]
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

/// 手写 Default：主题偏好回落 `system` 而非空串——serde 的
/// `default = "default_theme_preference"` 只在反序列化生效，derive Default
/// 会给出 `theme = ""` 的非法值（无设置行时 `get_settings_v2` 走此路径）。
impl Default for AhaSettingsV2 {
    fn default() -> Self {
        Self {
            shared: AhaSharedModels::default(),
            project: AhaContextConfig::default(),
            chat: AhaContextConfig::default(),
            context_debug: false,
            review: SshReviewConfig::default(),
            model_library: Vec::new(),
            graph: GraphExecutionConfig::default(),
            theme: default_theme_preference(),
        }
    }
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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GraphExecutionConfig {
    /// 高危写检查点：每个 run 首个 coding 节点启动前暂停，等待用户在图面板恢复。
    #[serde(default = "default_pause_before_write")]
    pub pause_before_write: bool,
    /// 图节点执行器（claude-agent-acp）的启动与凭据配置。
    #[serde(default)]
    pub acp: AcpAgentConfig,
}

/// 图节点 ACP 执行器（claude-agent-acp）的启动配置。
/// 凭据注入子进程环境变量；缺省时依赖 `~/.claude` 登录态。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AcpAgentConfig {
    /// 启动命令（按空白拆分为 program + args）。空 = 托管模式：应用把版本
    /// 锁定的官方包安装到 `~/.jkcodingagent/acp-agent/` 后以固定路径启动
    /// （默认推荐）。非空 = 用户自定义命令（可信输入，原样执行）。
    #[serde(default)]
    pub command: String,
    /// 注入子进程 env `ANTHROPIC_API_KEY`。
    #[serde(default)]
    pub api_key: Option<String>,
    /// 注入子进程 env `ANTHROPIC_BASE_URL`。
    #[serde(default)]
    pub base_url: Option<String>,
}

/// 历史默认启动命令（npx 运行时拉取）：启动解析时归一为托管模式，
/// 避免老配置继续走「PATH 查找 + 每次运行联网拉取」的旧信任模型。
pub(crate) const LEGACY_NPX_ACP_COMMAND: &str =
    "npx -y @agentclientprotocol/claude-agent-acp@0.79.0";

const fn default_pause_before_write() -> bool {
    true
}

impl Default for GraphExecutionConfig {
    fn default() -> Self {
        Self {
            pause_before_write: default_pause_before_write(),
            acp: AcpAgentConfig::default(),
        }
    }
}

/// 命令安全审查 AI 的默认系统提示词（前后端共用同一文案）。
///
/// 「需用户确认」标记必须与 `ssh_review::USER_CONFIRM_MARKER` 逐字一致：
/// 审查模型按本提示词输出 `DENY: 「需用户确认」…`，`with_confirm_guidance`
/// 按 `reason.contains(USER_CONFIRM_MARKER)` 命中才附加确认指引——故此处
/// 由常量拼接构造，任一侧改词都不会静默破坏确认链路。
pub static DEFAULT_REVIEW_SYSTEM_PROMPT: std::sync::LazyLock<String> = std::sync::LazyLock::new(
    || {
        format!(
        "你是命令安全审查员。依据用户任务、执行者任务、当前意图、对话上下文、目标环境、本会话已执行命令和待执行命令，判断该命令是否可安全执行。\n\n判定原则：\n- 拒绝：不可逆或高危操作，如删除/覆盖系统文件或关键数据（rm -rf 指向根目录或家目录、mkfs、dd 覆写块设备、清空数据库/表）、关机重启、提权后执行破坏性操作、fork 炸弹/资源耗尽、关闭防火墙或清空路由、向外部批量外传敏感数据。\n- 允许：常规只读巡检、查询状态、在用户明确指定目录内的受控写操作，以及与当前任务直接相关、影响可控的操作。\n- 进程管理：清理本会话「已执行命令」中本任务自己派生的进程（终止自己启动的开发/测试/后台进程）属于正常收尾，允许；终止与本轮任务无关的进程（尤其是系统进程、归属不明的进程）一律拒绝——若任务确实需要，输出 DENY 并在原因中标注「{USER_CONFIRM_MARKER}」，交由用户决定。\n- 必须结合「任务」「意图」「对话上下文」和「已执行命令」综合判断：同一命令在不同上下文风险不同（如 rm 清理本任务产生的临时目录可允许，针对根目录或用户家目录则拒绝；kill 本会话刚启动的进程可允许，kill 无关进程则拒绝）。\n- 避免过度拒绝：当任务、意图或对话上下文能解释命令目的且影响范围可控时应放行；不要仅因对话简短、表述口语化或命令不常见而拒绝。只有确实无法确认影响范围时才倾向拒绝。\n\n输出格式：仅一行。`ALLOW` 表示允许；`DENY: <简短中文原因>` 表示拒绝；任务确实需要该危险操作时输出 `DENY: 「{USER_CONFIRM_MARKER}」<原因>`。不要输出任何多余内容。",
        USER_CONFIRM_MARKER = crate::agent::ssh_review::USER_CONFIRM_MARKER,
    )
    },
);

fn default_review_system_prompt() -> String {
    DEFAULT_REVIEW_SYSTEM_PROMPT.clone()
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

fn parse_model_configs_json(raw: &str) -> Vec<DispatcherModelConfig> {
    normalize_model_configs(
        serde_json::from_str::<Vec<DispatcherModelConfig>>(raw).unwrap_or_default(),
    )
}

fn parse_review_model_config_json(raw: &str) -> DispatcherModelConfig {
    serde_json::from_str::<DispatcherModelConfig>(raw)
        .unwrap_or_default()
        .trimmed()
}

impl AhaSettingsV2 {
    /// 全部用途槽位（不含 review）的可变引用：规范化与库回填按槽位统一处理，
    /// 不再逐槽位手写展开。
    fn model_config_slots_mut(&mut self) -> impl Iterator<Item = &mut Vec<DispatcherModelConfig>> {
        let AhaSettingsV2 {
            shared,
            project,
            chat,
            ..
        } = self;
        [
            &mut shared.vision_model_configs,
            &mut shared.image_model_configs,
            &mut shared.image_edit_model_configs,
            &mut shared.asr_model_configs,
            &mut shared.tts_model_configs,
            &mut shared.embedding_model_configs,
            &mut project.chat_model_configs,
            &mut project.summary_model_configs,
            &mut project.verifier_model_configs,
            &mut chat.chat_model_configs,
            &mut chat.summary_model_configs,
        ]
        .into_iter()
    }

    /// 库引用解析：所有用途槽位与审查模型从库条目回填凭据与容量（读取路径）。
    /// 引用指向的条目缺失或停用时一并清空，由运行入口的完整性校验显式报错。
    pub(crate) fn resolve_library_references(&mut self) {
        let library = self.model_library.clone();
        for slot in self.model_config_slots_mut() {
            *slot = resolve_model_configs_from_library(std::mem::take(slot), &library);
        }
        self.review.model_config.resolve_from_library(&library);
    }

    /// 读取侧防御性规范化（与保存端语义对称）：历史/手改库中的脏值不流入
    /// 运行期。chat 上下文无验收槽位：恒清空（与旧 20 列形态「chat 侧无
    /// verifier 存列」的语义一致）。
    fn normalize_for_read(&mut self) {
        for slot in self.model_config_slots_mut() {
            *slot = normalize_model_configs(std::mem::take(slot));
        }
        self.review.model_config = self.review.model_config.clone().trimmed();
        let prompt = self.review.system_prompt.trim().to_string();
        self.review.system_prompt = if prompt.is_empty() {
            default_review_system_prompt()
        } else {
            prompt
        };
        self.chat.verifier_model_configs = Vec::new();
        self.model_library = normalized_library_entries(&self.model_library);
        self.theme = normalize_theme_preference(&self.theme);
    }

    /// 保存前统一规范化 + 落库形态（strip 库引用凭据），确保与读取端语义
    /// 对称：全部槽位 normalize（trim、过滤空条目、active 唯一化）→ 引用
    /// 条目剥离凭据与容量（DB 只存 libraryId）；审查模型 trim、空提示词
    /// 回落默认文案；chat 验收槽位清空；主题收敛 system/light/dark；库条目
    /// 容量归一。落盘的就是该结果，写后读回不漂移。
    fn normalized_stored(&self) -> AhaSettingsV2 {
        let mut stored = self.clone();
        for slot in stored.model_config_slots_mut() {
            *slot = strip_library_credentials(normalize_model_configs(std::mem::take(slot)));
        }
        stored.review.model_config =
            strip_library_config_credentials(self.review.model_config.clone().trimmed());
        let prompt = stored.review.system_prompt.trim().to_string();
        stored.review.system_prompt = if prompt.is_empty() {
            default_review_system_prompt()
        } else {
            prompt
        };
        stored.chat.verifier_model_configs = Vec::new();
        stored.model_library = normalized_library_entries(&self.model_library);
        stored.theme = normalize_theme_preference(&self.theme);
        stored
    }
}

/// v11 及更早 dispatcher_settings 宽表行 → stored-form `AhaSettingsV2`（映射
/// 与旧 get_settings_v2 的列语义逐字段一致）。按列名容错读取：v3 前无
/// theme、v10 前无 verifier 的更旧库按默认值兜底，不依赖「沿链到达本块时
/// 必为 20 列」的前提。仅供 `migrate_v11_to_v12` 使用。
pub(crate) fn legacy_settings_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AhaSettingsV2> {
    fn text(row: &rusqlite::Row<'_>, name: &str) -> Option<String> {
        let index = row.as_ref().column_index(name).ok()?;
        row.get::<_, Option<String>>(index).ok().flatten()
    }
    // context_debug 为 INTEGER 列：按整数读取（文本读取会类型不符而丢失）。
    fn integer(row: &rusqlite::Row<'_>, name: &str) -> Option<i64> {
        let index = row.as_ref().column_index(name).ok()?;
        row.get::<_, Option<i64>>(index).ok().flatten()
    }

    let shared = AhaSharedModels {
        vision_model_configs: text(row, "shared_vision_model_configs_json")
            .map(|raw| parse_model_configs_json(&raw))
            .unwrap_or_default(),
        image_model_configs: text(row, "shared_image_model_configs_json")
            .map(|raw| parse_model_configs_json(&raw))
            .unwrap_or_default(),
        image_edit_model_configs: text(row, "shared_image_edit_model_configs_json")
            .map(|raw| parse_model_configs_json(&raw))
            .unwrap_or_default(),
        asr_model_configs: text(row, "shared_asr_model_configs_json")
            .map(|raw| parse_model_configs_json(&raw))
            .unwrap_or_default(),
        tts_model_configs: text(row, "shared_tts_model_configs_json")
            .map(|raw| parse_model_configs_json(&raw))
            .unwrap_or_default(),
        embedding_model_configs: text(row, "shared_embedding_model_configs_json")
            .map(|raw| parse_model_configs_json(&raw))
            .unwrap_or_default(),
    };
    let project = AhaContextConfig {
        chat_model_configs: text(row, "project_chat_model_configs_json")
            .map(|raw| parse_model_configs_json(&raw))
            .unwrap_or_default(),
        summary_model_configs: text(row, "project_summary_model_configs_json")
            .map(|raw| parse_model_configs_json(&raw))
            .unwrap_or_default(),
        verifier_model_configs: text(row, "project_verifier_model_configs_json")
            .map(|raw| parse_model_configs_json(&raw))
            .unwrap_or_default(),
        allowed_tools: text(row, "project_allowed_tools_json")
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default(),
    };
    let chat = AhaContextConfig {
        chat_model_configs: text(row, "chat_agent_chat_model_configs_json")
            .map(|raw| parse_model_configs_json(&raw))
            .unwrap_or_default(),
        summary_model_configs: text(row, "chat_agent_summary_model_configs_json")
            .map(|raw| parse_model_configs_json(&raw))
            .unwrap_or_default(),
        // chat 上下文无验收槽位存列：恒空。
        verifier_model_configs: Vec::new(),
        allowed_tools: text(row, "chat_agent_allowed_tools_json")
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default(),
    };
    let context_debug = integer(row, "context_debug")
        .map(|value| value != 0)
        .unwrap_or(false);
    let review = SshReviewConfig {
        model_config: text(row, "review_model_config_json")
            .map(|raw| parse_review_model_config_json(&raw))
            .unwrap_or_default(),
        system_prompt: {
            let prompt = text(row, "review_system_prompt")
                .map(|raw| raw.trim().to_string())
                .unwrap_or_default();
            if prompt.is_empty() {
                default_review_system_prompt()
            } else {
                prompt
            }
        },
    };
    let model_library = text(row, "model_library_json")
        .map(|raw| {
            normalized_library_entries(
                &serde_json::from_str::<Vec<ModelLibraryEntry>>(&raw).unwrap_or_default(),
            )
        })
        .unwrap_or_default();
    let graph = text(row, "graph_execution_config_json")
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();
    let theme = text(row, "theme").unwrap_or_else(default_theme_preference);

    Ok(AhaSettingsV2 {
        shared,
        project,
        chat,
        context_debug,
        review,
        model_library,
        graph,
        theme: normalize_theme_preference(&theme),
    })
}

impl DispatcherDb {
    pub fn get_settings_v2(&self) -> Result<AhaSettingsV2> {
        let conn = self.conn()?;
        let raw: Option<String> = conn
            .query_row(
                "SELECT settings_json FROM dispatcher_settings WHERE id = 'default'",
                [],
                |row| row.get(0),
            )
            .optional()
            .context("load dispatcher settings v2")?;
        let Some(raw) = raw else {
            return Ok(AhaSettingsV2::default());
        };
        let mut settings: AhaSettingsV2 = serde_json::from_str(&raw)
            .with_context(|| format!("解析 dispatcher settings json 失败（{} 字节）", raw.len()))?;
        settings.normalize_for_read();
        settings.resolve_library_references();
        Ok(settings)
    }

    pub fn save_settings_v2(&self, settings: &AhaSettingsV2) -> Result<AhaSettingsV2> {
        let stored = settings.normalized_stored();
        let json = serde_json::to_string(&stored).context("serialize dispatcher settings json")?;
        self.conn()?
            .execute(
                "INSERT INTO dispatcher_settings (id, settings_json)
                 VALUES ('default', ?1)
                 ON CONFLICT(id) DO UPDATE SET settings_json = excluded.settings_json",
                params![&json],
            )
            .context("save dispatcher settings")?;
        // 直接返回落盘的规范化结果，保证返回值与 DB 状态一致。
        Ok(stored)
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

    /// 落库形态（stored-form JSON）：断言「库引用槽位剥离凭据」等落库约定
    /// 时经它读取，不再依赖列名（v12 起整对象单列存储）。
    fn stored_json(db: &DispatcherDb) -> serde_json::Value {
        let conn = db.conn().unwrap();
        let raw: String = conn
            .query_row(
                "SELECT settings_json FROM dispatcher_settings WHERE id='default'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        serde_json::from_str(&raw).unwrap()
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
            max_tokens: None,
            context_window: None,
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

        // 落库形态：引用条目不含凭据（库条目本身持有凭据，是唯一权威源）。
        let stored = stored_json(&db);
        let slot = &stored["project"]["chatModelConfigs"][0];
        assert_eq!(slot["libraryId"], "e1");
        assert_eq!(slot["apiKey"], "");
        assert_eq!(stored["modelLibrary"][0]["apiKey"], "sk-lib");
        let review_slot = &stored["review"]["modelConfig"];
        assert_eq!(review_slot["libraryId"], "e1");
        assert_eq!(review_slot["apiKey"], "");

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
        assert!(
            entry.max_tokens.is_none() && entry.context_window.is_none(),
            "停用条目的容量必须与凭据一起清空"
        );
    }

    #[test]
    fn capacity_fields_strip_on_store_and_backfill_on_load() {
        let db = test_db();
        let mut entry = library_entry("e1", true);
        entry.max_tokens = Some(65_536);
        entry.context_window = Some(1_000_000);
        let mut settings = AhaSettingsV2 {
            model_library: vec![entry],
            ..Default::default()
        };
        settings.chat.chat_model_configs = vec![DispatcherModelConfig {
            library_id: "e1".to_string(),
            active: true,
            // 槽位携带的陈旧容量：保存必须剥离，读取必须由库条目回填覆盖。
            max_tokens: Some(999),
            context_window: Some(999),
            ..Default::default()
        }];
        db.save_settings_v2(&settings).unwrap();

        let stored = stored_json(&db);
        let slot = &stored["chat"]["chatModelConfigs"][0];
        assert_eq!(slot["libraryId"], "e1");
        assert!(
            slot.get("maxTokens").is_none() && slot.get("contextWindow").is_none(),
            "引用槽位的容量必须随凭据一起剥离：{slot}"
        );

        let loaded = db.get_settings_v2().unwrap();
        let chat = &loaded.chat.chat_model_configs[0];
        assert_eq!(chat.max_tokens, Some(65_536));
        assert_eq!(chat.context_window, Some(1_000_000));
    }

    #[test]
    fn out_of_range_entry_capacity_is_normalized_to_unset() {
        let db = test_db();
        let mut entry = library_entry("e1", true);
        entry.max_tokens = Some(10); // 低于下限 1024
        entry.context_window = Some(200_000_000); // 高于上限 100M
        let settings = AhaSettingsV2 {
            model_library: vec![entry],
            ..Default::default()
        };
        db.save_settings_v2(&settings).unwrap();

        let loaded = db.get_settings_v2().unwrap();
        assert_eq!(loaded.model_library[0].max_tokens, None);
        assert_eq!(loaded.model_library[0].context_window, None);
    }

    #[test]
    fn verifier_slot_strips_on_store_and_resolves_on_load() {
        let db = test_db();
        let mut settings = AhaSettingsV2 {
            model_library: vec![library_entry("e1", true)],
            ..Default::default()
        };
        settings.project.verifier_model_configs = vec![DispatcherModelConfig {
            library_id: "e1".to_string(),
            active: true,
            ..Default::default()
        }];
        db.save_settings_v2(&settings).unwrap();

        let stored = stored_json(&db);
        let slot = &stored["project"]["verifierModelConfigs"][0];
        assert_eq!(slot["libraryId"], "e1");
        assert_eq!(slot["apiKey"], "", "落库必须剥离库引用凭据：{slot}");

        let loaded = db.get_settings_v2().unwrap();
        let verifier = &loaded.project.verifier_model_configs[0];
        assert_eq!(verifier.api_key, "sk-lib");
        assert_eq!(verifier.model, "lib-model");
    }

    #[test]
    fn chat_context_verifier_slot_stays_empty_after_roundtrip() {
        // chat 上下文无验收槽位存列：保存时显式清空，防止写后读回漂移。
        let db = test_db();
        let mut settings = AhaSettingsV2::default();
        settings.chat.verifier_model_configs = vec![DispatcherModelConfig {
            url: "http://u".into(),
            api_key: "k".into(),
            model: "m".into(),
            ..Default::default()
        }];
        let saved = db.save_settings_v2(&settings).unwrap();
        assert!(saved.chat.verifier_model_configs.is_empty());
        assert!(db
            .get_settings_v2()
            .unwrap()
            .chat
            .verifier_model_configs
            .is_empty());
    }
}
