use std::collections::{BTreeSet, HashSet};

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

/// timeout_secs 允许的上限（秒）。超过该值的任务视为配置错误，避免长时间挂起。
pub const MAX_TIMEOUT_SECS: u64 = 3600;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentModelConfig {
    pub inherit_from_parent: bool,
    pub api_base: Option<String>,
    pub api_key: Option<String>,
    pub model_name: Option<String>,
}

impl Default for SubAgentModelConfig {
    fn default() -> Self {
        Self {
            inherit_from_parent: true,
            api_base: None,
            api_key: None,
            model_name: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentConfig {
    pub agent_id: String,
    pub agent_name: String,
    pub description: String,
    pub system_prompt: String,
    #[serde(default = "default_user_prompt_template")]
    pub user_prompt_template: String,
    pub allowed_tools: Vec<String>,
    #[serde(default)]
    pub model_config: SubAgentModelConfig,
    #[serde(default = "default_max_iterations")]
    pub max_iterations: u32,
    #[serde(default = "default_max_output_tokens")]
    pub max_output_tokens: u32,
    #[serde(default = "default_temperature")]
    pub temperature: f64,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_timestamp")]
    pub created_at: i64,
    #[serde(default = "default_timestamp")]
    pub updated_at: i64,
}

fn default_user_prompt_template() -> String {
    "{{task}}".to_string()
}

fn default_max_iterations() -> u32 {
    20
}

fn default_max_output_tokens() -> u32 {
    4096
}

fn default_temperature() -> f64 {
    0.7
}

fn default_timeout_secs() -> u64 {
    120
}

fn default_true() -> bool {
    true
}

/// 时间戳缺省值：字段缺失时生成当前毫秒时间戳，避免落到 1970-01-01。
fn default_timestamp() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

impl SubAgentConfig {
    pub fn validate(&self) -> Result<()> {
        if self.agent_id.is_empty() {
            return Err(anyhow!("agent_id 不能为空"));
        }
        if self.agent_id.len() > 64 {
            return Err(anyhow!("agent_id 长度不能超过 64"));
        }
        if !self
            .agent_id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
        {
            return Err(anyhow!("agent_id 仅支持小写字母、数字、下划线和短横线"));
        }
        if !self
            .agent_id
            .chars()
            .next()
            .map(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
            .unwrap_or(false)
        {
            return Err(anyhow!("agent_id 必须以小写字母或数字开头"));
        }
        if self.agent_name.is_empty() || self.agent_name.len() > 64 {
            return Err(anyhow!("agent_name 长度必须在 1-64 之间"));
        }
        if self.description.is_empty() || self.description.len() > 512 {
            return Err(anyhow!("description 长度必须在 1-512 之间"));
        }
        if self.system_prompt.is_empty() {
            return Err(anyhow!("错误：system_prompt 不能为空"));
        }
        if self.user_prompt_template.trim().is_empty() {
            return Err(anyhow!("错误：user_prompt_template 不能为空"));
        }
        if self.allowed_tools.is_empty() {
            return Err(anyhow!("错误：allowed_tools 不能为空"));
        }
        // 声明不继承父级模型配置时，必须自带模型名；api_base/api_key 缺失会在
        // 运行期回退到父级配置（见 runtime.rs），属于有意保留的降级行为。
        if !self.model_config.inherit_from_parent
            && self
                .model_config
                .model_name
                .as_deref()
                .map(str::trim)
                .map(str::is_empty)
                .unwrap_or(true)
        {
            return Err(anyhow!(
                "错误：inherit_from_parent 为 false 时 model_name 不能为空"
            ));
        }
        if self.max_iterations < 1 || self.max_iterations > 100 {
            return Err(anyhow!("错误：max_iterations 必须在 1-100 之间"));
        }
        if self.max_output_tokens < 256 || self.max_output_tokens > 65536 {
            return Err(anyhow!("错误：max_output_tokens 必须在 256-65536 之间"));
        }
        // 用 is_finite + contains 拦截 NaN/无穷大：IEEE 754 下 NaN 与任何值
        // 的比较都为 false，裸 `<`/`>` 判断会让 NaN 通过校验。
        if !self.temperature.is_finite() || !(0.0..=2.0).contains(&self.temperature) {
            return Err(anyhow!("错误：temperature 必须在 0-2 之间"));
        }
        if self.timeout_secs < 1 || self.timeout_secs > MAX_TIMEOUT_SECS {
            return Err(anyhow!(
                "错误：timeout_secs 必须在 1-{MAX_TIMEOUT_SECS} 之间"
            ));
        }
        Ok(())
    }

    /// 校验 allowed_tools 是否都在已知工具集合内（系统边界调用）。
    ///
    /// 与 `validate()` 拆分的原因：工具注册表属于应用层资源，`SubAgentConfig`
    /// 作为纯数据结构不持有它；由 commands 层在创建/更新入口取注册表名称集合后调用。
    pub fn validate_allowed_tools(&self, known_tool_names: &HashSet<String>) -> Result<()> {
        let unknown: BTreeSet<&str> = self
            .allowed_tools
            .iter()
            .filter(|name| !known_tool_names.contains(name.as_str()))
            .map(String::as_str)
            .collect();
        if !unknown.is_empty() {
            return Err(anyhow!(
                "错误：allowed_tools 包含未注册的工具：{}",
                unknown.into_iter().collect::<Vec<_>>().join("、")
            ));
        }
        Ok(())
    }

    pub fn browser_agent_default() -> Self {
        Self {
            agent_id: "browser-agent".to_string(),
            agent_name: "浏览器助手".to_string(),
            description: "处理网页搜索、页面浏览、信息提取和页面自动化任务。适合需要从互联网获取信息的场景，如搜索文档、查询 API、阅读网页内容、填写表单等。".to_string(),
            system_prompt: concat!(
                "你是一个专业的浏览器自动化助手。你通过 CloakBrowser 工具与网页交互。\n\n",
                "## 工作流程\n",
                "1. 使用 browser_read_text 获取页面可访问性树快照，获取元素 ref\n",
                "2. 使用 ref 与具体元素交互（click, type）\n",
                "3. 使用 browser_visual_analyze 进行视觉分析（仅在文本信息不足时使用）\n\n",
                "## 元素引用（ref）规则——极其重要\n",
                "- ref 只在最近一次 browser_read_text 返回的快照中有效\n",
                "- 任何可能导致页面变化的操作（点击链接/按钮、表单提交、URL 导航）后，旧 ref 可能失效\n",
                "- 如果收到 'ref 已失效' 错误，系统会自动附上最新快照，你必须基于新快照中的 ref 重新定位目标元素\n",
                "- 绝不要在不同快照之间混用 ref 编号\n",
                "- 每次页面变化后，优先重新调用 browser_read_text 而不是复用旧 ref\n\n",
                "## 错误处理\n",
                "- 元素引用失效：系统会自动获取新快照并附在错误响应中，基于新快照重试即可\n",
                "- 超时错误：检查页面是否加载完成，可用 browser_wait_for 等待后再试\n",
                "- 系统错误（进程崩溃等）：这些不可恢复，直接向用户报告\n\n",
                "## 输出规范\n",
                "- 完成任务后，输出结构化的信息提取结果\n",
                "- 如果搜索未找到结果，明确说明并建议替代方案\n",
                "- 不要输出原始的 HTML 或可访问性树内容\n\n",
                "## 约束\n",
                "- 不要访问需要登录的页面，除非用户明确指示\n",
                "- 每次操作后验证页面状态\n",
                "- 遇到验证码或反爬机制时，通知用户",
            ).to_string(),
            user_prompt_template: "{{task}}".to_string(),
            allowed_tools: vec![
                "browser_open_url".to_string(),
                "browser_click".to_string(),
                "browser_type".to_string(),
                "browser_press".to_string(),
                "browser_wait_for".to_string(),
                "browser_read_text".to_string(),
                "browser_visual_analyze".to_string(),
                "browser_close".to_string(),
            ],
            model_config: SubAgentModelConfig::default(),
            max_iterations: 25,
            max_output_tokens: 4096,
            temperature: 0.7,
            timeout_secs: 600,
            enabled: true,
            created_at: chrono::Utc::now().timestamp_millis(),
            updated_at: chrono::Utc::now().timestamp_millis(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_config() -> SubAgentConfig {
        SubAgentConfig::browser_agent_default()
    }

    #[test]
    fn browser_agent_default_passes_validation() {
        base_config().validate().expect("default config must be valid");
    }

    #[test]
    fn blank_user_prompt_template_is_rejected() {
        let mut config = base_config();
        config.user_prompt_template = "   ".to_string();
        let error = config.validate().expect_err("blank template must fail");
        assert!(error.to_string().contains("user_prompt_template"));
    }

    #[test]
    fn independent_model_config_requires_model_name() {
        let mut config = base_config();
        config.model_config.inherit_from_parent = false;
        config.model_config.model_name = None;
        let error = config
            .validate()
            .expect_err("missing model_name must fail when not inheriting");
        assert!(error.to_string().contains("model_name"));

        config.model_config.model_name = Some("  ".to_string());
        assert!(config.validate().is_err(), "blank model_name must fail");

        config.model_config.model_name = Some("gpt-test".to_string());
        config.validate().expect("model_name present must pass");
    }

    #[test]
    fn timeout_secs_range_is_enforced() {
        let mut config = base_config();
        config.timeout_secs = 0;
        assert!(config.validate().is_err(), "timeout 0 must fail");
        config.timeout_secs = MAX_TIMEOUT_SECS + 1;
        assert!(config.validate().is_err(), "timeout above cap must fail");
        config.timeout_secs = 600;
        config.validate().expect("timeout 600 must pass");
    }

    #[test]
    fn temperature_rejects_nan_and_out_of_range() {
        let mut config = base_config();
        config.temperature = f64::NAN;
        assert!(config.validate().is_err(), "NaN must fail");
        config.temperature = f64::INFINITY;
        assert!(config.validate().is_err(), "infinity must fail");
        config.temperature = -0.1;
        assert!(config.validate().is_err(), "negative must fail");
        config.temperature = 2.1;
        assert!(config.validate().is_err(), "above 2 must fail");
        config.temperature = 0.0;
        config.validate().expect("0.0 must pass");
        config.temperature = 2.0;
        config.validate().expect("2.0 must pass");
    }

    #[test]
    fn timestamps_default_to_now_when_missing() {
        let json = r#"{
            "agent_id": "demo-agent",
            "agent_name": "demo",
            "description": "demo",
            "system_prompt": "demo",
            "allowed_tools": ["browser_open_url"]
        }"#;
        let config: SubAgentConfig = serde_json::from_str(json).expect("deserialize");
        let before = chrono::Utc::now().timestamp_millis();
        assert!(
            config.created_at > 0 && config.updated_at > 0,
            "missing timestamps must default to current millis, got {} / {}",
            config.created_at,
            config.updated_at
        );
        assert!(config.created_at <= before + 1000);
    }

    #[test]
    fn allowed_tools_checked_against_known_registry_names() {
        let mut config = base_config();
        let known: HashSet<String> = ["browser_open_url", "browser_click"]
            .into_iter()
            .map(str::to_string)
            .collect();

        config.allowed_tools = vec!["browser_open_url".to_string()];
        config
            .validate_allowed_tools(&known)
            .expect("known tool must pass");

        config.allowed_tools = vec![
            "browser_open_url".to_string(),
            "browser_typo".to_string(),
            "ghost_tool".to_string(),
        ];
        let error = config
            .validate_allowed_tools(&known)
            .expect_err("unknown tools must fail");
        let message = error.to_string();
        assert!(message.contains("browser_typo"));
        assert!(message.contains("ghost_tool"));
        assert!(!message.contains("browser_open_url"));
    }
}
