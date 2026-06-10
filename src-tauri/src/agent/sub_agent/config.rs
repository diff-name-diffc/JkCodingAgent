use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

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
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
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
            return Err(anyhow!("system_prompt 不能为空"));
        }
        if self.allowed_tools.is_empty() {
            return Err(anyhow!("allowed_tools 不能为空"));
        }
        if self.max_iterations < 1 || self.max_iterations > 100 {
            return Err(anyhow!("max_iterations 必须在 1-100 之间"));
        }
        if self.max_output_tokens < 256 || self.max_output_tokens > 65536 {
            return Err(anyhow!("max_output_tokens 必须在 256-65536 之间"));
        }
        if self.temperature < 0.0 || self.temperature > 2.0 {
            return Err(anyhow!("temperature 必须在 0-2 之间"));
        }
        if self.timeout_secs < 10 || self.timeout_secs > 1200 {
            return Err(anyhow!("timeout_secs 必须在 10-1200 之间"));
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
