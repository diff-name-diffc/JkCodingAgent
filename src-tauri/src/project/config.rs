use std::fs;
use std::path::Path;

use super::mcp::ensure_project_mcp_file;
use super::storage::{atomic_write, StorageError};
use crate::shared::error::{CommandResult, IntoCommandResult};
use anyhow::Context;

type ConfigResult<T> = std::result::Result<T, ConfigError>;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("{action} 失败（{path}）：{source}")]
    Io {
        action: &'static str,
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("解析项目配置失败（{path}）：{source}")]
    ParseProjectConfig {
        path: std::path::PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("序列化项目配置失败（{path}）：{source}")]
    SerializeProjectConfig {
        path: std::path::PathBuf,
        #[source]
        source: toml::ser::Error,
    },
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error("初始化 MCP 配置失败：{0}")]
    Mcp(String),
}

fn io_error(
    action: &'static str,
    path: impl Into<std::path::PathBuf>,
) -> impl FnOnce(std::io::Error) -> ConfigError {
    move |source| ConfigError::Io {
        action,
        path: path.into(),
        source,
    }
}

const DEFAULT_AGENT_PROMPT_PREFIX: &str = "- 先围绕当前任务目标确认相关代码、约束和必要上下文。\n- 只做与目标直接相关的最小充分改动，避免无关重构。\n- 完成后简洁说明改动、验证结果和剩余风险。";

const PREVIOUS_DEFAULT_AGENT_PROMPT_PREFIX: &str = "你是一名资深软件工程师，当前目标是完成用户交付的编码任务。\n- 先阅读相关代码、调用链和约束，再动手修改。\n- 优先做与当前任务直接相关的最小充分改动，不无端扩散范围。\n- 对正确性、边界条件、兼容性和可维护性保持敏感。\n- 修改后主动做编译、测试或关键路径验证；若无法验证，明确说明。\n- 以操作和交付为主，少讲概念，少做长篇分析。\n- 过程输出保持简洁，只在必要时汇报关键判断、风险和下一步。\n- 不要重复复述用户需求，不要写冗长总结。\n- 收尾时只做简洁结果说明：改了什么、验证了什么、还有什么风险。";

const LEGACY_DEFAULT_AGENT_PROMPT_PREFIX: &str = "你是一名资深软件工程师，当前目标是完成用户交付的编码任务。\n- 先阅读相关代码、调用链和约束，再动手修改。\n- 优先做与当前任务直接相关的最小充分改动，不无端扩散范围。\n- 对正确性、边界条件、兼容性和可维护性保持敏感。\n- 修改后主动做编译、测试或关键路径验证；若无法验证，明确说明。\n- 输出聚焦结果、风险、验证结论和后续建议。";

const DEFAULT_COMMIT_PROMPT: &str = "你是一名资深软件工程师，请基于给定的 Git diff 生成提交信息。\n要求：\n1. 使用祈使句，直接描述本次改动。\n2. 第一行格式为 type(scope): summary，尽量不超过 50 个字符。\n3. type 仅使用 feat、fix、refactor、docs、style、test、chore。\n4. 如需补充说明，空一行后用 1-3 行说明原因、影响或验证重点。\n5. 只输出提交信息正文，不要解释，不要 Markdown。";

const DEFAULT_CONFIG: &str = r#"# JKCodingAgent 项目配置
# https://github.com/diff-name-diffc/JkCodingAgent

[agent]
# 每个任务提示词前自动追加的公共工程指令
prompt_prefix = "- 先围绕当前任务目标确认相关代码和文件路径、和子任务相关的上下文。\n- 只做与目标直接相关的最小充分改动，避免无关重构。\n- 完成后简洁说明改动、验证结果和剩余风险。"

[git]
# 生成提交信息时使用的提示词
commit_prompt = "你是一名资深软件工程师，请基于给定的 Git diff 生成提交信息。\n要求：\n1. 使用祈使句，直接描述本次改动。\n2. 第一行格式为 type(scope): summary，尽量不超过 50 个字符。\n3. type 仅使用 feat、fix、refactor、docs、style、test、chore。\n4. 如需补充说明，空一行后用 1-3 行说明原因、影响或验证重点。\n5. 只输出提交信息正文，不要解释，不要 Markdown。"

[browser]
# 是否允许内置 Aha Agent 启动 CloakBrowser
enabled = true
# 可选代理，例如 "http://user:pass@host:8080" 或 "socks5://host:1080"
proxy = ""
# 浏览器 locale / timezone，留空则使用 CloakBrowser 默认值
locale = ""
timezone = ""
# 右侧抽屉镜像视口尺寸
viewport_width = 1280
viewport_height = 800
"#;

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct AgentConfig {
    #[serde(default)]
    pub prompt_prefix: String,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct GitConfig {
    pub commit_prompt: String,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct BrowserConfig {
    #[serde(default = "default_browser_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub proxy: String,
    #[serde(default)]
    pub locale: String,
    #[serde(default)]
    pub timezone: String,
    #[serde(default = "default_browser_viewport_width")]
    pub viewport_width: u32,
    #[serde(default = "default_browser_viewport_height")]
    pub viewport_height: u32,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            enabled: default_browser_enabled(),
            proxy: String::new(),
            locale: String::new(),
            timezone: String::new(),
            viewport_width: default_browser_viewport_width(),
            viewport_height: default_browser_viewport_height(),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct ProjectConfig {
    pub agent: AgentConfig,
    pub git: GitConfig,
    #[serde(default)]
    pub browser: BrowserConfig,
}

impl Default for ProjectConfig {
    fn default() -> Self {
        ProjectConfig {
            agent: AgentConfig {
                prompt_prefix: DEFAULT_AGENT_PROMPT_PREFIX.to_string(),
            },
            git: GitConfig {
                commit_prompt: DEFAULT_COMMIT_PROMPT.to_string(),
            },
            browser: BrowserConfig::default(),
        }
    }
}

fn default_browser_enabled() -> bool {
    true
}

fn default_browser_viewport_width() -> u32 {
    1280
}

fn default_browser_viewport_height() -> u32 {
    800
}

fn should_refresh_prompt_prefix(prompt_prefix: &str) -> bool {
    prompt_prefix.is_empty()
        || prompt_prefix == PREVIOUS_DEFAULT_AGENT_PROMPT_PREFIX
        || prompt_prefix == LEGACY_DEFAULT_AGENT_PROMPT_PREFIX
}

fn contains_legacy_agent_fields(raw: &str) -> bool {
    toml::from_str::<toml::Value>(raw)
        .ok()
        .and_then(|value| value.get("agent").and_then(toml::Value::as_table).cloned())
        .is_some_and(|agent| {
            ["default", "default_agent", "claude_version", "codex_version"]
                .iter()
                .any(|key| agent.contains_key(*key))
        })
}

/// Creates `.jkcodingagent/config.toml` in the project directory if it doesn't already exist.
/// Also ensures `.jkcodingagent/mcp.json` exists.
/// Returns the parsed config.
#[tauri::command]
pub fn init_project_config(project_path: String) -> CommandResult<ProjectConfig> {
    init_project_config_impl(&project_path)
        .with_context(|| format!("初始化项目配置失败（{}）", project_path))
        .into_command_result()
}

fn init_project_config_impl(project_path: &str) -> ConfigResult<ProjectConfig> {
    let config_dir = Path::new(&project_path).join(".jkcodingagent");
    let config_path = config_dir.join("config.toml");

    fs::create_dir_all(&config_dir).map_err(io_error("创建项目配置目录", config_dir.clone()))?;
    ensure_project_mcp_file(&project_path).map_err(ConfigError::Mcp)?;

    if !config_path.exists() {
        fs::write(&config_path, DEFAULT_CONFIG)
            .map_err(io_error("写入默认项目配置", config_path.clone()))?;
    }

    let raw =
        fs::read_to_string(&config_path).map_err(io_error("读取项目配置", config_path.clone()))?;
    let mut config: ProjectConfig =
        toml::from_str(&raw).map_err(|source| ConfigError::ParseProjectConfig {
            path: config_path.clone(),
            source,
        })?;

    // 反序列化会忽略旧字段；显式重写一次，确保磁盘上的 CLI 配置也真正消失。
    let mut updated = contains_legacy_agent_fields(&raw);
    if should_refresh_prompt_prefix(&config.agent.prompt_prefix) {
        config.agent.prompt_prefix = DEFAULT_AGENT_PROMPT_PREFIX.to_string();
        updated = true;
    }
    if updated {
        let raw = toml::to_string_pretty(&config).map_err(|source| {
            ConfigError::SerializeProjectConfig {
                path: config_path.clone(),
                source,
            }
        })?;
        atomic_write(&config_path, &raw)?;
    }

    Ok(config)
}

/// Reads `.jkcodingagent/config.toml` from the project directory.
/// Returns the default config if the file doesn't exist yet.
#[tauri::command]
pub fn read_project_config(project_path: String) -> CommandResult<ProjectConfig> {
    read_project_config_impl(&project_path)
        .with_context(|| format!("读取项目配置失败（{}）", project_path))
        .into_command_result()
}

fn read_project_config_impl(project_path: &str) -> ConfigResult<ProjectConfig> {
    let config_path = Path::new(&project_path)
        .join(".jkcodingagent")
        .join("config.toml");
    if !config_path.exists() {
        return Ok(ProjectConfig::default());
    }
    let raw =
        fs::read_to_string(&config_path).map_err(io_error("读取项目配置", config_path.clone()))?;
    let config: ProjectConfig =
        toml::from_str(&raw).map_err(|source| ConfigError::ParseProjectConfig {
            path: config_path.clone(),
            source,
        })?;
    Ok(config)
}

/// Writes updated config to `.jkcodingagent/config.toml`, creating the directory if needed.
#[tauri::command]
pub fn write_project_config(project_path: String, config: ProjectConfig) -> CommandResult<()> {
    write_project_config_impl(&project_path, config)
        .with_context(|| format!("写入项目配置失败（{}）", project_path))
        .into_command_result()
}

fn write_project_config_impl(project_path: &str, config: ProjectConfig) -> ConfigResult<()> {
    let config_dir = Path::new(&project_path).join(".jkcodingagent");
    fs::create_dir_all(&config_dir).map_err(io_error("创建项目配置目录", config_dir.clone()))?;
    let config_path = config_dir.join("config.toml");
    let raw =
        toml::to_string_pretty(&config).map_err(|source| ConfigError::SerializeProjectConfig {
            path: config_path.clone(),
            source,
        })?;
    Ok(atomic_write(&config_path, &raw)?)
}
