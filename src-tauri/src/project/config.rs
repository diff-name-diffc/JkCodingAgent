use std::fs;
use std::path::Path;

use super::mcp::ensure_project_mcp_file;
use super::storage::atomic_write;
use crate::platform::{detect_claude_version, detect_codex_version};

const DEFAULT_AGENT_PROMPT_PREFIX: &str = "你是一名资深软件工程师，当前目标是完成用户交付的编码任务。\n- 先阅读相关代码、调用链和约束，再动手修改。\n- 优先做与当前任务直接相关的最小充分改动，不无端扩散范围。\n- 对正确性、边界条件、兼容性和可维护性保持敏感。\n- 修改后主动做编译、测试或关键路径验证；若无法验证，明确说明。\n- 以操作和交付为主，少讲概念，少做长篇分析。\n- 过程输出保持简洁，只在必要时汇报关键判断、风险和下一步。\n- 不要重复复述用户需求，不要写冗长总结。\n- 收尾时只做简洁结果说明：改了什么、验证了什么、还有什么风险。";

const LEGACY_DEFAULT_AGENT_PROMPT_PREFIX: &str = "你是一名资深软件工程师，当前目标是完成用户交付的编码任务。\n- 先阅读相关代码、调用链和约束，再动手修改。\n- 优先做与当前任务直接相关的最小充分改动，不无端扩散范围。\n- 对正确性、边界条件、兼容性和可维护性保持敏感。\n- 修改后主动做编译、测试或关键路径验证；若无法验证，明确说明。\n- 输出聚焦结果、风险、验证结论和后续建议。";

const DEFAULT_COMMIT_PROMPT: &str = "你是一名资深软件工程师，请基于给定的 Git diff 生成提交信息。\n要求：\n1. 使用祈使句，直接描述本次改动。\n2. 第一行格式为 type(scope): summary，尽量不超过 50 个字符。\n3. type 仅使用 feat、fix、refactor、docs、style、test、chore。\n4. 如需补充说明，空一行后用 1-3 行说明原因、影响或验证重点。\n5. 只输出提交信息正文，不要解释，不要 Markdown。";

const DEFAULT_CONFIG: &str = r#"# JKCodingAgent 项目配置
# https://github.com/diff-name-diffc/JkCodingAgent

[agent]
# 新任务默认使用的智能体："claude" 或 "codex"
default = "claude"
# 每个任务提示词前自动追加的公共工程指令
prompt_prefix = "你是一名资深软件工程师，当前目标是完成用户交付的编码任务。\n- 先阅读相关代码、调用链和约束，再动手修改。\n- 优先做与当前任务直接相关的最小充分改动，不无端扩散范围。\n- 对正确性、边界条件、兼容性和可维护性保持敏感。\n- 修改后主动做编译、测试或关键路径验证；若无法验证，明确说明。\n- 以操作和交付为主，少讲概念，少做长篇分析。\n- 过程输出保持简洁，只在必要时汇报关键判断、风险和下一步。\n- 不要重复复述用户需求，不要写冗长总结。\n- 收尾时只做简洁结果说明：改了什么、验证了什么、还有什么风险。"

# 自动检测回写的 Claude Code 版本，可留空
claude_version = ""
# 自动检测回写的 Codex 版本，可留空
codex_version = ""

[git]
# 生成提交信息时使用的提示词
commit_prompt = "你是一名资深软件工程师，请基于给定的 Git diff 生成提交信息。\n要求：\n1. 使用祈使句，直接描述本次改动。\n2. 第一行格式为 type(scope): summary，尽量不超过 50 个字符。\n3. type 仅使用 feat、fix、refactor、docs、style、test、chore。\n4. 如需补充说明，空一行后用 1-3 行说明原因、影响或验证重点。\n5. 只输出提交信息正文，不要解释，不要 Markdown。"
"#;

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct AgentConfig {
    pub default: String,
    #[serde(default)]
    pub prompt_prefix: String,
    #[serde(default)]
    pub claude_version: String,
    #[serde(default)]
    pub codex_version: String,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct GitConfig {
    pub commit_prompt: String,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct ProjectConfig {
    pub agent: AgentConfig,
    pub git: GitConfig,
}

impl Default for ProjectConfig {
    fn default() -> Self {
        ProjectConfig {
            agent: AgentConfig {
                default: "claude".to_string(),
                prompt_prefix: DEFAULT_AGENT_PROMPT_PREFIX.to_string(),
                claude_version: String::new(),
                codex_version: String::new(),
            },
            git: GitConfig {
                commit_prompt: DEFAULT_COMMIT_PROMPT.to_string(),
            },
        }
    }
}

/// Creates `.jkcodingagent/config.toml` in the project directory if it doesn't already exist.
/// Also ensures `.jkcodingagent/attachments/` and `.jkcodingagent/mcp.json` exist.
/// Returns the parsed config.
#[tauri::command]
pub fn init_project_config(project_path: String) -> Result<ProjectConfig, String> {
    let config_dir = Path::new(&project_path).join(".jkcodingagent");
    let config_path = config_dir.join("config.toml");
    let attachments_dir = config_dir.join("attachments");

    fs::create_dir_all(&attachments_dir).map_err(|e| e.to_string())?;
    ensure_project_mcp_file(&project_path)?;

    if !config_path.exists() {
        fs::write(&config_path, DEFAULT_CONFIG).map_err(|e| e.to_string())?;
    }

    let raw = fs::read_to_string(&config_path).map_err(|e| e.to_string())?;
    let mut config: ProjectConfig = toml::from_str(&raw).unwrap_or_default();

    // 首次打开或版本字段为空时，自动检测并回写
    let mut updated = false;
    if config.agent.prompt_prefix.is_empty()
        || config.agent.prompt_prefix == LEGACY_DEFAULT_AGENT_PROMPT_PREFIX
    {
        config.agent.prompt_prefix = DEFAULT_AGENT_PROMPT_PREFIX.to_string();
        updated = true;
    }
    if config.agent.claude_version.is_empty() {
        if let Some(v) = detect_claude_version() {
            config.agent.claude_version = v;
            updated = true;
        }
    }
    if config.agent.codex_version.is_empty() {
        if let Some(v) = detect_codex_version() {
            config.agent.codex_version = v;
            updated = true;
        }
    }
    if updated {
        if let Ok(raw) = toml::to_string_pretty(&config) {
            let _ = atomic_write(&config_path, &raw);
        }
    }

    Ok(config)
}

/// Reads `.jkcodingagent/config.toml` from the project directory.
/// Returns the default config if the file doesn't exist yet.
#[tauri::command]
pub fn read_project_config(project_path: String) -> Result<ProjectConfig, String> {
    let config_path = Path::new(&project_path)
        .join(".jkcodingagent")
        .join("config.toml");
    if !config_path.exists() {
        return Ok(ProjectConfig::default());
    }
    let raw = fs::read_to_string(&config_path).map_err(|e| e.to_string())?;
    let config: ProjectConfig = toml::from_str(&raw).unwrap_or_default();
    Ok(config)
}

/// Writes updated config to `.jkcodingagent/config.toml`, creating the directory if needed.
#[tauri::command]
pub fn write_project_config(project_path: String, config: ProjectConfig) -> Result<(), String> {
    let config_dir = Path::new(&project_path).join(".jkcodingagent");
    fs::create_dir_all(&config_dir).map_err(|e| e.to_string())?;
    let config_path = config_dir.join("config.toml");
    let raw = toml::to_string_pretty(&config).map_err(|e| e.to_string())?;
    atomic_write(&config_path, &raw)
}

fn home_dir() -> Result<std::path::PathBuf, String> {
    std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .ok_or_else(|| "找不到用户主目录".to_string())
}

fn agent_config_path(agent: &str) -> Result<std::path::PathBuf, String> {
    let home = home_dir()?;
    match agent {
        "claude" => Ok(home.join(".claude").join("settings.json")),
        "codex" => Ok(home.join(".codex").join("config.toml")),
        _ => Err(format!("Unknown agent: {}", agent)),
    }
}

/// Reads the local settings file for the given agent ("claude" or "codex").
/// Returns None if the file doesn't exist.
#[tauri::command]
pub fn read_agent_config_file(agent: String) -> Result<Option<String>, String> {
    let path = agent_config_path(&agent)?;
    if !path.exists() {
        return Ok(None);
    }
    fs::read_to_string(&path)
        .map(Some)
        .map_err(|e| e.to_string())
}

/// Writes raw content back to the agent's local settings file.
#[tauri::command]
pub fn write_agent_config_file(agent: String, content: String) -> Result<(), String> {
    let path = agent_config_path(&agent)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    atomic_write(&path, &content)
}
