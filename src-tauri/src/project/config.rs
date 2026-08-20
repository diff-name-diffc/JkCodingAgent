use std::fs;
use std::path::Path;

use super::mcp::ensure_project_mcp_file;
use super::storage::StorageError;
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

const DEFAULT_COMMIT_PROMPT: &str = "你是一名资深软件工程师，请基于给定的 Git diff 生成提交信息。\n要求：\n1. 使用祈使句，直接描述本次改动。\n2. 第一行格式为 type(scope): summary，尽量不超过 50 个字符。\n3. type 仅使用 feat、fix、refactor、docs、style、test、chore。\n4. 如需补充说明，空一行后用 1-3 行说明原因、影响或验证重点。\n5. 只输出提交信息正文，不要解释，不要 Markdown。";

const DEFAULT_CONFIG: &str = r#"# JKCodingAgent 项目配置
# https://github.com/diff-name-diffc/JkCodingAgent

[git]
# 生成提交信息时使用的提示词（随仓库共享）
commit_prompt = "你是一名资深软件工程师，请基于给定的 Git diff 生成提交信息。\n要求：\n1. 使用祈使句，直接描述本次改动。\n2. 第一行格式为 type(scope): summary，尽量不超过 50 个字符。\n3. type 仅使用 feat、fix、refactor、docs、style、test、chore。\n4. 如需补充说明，空一行后用 1-3 行说明原因、影响或验证重点。\n5. 只输出提交信息正文，不要解释，不要 Markdown。"
"#;

/// 项目级配置：只保留随仓库共享的设置。应用级偏好（浏览器选项等）
/// 存全局库 app_config 表；历史上的 [agent] prompt_prefix 从未有消费者，已删除。
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct ProjectConfig {
    pub git: GitConfig,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct GitConfig {
    pub commit_prompt: String,
}

impl Default for ProjectConfig {
    fn default() -> Self {
        ProjectConfig {
            git: GitConfig {
                commit_prompt: DEFAULT_COMMIT_PROMPT.to_string(),
            },
        }
    }
}

/// Creates `.jkcodingagent/config.toml` in the project directory if it doesn't already exist.
/// Also ensures `.jkcodingagent/mcp.json` exists.
/// Returns the parsed config. 旧文件中的 [agent] / [browser] 段会被忽略（反序列化跳过未知字段）。
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

    read_project_config_impl(project_path)
}

/// Reads `.jkcodingagent/config.toml` from the project directory.
/// Returns the default config if the file doesn't exist yet.
///
/// 不再注册为前端命令：仅被 Rust 内部（git 提交信息生成）当普通函数调用。
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
