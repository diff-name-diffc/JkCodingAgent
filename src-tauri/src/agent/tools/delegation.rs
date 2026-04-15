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

    fn dispatch_prefix(self) -> &'static str {
        match self {
            Self::Claude => "__DISPATCH_CLAUDE__:",
            Self::Codex => "__DISPATCH_CODEX__:",
        }
    }

    fn continue_prefix(self) -> &'static str {
        match self {
            Self::Claude => "__CONTINUE_CLAUDE__:",
            Self::Codex => "__CONTINUE_CODEX__:",
        }
    }

    fn exit_prefix(self) -> &'static str {
        match self {
            Self::Claude => "__EXIT_CLAUDE__:",
            Self::Codex => "__EXIT_CODEX__:",
        }
    }

    fn dispatch_description(self) -> &'static str {
        match self {
            Self::Claude => {
                "Delegate a coding task to a Claude Code specialist agent running in a real terminal. Prefer Claude when you want faster iteration for new features, algorithm design, debugging exploration, or broad solution search. The task description should be detailed and self-contained."
            }
            Self::Codex => {
                "Delegate a coding task to a Codex specialist agent running in a real terminal. Prefer Codex when you want slower but more careful execution for refactoring, structural cleanup, regression-sensitive edits, or tasks that need extra verification discipline. The task description should be detailed and self-contained."
            }
        }
    }

    fn continue_description(self) -> &'static str {
        match self {
            Self::Claude => {
                "Continue an active Claude Code session by sending additional instructions to the running terminal. Use this for follow-up work in the same Claude subprocess."
            }
            Self::Codex => {
                "Continue an active Codex session by sending additional instructions to the running terminal. Use this for follow-up work in the same Codex subprocess."
            }
        }
    }

    fn exit_description(self) -> &'static str {
        match self {
            Self::Claude => {
                "Exit the active Claude Code session by sending /exit to the terminal. Use this when the Claude subprocess is complete."
            }
            Self::Codex => {
                "Exit the active Codex session by sending /exit to the terminal. Use this when the Codex subprocess is complete."
            }
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
                    "description": format!("A detailed, self-contained description of the coding task for {} to execute", self.agent.display_name())
                },
                "permission_mode": {
                    "type": "string",
                    "description": format!("Permission mode for {}: ask (default permissions), auto_edit (auto-accept edits), full_access (skip all permissions)", self.agent.display_name()),
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
            _ => return "Error: task_description is required and must not be empty".to_string(),
        };
        let permission_mode =
            string_arg(args, "permission_mode").unwrap_or_else(|| "full_access".to_string());

        format!(
            "{}{}|||{}",
            self.agent.dispatch_prefix(),
            task_description,
            permission_mode
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
                    "description": format!("The follow-up instruction to send to the active {} session", self.agent.display_name())
                }
            },
            "required": ["task_description"]
        })
    }

    async fn execute(&self, args: &Value, _context: &ToolContext) -> String {
        let task_description = match string_arg(args, "task_description") {
            Some(description) if !description.trim().is_empty() => description,
            _ => return "Error: task_description is required and must not be empty".to_string(),
        };
        format!("{}{}", self.agent.continue_prefix(), task_description)
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
                    "description": "Optional reason for exiting the session"
                }
            }
        })
    }

    async fn execute(&self, args: &Value, _context: &ToolContext) -> String {
        let reason = string_arg(args, "reason").unwrap_or_else(|| "Task completed".to_string());
        format!("{}{}", self.agent.exit_prefix(), reason)
    }
}

pub fn is_dispatch_instruction(result: &str, agent: DispatchAgent) -> bool {
    result.starts_with(agent.dispatch_prefix())
}

pub fn parse_dispatch_instruction(result: &str, agent: DispatchAgent) -> Option<(String, String)> {
    let after = result.strip_prefix(agent.dispatch_prefix())?;
    let parts: Vec<&str> = after.splitn(2, "|||").collect();
    if parts.len() == 2 {
        Some((parts[0].to_string(), parts[1].to_string()))
    } else {
        Some((after.to_string(), "full_access".to_string()))
    }
}

pub fn is_continue_instruction(result: &str, agent: DispatchAgent) -> bool {
    result.starts_with(agent.continue_prefix())
}

pub fn parse_continue_instruction(result: &str, agent: DispatchAgent) -> Option<String> {
    result
        .strip_prefix(agent.continue_prefix())
        .map(|value| value.to_string())
}

pub fn is_exit_instruction(result: &str, agent: DispatchAgent) -> bool {
    result.starts_with(agent.exit_prefix())
}

pub fn parse_exit_instruction(result: &str, agent: DispatchAgent) -> Option<String> {
    result
        .strip_prefix(agent.exit_prefix())
        .map(|value| value.to_string())
}

fn string_arg(args: &Value, key: &str) -> Option<String> {
    args.get(key)?.as_str().map(str::to_string)
}
