use async_trait::async_trait;
use serde_json::{json, Value};

use super::context::ToolContext;
use super::registry::AgentTool;

pub(super) fn delegation_tools() -> Vec<Box<dyn AgentTool>> {
    vec![
        Box::new(DispatchAgentTool::new(DispatchAgent::Claude)),
        Box::new(DispatchAgentTool::new(DispatchAgent::Codex)),
        Box::new(ContinueAgentSessionTool::new(DispatchAgent::Claude)),
        Box::new(ContinueAgentSessionTool::new(DispatchAgent::Codex)),
        Box::new(ExitAgentSessionTool::new(DispatchAgent::Claude)),
        Box::new(ExitAgentSessionTool::new(DispatchAgent::Codex)),
    ]
}

struct DispatchAgentTool {
    agent: DispatchAgent,
}

struct ContinueAgentSessionTool {
    agent: DispatchAgent,
}

struct ExitAgentSessionTool {
    agent: DispatchAgent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DispatchAgent {
    Claude,
    Codex,
}

impl DispatchAgent {
    pub fn slug(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
        }
    }

    pub fn from_dispatch_tool_name(name: &str) -> Option<Self> {
        match name {
            "dispatch_claude" => Some(Self::Claude),
            "dispatch_codex" => Some(Self::Codex),
            _ => None,
        }
    }

    pub fn from_continue_tool_name(name: &str) -> Option<Self> {
        match name {
            "continue_claude_session" => Some(Self::Claude),
            "continue_codex_session" => Some(Self::Codex),
            _ => None,
        }
    }

    pub fn from_exit_tool_name(name: &str) -> Option<Self> {
        match name {
            "exit_claude_session" => Some(Self::Claude),
            "exit_codex_session" => Some(Self::Codex),
            _ => None,
        }
    }

    fn tool_name(self) -> &'static str {
        match self {
            Self::Claude => "dispatch_claude",
            Self::Codex => "dispatch_codex",
        }
    }

    fn continue_tool_name(self) -> &'static str {
        match self {
            Self::Claude => "continue_claude_session",
            Self::Codex => "continue_codex_session",
        }
    }

    fn exit_tool_name(self) -> &'static str {
        match self {
            Self::Claude => "exit_claude_session",
            Self::Codex => "exit_codex_session",
        }
    }

    fn dispatch_description(self) -> &'static str {
        match self {
            Self::Claude => {
                "把编码任务委派给 Claude 终端执行。适合新功能、快速迭代、探索性调试、算法尝试和需要边实现边收敛的任务。发起前应先完成调查，并提供自包含的任务说明。子任务回流到主调度时默认只同步任务摘要，不回灌完整终端日志。"
            }
            Self::Codex => {
                "把编码任务委派给 Codex 终端执行。适合重构、结构整理、跨文件一致性修改、回归风险较高和需要严谨验证的任务。发起前应先完成调查，并提供自包含的任务说明。子任务回流到主调度时默认只同步任务摘要，不回灌完整终端日志。"
            }
        }
    }

    fn continue_description(self) -> &'static str {
        match self {
            Self::Claude => {
                "向正在运行的 Claude 会话发送后续指令，用于在同一个 Claude 子进程中继续推进任务。阶段性执行结果回流时默认同步摘要。"
            }
            Self::Codex => {
                "向正在运行的 Codex 会话发送后续指令，用于在同一个 Codex 子进程中继续推进任务。阶段性执行结果回流时默认同步摘要。"
            }
        }
    }

    fn exit_description(self) -> &'static str {
        match self {
            Self::Claude => {
                "向 Claude 子进程发送 /exit 以结束当前会话。仅在该子任务确认完成后使用。"
            }
            Self::Codex => "向 Codex 子进程发送 /exit 以结束当前会话。仅在该子任务确认完成后使用。",
        }
    }
}

impl DispatchAgentTool {
    fn new(agent: DispatchAgent) -> Self {
        Self { agent }
    }
}

impl ContinueAgentSessionTool {
    fn new(agent: DispatchAgent) -> Self {
        Self { agent }
    }
}

impl ExitAgentSessionTool {
    fn new(agent: DispatchAgent) -> Self {
        Self { agent }
    }
}

#[async_trait]
impl AgentTool for DispatchAgentTool {
    fn name(&self) -> &'static str {
        self.agent.tool_name()
    }

    fn description(&self) -> &'static str {
        self.agent.dispatch_description()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task_description": {
                    "type": "string",
                    "description": format!("交给 {} 执行的自包含任务说明，应包含目标、上下文、相关文件、约束和验证方式", self.agent.display_name())
                },
                "permission_mode": {
                    "type": "string",
                    "description": format!("{} 的权限模式：ask（默认权限）、auto_edit（自动接受编辑）、full_access（跳过权限确认）", self.agent.display_name()),
                    "enum": ["ask", "auto_edit", "full_access"],
                    "default": "full_access"
                }
            },
            "required": ["task_description"]
        })
    }

    async fn execute(&self, args: &Value, _context: &ToolContext) -> String {
        let task_description = match string_arg(args, "task_description") {
            Some(description) if !description.trim().is_empty() => description,
            _ => return "错误：task_description 为必填项，且不能为空".to_string(),
        };
        let permission_mode =
            string_arg(args, "permission_mode").unwrap_or_else(|| "full_access".to_string());

        format!(
            "已记录向 {} 提交子任务请求（权限：{}，说明长度：{} 字符）",
            self.agent.display_name(),
            permission_mode,
            task_description.chars().count()
        )
    }
}

#[async_trait]
impl AgentTool for ContinueAgentSessionTool {
    fn name(&self) -> &'static str {
        self.agent.continue_tool_name()
    }

    fn description(&self) -> &'static str {
        self.agent.continue_description()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task_description": {
                    "type": "string",
                    "description": format!("发送给当前 {} 会话的后续指令", self.agent.display_name())
                }
            },
            "required": ["task_description"]
        })
    }

    async fn execute(&self, args: &Value, _context: &ToolContext) -> String {
        let task_description = match string_arg(args, "task_description") {
            Some(description) if !description.trim().is_empty() => description,
            _ => return "错误：task_description 为必填项，且不能为空".to_string(),
        };
        format!(
            "已记录发送到 {} 会话的后续指令（{} 字符）",
            self.agent.display_name(),
            task_description.chars().count()
        )
    }
}

#[async_trait]
impl AgentTool for ExitAgentSessionTool {
    fn name(&self) -> &'static str {
        self.agent.exit_tool_name()
    }

    fn description(&self) -> &'static str {
        self.agent.exit_description()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "reason": {
                    "type": "string",
                    "description": "可选，说明为什么结束当前会话"
                }
            }
        })
    }

    async fn execute(&self, args: &Value, _context: &ToolContext) -> String {
        let reason = string_arg(args, "reason").unwrap_or_else(|| "任务已完成".to_string());
        format!(
            "已记录结束 {} 会话的请求：{}",
            self.agent.display_name(),
            reason
        )
    }
}

pub fn parse_dispatch_instruction(
    args: &Value,
    _agent: DispatchAgent,
) -> Result<(String, String), String> {
    let description = string_arg(args, "task_description")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "错误：task_description 为必填项，且不能为空".to_string())?;
    let permission_mode = string_arg(args, "permission_mode")
        .unwrap_or_else(|| "full_access".to_string())
        .trim()
        .to_string();

    if !matches!(
        permission_mode.as_str(),
        "ask" | "auto_edit" | "full_access"
    ) {
        return Err(format!(
            "错误：permission_mode 必须是 ask / auto_edit / full_access，实际收到：{}",
            permission_mode
        ));
    }

    Ok((description, permission_mode))
}

pub fn parse_continue_instruction(args: &Value, _agent: DispatchAgent) -> Result<String, String> {
    string_arg(args, "task_description")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "错误：task_description 为必填项，且不能为空".to_string())
}

pub fn parse_exit_instruction(args: &Value, _agent: DispatchAgent) -> String {
    string_arg(args, "reason").unwrap_or_else(|| "任务已完成".to_string())
}

fn string_arg(args: &Value, key: &str) -> Option<String> {
    if let Some(value) = args.get(key).and_then(Value::as_str) {
        return Some(value.to_string());
    }

    let parsed = match args {
        Value::String(raw) => serde_json::from_str::<Value>(raw).ok()?,
        _ => return None,
    };

    parsed.get(key)?.as_str().map(str::to_string)
}
