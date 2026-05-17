use std::fs;
use std::path::Path;

use super::mcp::ensure_project_mcp_file;
use super::storage::atomic_write;
use crate::platform::{detect_claude_version, detect_codex_version};

const DEFAULT_AGENT_PROMPT_PREFIX: &str = "- 先围绕当前任务目标确认相关代码、约束和必要上下文。\n- 只做与目标直接相关的最小充分改动，避免无关重构。\n- 完成后简洁说明改动、验证结果和剩余风险。";

const PREVIOUS_DEFAULT_AGENT_PROMPT_PREFIX: &str = "你是一名资深软件工程师，当前目标是完成用户交付的编码任务。\n- 先阅读相关代码、调用链和约束，再动手修改。\n- 优先做与当前任务直接相关的最小充分改动，不无端扩散范围。\n- 对正确性、边界条件、兼容性和可维护性保持敏感。\n- 修改后主动做编译、测试或关键路径验证；若无法验证，明确说明。\n- 以操作和交付为主，少讲概念，少做长篇分析。\n- 过程输出保持简洁，只在必要时汇报关键判断、风险和下一步。\n- 不要重复复述用户需求，不要写冗长总结。\n- 收尾时只做简洁结果说明：改了什么、验证了什么、还有什么风险。";

const LEGACY_DEFAULT_AGENT_PROMPT_PREFIX: &str = "你是一名资深软件工程师，当前目标是完成用户交付的编码任务。\n- 先阅读相关代码、调用链和约束，再动手修改。\n- 优先做与当前任务直接相关的最小充分改动，不无端扩散范围。\n- 对正确性、边界条件、兼容性和可维护性保持敏感。\n- 修改后主动做编译、测试或关键路径验证；若无法验证，明确说明。\n- 输出聚焦结果、风险、验证结论和后续建议。";

const DEFAULT_COMMIT_PROMPT: &str = "你是一名资深软件工程师，请基于给定的 Git diff 生成提交信息。\n要求：\n1. 使用祈使句，直接描述本次改动。\n2. 第一行格式为 type(scope): summary，尽量不超过 50 个字符。\n3. type 仅使用 feat、fix、refactor、docs、style、test、chore。\n4. 如需补充说明，空一行后用 1-3 行说明原因、影响或验证重点。\n5. 只输出提交信息正文，不要解释，不要 Markdown。";

const DEFAULT_CONFIG: &str = r#"# JKCodingAgent 项目配置
# https://github.com/diff-name-diffc/JkCodingAgent

[agent]
# 新任务默认使用的智能体："claude" 或 "codex"
default = "claude"
# 每个任务提示词前自动追加的公共工程指令
prompt_prefix = "- 先围绕当前任务目标确认相关代码和文件路径、和子任务相关的上下文。\n- 只做与目标直接相关的最小充分改动，避免无关重构。\n- 完成后简洁说明改动、验证结果和剩余风险。"

# 自动检测回写的 Claude Code 版本，可留空
claude_version = ""
# 自动检测回写的 Codex 版本，可留空
codex_version = ""

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
                default: "claude".to_string(),
                prompt_prefix: DEFAULT_AGENT_PROMPT_PREFIX.to_string(),
                claude_version: String::new(),
                codex_version: String::new(),
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

/// Creates `.jkcodingagent/config.toml` in the project directory if it doesn't already exist.
/// Also ensures `.jkcodingagent/mcp.json` exists.
/// Returns the parsed config.
#[tauri::command]
pub fn init_project_config(project_path: String) -> Result<ProjectConfig, String> {
    let config_dir = Path::new(&project_path).join(".jkcodingagent");
    let config_path = config_dir.join("config.toml");

    fs::create_dir_all(&config_dir).map_err(|e| e.to_string())?;
    ensure_project_mcp_file(&project_path)?;

    if !config_path.exists() {
        fs::write(&config_path, DEFAULT_CONFIG).map_err(|e| e.to_string())?;
    }

    let raw = fs::read_to_string(&config_path).map_err(|e| e.to_string())?;
    let mut config: ProjectConfig = toml::from_str(&raw)
        .map_err(|e| format!("解析项目配置失败（{}）：{e}", config_path.display()))?;

    // 首次打开或版本字段为空时，自动检测并回写
    let mut updated = false;
    if should_refresh_prompt_prefix(&config.agent.prompt_prefix) {
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
    let config: ProjectConfig = toml::from_str(&raw)
        .map_err(|e| format!("解析项目配置失败（{}）：{e}", config_path.display()))?;
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

#[cfg(test)]
mod tests {
    use super::{
        should_refresh_prompt_prefix, BrowserConfig, ProjectConfig, DEFAULT_AGENT_PROMPT_PREFIX,
        LEGACY_DEFAULT_AGENT_PROMPT_PREFIX, PREVIOUS_DEFAULT_AGENT_PROMPT_PREFIX,
    };

    #[test]
    fn refreshes_empty_or_builtin_legacy_prompt_prefix() {
        assert!(should_refresh_prompt_prefix(""));
        assert!(should_refresh_prompt_prefix(
            PREVIOUS_DEFAULT_AGENT_PROMPT_PREFIX
        ));
        assert!(should_refresh_prompt_prefix(
            LEGACY_DEFAULT_AGENT_PROMPT_PREFIX
        ));
    }

    #[test]
    fn keeps_custom_prompt_prefix() {
        assert!(!should_refresh_prompt_prefix("请始终使用英文输出。"));
        assert!(!should_refresh_prompt_prefix(DEFAULT_AGENT_PROMPT_PREFIX));
    }

    #[test]
    fn project_config_accepts_missing_browser_section() {
        let raw = r#"
[agent]
default = "claude"
prompt_prefix = ""
claude_version = ""
codex_version = ""

[git]
commit_prompt = "commit"
"#;
        let config: ProjectConfig = toml::from_str(raw).expect("parse legacy project config");

        assert!(config.browser.enabled);
        assert_eq!(config.browser.viewport_width, 1280);
        assert_eq!(config.browser.viewport_height, 800);
    }

    // ── Additional tests ──────────────────────────────────────────────────────

    #[test]
    fn default_project_config_has_expected_values() {
        let config = ProjectConfig::default();
        assert_eq!(config.agent.default, "claude");
        assert!(!config.agent.prompt_prefix.is_empty());
        assert!(config.agent.claude_version.is_empty());
        assert!(config.agent.codex_version.is_empty());
        assert!(!config.git.commit_prompt.is_empty());
        assert!(config.browser.enabled);
        assert_eq!(config.browser.viewport_width, 1280);
        assert_eq!(config.browser.viewport_height, 800);
        assert!(config.browser.proxy.is_empty());
        assert!(config.browser.locale.is_empty());
        assert!(config.browser.timezone.is_empty());
    }

    #[test]
    fn default_browser_config_values() {
        let browser = BrowserConfig::default();
        assert!(browser.enabled);
        assert_eq!(browser.viewport_width, 1280);
        assert_eq!(browser.viewport_height, 800);
        assert!(browser.proxy.is_empty());
        assert!(browser.locale.is_empty());
        assert!(browser.timezone.is_empty());
    }

    #[test]
    fn parse_full_config_toml() {
        let config: ProjectConfig = toml::from_str(super::DEFAULT_CONFIG)
            .expect("DEFAULT_CONFIG should parse");
        assert_eq!(config.agent.default, "claude");
        assert!(!config.agent.prompt_prefix.is_empty());
        assert!(config.browser.enabled);
        assert_eq!(config.browser.viewport_width, 1280);
        assert_eq!(config.browser.viewport_height, 800);
    }

    #[test]
    fn config_roundtrip_toml() {
        let original = ProjectConfig::default();
        let serialized = toml::to_string_pretty(&original).expect("serialize");
        let deserialized: ProjectConfig = toml::from_str(&serialized).expect("deserialize");
        assert_eq!(original.agent.default, deserialized.agent.default);
        assert_eq!(original.agent.prompt_prefix, deserialized.agent.prompt_prefix);
        assert_eq!(original.git.commit_prompt, deserialized.git.commit_prompt);
        assert_eq!(original.browser.enabled, deserialized.browser.enabled);
        assert_eq!(original.browser.viewport_width, deserialized.browser.viewport_width);
        assert_eq!(original.browser.viewport_height, deserialized.browser.viewport_height);
    }

    #[test]
    fn config_json_roundtrip_via_serde_json() {
        let original = ProjectConfig::default();
        let json = serde_json::to_string(&original).expect("serialize to json");
        let deserialized: ProjectConfig = serde_json::from_str(&json).expect("deserialize from json");
        assert_eq!(original.agent.default, deserialized.agent.default);
    }

    #[test]
    fn parse_config_with_custom_browser_values() {
        let raw = r#"
[agent]
default = "codex"
prompt_prefix = "custom prefix"
claude_version = "1.2.3"
codex_version = "4.5.6"

[git]
commit_prompt = "custom commit"

[browser]
enabled = false
proxy = "socks5://host:1080"
locale = "en-US"
timezone = "America/New_York"
viewport_width = 1920
viewport_height = 1080
"#;
        let config: ProjectConfig = toml::from_str(raw).expect("parse custom config");
        assert_eq!(config.agent.default, "codex");
        assert_eq!(config.agent.prompt_prefix, "custom prefix");
        assert_eq!(config.agent.claude_version, "1.2.3");
        assert_eq!(config.agent.codex_version, "4.5.6");
        assert_eq!(config.git.commit_prompt, "custom commit");
        assert!(!config.browser.enabled);
        assert_eq!(config.browser.proxy, "socks5://host:1080");
        assert_eq!(config.browser.locale, "en-US");
        assert_eq!(config.browser.timezone, "America/New_York");
        assert_eq!(config.browser.viewport_width, 1920);
        assert_eq!(config.browser.viewport_height, 1080);
    }

    #[test]
    fn parse_config_missing_agent_field_fails() {
        let raw = r#"
[git]
commit_prompt = "hello"
"#;
        let result = toml::from_str::<ProjectConfig>(raw);
        assert!(result.is_err());
    }

    #[test]
    fn parse_config_missing_git_field_fails() {
        let raw = r#"
[agent]
default = "claude"
"#;
        let result = toml::from_str::<ProjectConfig>(raw);
        assert!(result.is_err());
    }

    #[test]
    fn parse_config_invalid_toml_fails() {
        let raw = "this is not valid toml [[[";
        let result = toml::from_str::<ProjectConfig>(raw);
        assert!(result.is_err());
    }

    #[test]
    fn parse_config_empty_string_fails() {
        let result = toml::from_str::<ProjectConfig>("");
        assert!(result.is_err());
    }

    #[test]
    fn should_refresh_prompt_prefix_current_default_is_not_refreshed() {
        // The current default should NOT be refreshed (it is already up to date)
        assert!(!should_refresh_prompt_prefix(
            super::DEFAULT_AGENT_PROMPT_PREFIX
        ));
    }

    #[test]
    fn should_refresh_prompt_prefix_custom_is_not_refreshed() {
        assert!(!should_refresh_prompt_prefix("My custom prefix."));
    }

    #[test]
    fn should_refresh_prompt_prefix_legacy_versions_are_refreshed() {
        assert!(should_refresh_prompt_prefix(""));
        assert!(should_refresh_prompt_prefix(
            super::PREVIOUS_DEFAULT_AGENT_PROMPT_PREFIX
        ));
        assert!(should_refresh_prompt_prefix(
            super::LEGACY_DEFAULT_AGENT_PROMPT_PREFIX
        ));
    }

    #[test]
    fn agent_config_serializes_field_names_correctly() {
        let agent = super::AgentConfig {
            default: "claude".to_string(),
            prompt_prefix: "test".to_string(),
            claude_version: "1.0".to_string(),
            codex_version: "".to_string(),
        };
        let json = serde_json::to_string(&agent).expect("serialize");
        assert!(json.contains("\"default\""));
        assert!(json.contains("\"prompt_prefix\""));
        assert!(json.contains("\"claude_version\""));
        // Empty string should still be serialized (not skipped)
        assert!(json.contains("\"codex_version\""));
    }

    #[test]
    fn browser_config_deserializes_partial_fields() {
        let raw = r#"
enabled = false
"#;
        let browser: BrowserConfig = toml::from_str(raw).expect("parse partial browser");
        assert!(!browser.enabled);
        assert_eq!(browser.viewport_width, 1280);
        assert_eq!(browser.viewport_height, 800);
        assert!(browser.proxy.is_empty());
    }

    #[test]
    fn init_project_config_creates_config_file() {
        let tmp = std::env::temp_dir().join(format!(
            "test_config_init_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).expect("create tmp dir");

        let result = super::init_project_config(tmp.to_string_lossy().to_string());
        assert!(result.is_ok(), "init_project_config should succeed");

        let config_path = tmp.join(".jkcodingagent").join("config.toml");
        assert!(config_path.exists(), "config.toml should be created");

        let mcp_path = tmp.join(".jkcodingagent").join("mcp.json");
        assert!(mcp_path.exists(), "mcp.json should be created");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn init_project_config_idempotent() {
        let tmp = std::env::temp_dir().join(format!(
            "test_config_idem_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).expect("create tmp dir");

        let config1 = super::init_project_config(tmp.to_string_lossy().to_string())
            .expect("first init");
        let config2 = super::init_project_config(tmp.to_string_lossy().to_string())
            .expect("second init");

        // Both should return valid configs with the same default agent
        assert_eq!(config1.agent.default, config2.agent.default);
        assert_eq!(config1.agent.default, "claude");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn read_project_config_returns_default_when_no_file() {
        let tmp = std::env::temp_dir().join(format!(
            "test_config_read_missing_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).expect("create tmp dir");

        let result = super::read_project_config(tmp.to_string_lossy().to_string());
        assert!(result.is_ok(), "should return Ok with default config");
        let config = result.unwrap();
        assert_eq!(config.agent.default, "claude");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn write_then_read_project_config_roundtrip() {
        let tmp = std::env::temp_dir().join(format!(
            "test_config_rw_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).expect("create tmp dir");

        let mut config = ProjectConfig::default();
        config.agent.default = "codex".to_string();
        config.agent.prompt_prefix = "custom prefix".to_string();
        config.browser.enabled = false;

        super::write_project_config(tmp.to_string_lossy().to_string(), config.clone())
            .expect("write config");

        let read_config =
            super::read_project_config(tmp.to_string_lossy().to_string()).expect("read config");

        assert_eq!(read_config.agent.default, "codex");
        assert_eq!(read_config.agent.prompt_prefix, "custom prefix");
        assert!(!read_config.browser.enabled);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn read_agent_config_file_returns_none_for_missing_file() {
        // Use a temp path that won't interfere with real agent configs.
        // We can't easily test this without mocking HOME, so we test
        // the error case for an unknown agent.
        let result = super::read_agent_config_file("unknown_agent".to_string());
        assert!(result.is_err(), "unknown agent should return error");
    }

    #[test]
    fn write_agent_config_file_rejects_unknown_agent() {
        let result = super::write_agent_config_file(
            "unknown_agent".to_string(),
            "content".to_string(),
        );
        assert!(result.is_err(), "unknown agent should return error");
    }

    #[test]
    fn atomic_write_creates_file() {
        let tmp = std::env::temp_dir().join(format!(
            "test_atomic_write_{}.txt",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&tmp);

        super::super::storage::atomic_write(&tmp, "hello world").expect("atomic write");
        let content = std::fs::read_to_string(&tmp).expect("read back");
        assert_eq!(content, "hello world");

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn atomic_write_overwrites_existing_file() {
        let tmp = std::env::temp_dir().join(format!(
            "test_atomic_overwrite_{}.txt",
            std::process::id()
        ));

        super::super::storage::atomic_write(&tmp, "first").expect("write first");
        super::super::storage::atomic_write(&tmp, "second").expect("write second");

        let content = std::fs::read_to_string(&tmp).expect("read back");
        assert_eq!(content, "second");

        let _ = std::fs::remove_file(&tmp);
    }
}
