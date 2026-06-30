use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent::llm::{ToolDefinition, ToolFunctionDefinition};

pub const DEFAULT_FORCE_COMPRESS_AFTER_CHARS: usize = 1_000;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ToolCategory {
    Filesystem,
    Search,
    Shell,
    Browser,
    Image,
    Ssh,
    Planning,
    Delegation,
    Mcp,
    SubAgent,
    Other,
}

impl ToolCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Filesystem => "filesystem",
            Self::Search => "search",
            Self::Shell => "shell",
            Self::Browser => "browser",
            Self::Image => "image",
            Self::Ssh => "ssh",
            Self::Planning => "planning",
            Self::Delegation => "delegation",
            Self::Mcp => "mcp",
            Self::SubAgent => "sub_agent",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ToolSafety {
    Safe,
    ReviewRequired,
    Dangerous,
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolAccess {
    pub readonly: bool,
    pub workspace_bound: bool,
    pub requires_network: bool,
    pub mutates_filesystem: bool,
    pub mutates_external_state: bool,
}

impl ToolAccess {
    pub fn readonly_workspace() -> Self {
        Self {
            readonly: true,
            workspace_bound: true,
            requires_network: false,
            mutates_filesystem: false,
            mutates_external_state: false,
        }
    }

    pub fn mutates_workspace() -> Self {
        Self {
            readonly: false,
            workspace_bound: true,
            requires_network: false,
            mutates_filesystem: true,
            mutates_external_state: false,
        }
    }

    pub fn external_effects() -> Self {
        Self {
            readonly: false,
            workspace_bound: false,
            requires_network: true,
            mutates_filesystem: false,
            mutates_external_state: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolExecutionPolicy {
    pub parallelizable: bool,
    pub timeout_secs: u64,
    pub cancellable: bool,
}

impl ToolExecutionPolicy {
    pub fn sequential(timeout_secs: u64) -> Self {
        Self {
            parallelizable: false,
            timeout_secs,
            cancellable: true,
        }
    }

    pub fn parallel_readonly(timeout_secs: u64) -> Self {
        Self {
            parallelizable: true,
            timeout_secs,
            cancellable: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolResultPolicy {
    pub default_compress: bool,
    pub force_compress_after_chars: usize,
    pub persist_raw_artifact: bool,
}

impl ToolResultPolicy {
    pub fn new(default_compress: bool) -> Self {
        Self {
            default_compress,
            force_compress_after_chars: DEFAULT_FORCE_COMPRESS_AFTER_CHARS,
            persist_raw_artifact: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    pub provider: String,
    pub category: ToolCategory,
    pub access: ToolAccess,
    pub safety: ToolSafety,
    pub execution: ToolExecutionPolicy,
    pub result_policy: ToolResultPolicy,
}

impl ToolSpec {
    pub fn new(name: &str, description: &str, parameters: Value) -> Self {
        let profile = ToolProfile::from_name(name);
        Self {
            name: name.to_string(),
            description: description.to_string(),
            parameters,
            provider: "builtin".to_string(),
            category: profile.category,
            access: profile.access,
            safety: profile.safety,
            execution: profile.execution,
            result_policy: profile.result_policy,
        }
    }

    pub fn mcp(name: String, description: String, parameters: Value) -> Self {
        Self {
            name,
            description,
            parameters,
            provider: "mcp".to_string(),
            category: ToolCategory::Mcp,
            access: ToolAccess {
                readonly: false,
                workspace_bound: false,
                requires_network: true,
                mutates_filesystem: false,
                mutates_external_state: true,
            },
            safety: ToolSafety::ReviewRequired,
            execution: ToolExecutionPolicy::sequential(60),
            result_policy: ToolResultPolicy::new(true),
        }
    }

    pub fn to_definition(&self) -> ToolDefinition {
        ToolDefinition {
            kind: "function".to_string(),
            function: ToolFunctionDefinition {
                name: self.name.clone(),
                description: self.description.clone(),
                parameters: self.parameters.clone(),
            },
        }
    }

    pub fn supports_parallel_readonly(&self) -> bool {
        self.access.readonly && self.execution.parallelizable
    }
}

struct ToolProfile {
    category: ToolCategory,
    access: ToolAccess,
    safety: ToolSafety,
    execution: ToolExecutionPolicy,
    result_policy: ToolResultPolicy,
}

impl ToolProfile {
    fn from_name(name: &str) -> Self {
        let category = category_for_name(name);
        let access = access_for_name(name, category);
        let execution = if access.readonly && is_known_parallel_readonly(name) {
            ToolExecutionPolicy::parallel_readonly(default_timeout_for_name(name))
        } else {
            ToolExecutionPolicy::sequential(default_timeout_for_name(name))
        };
        let safety = safety_for_name(name, &access);
        let result_policy = ToolResultPolicy::new(default_compress_for_name(name));
        Self {
            category,
            access,
            safety,
            execution,
            result_policy,
        }
    }
}

fn category_for_name(name: &str) -> ToolCategory {
    match name {
        "read_file" | "write_file" | "edit_file" | "list_dir" => ToolCategory::Filesystem,
        "glob" | "grep" => ToolCategory::Search,
        "exec" | "local_zsh" | "message" => ToolCategory::Shell,
        name if name.starts_with("browser_") => ToolCategory::Browser,
        "generate_image" | "edit_image" => ToolCategory::Image,
        name if name.starts_with("ssh_") => ToolCategory::Ssh,
        "update_plan"
        | "ask_plan_question"
        | "create_plan_document"
        | "read_plan_document"
        | "replace_plan_document"
        | "edit_plan_document"
        | "present_plan"
        | "mark_plan_implemented" => ToolCategory::Planning,
        "dispatch_claude"
        | "dispatch_codex"
        | "continue_claude_session"
        | "continue_codex_session"
        | "exit_claude_session"
        | "exit_codex_session" => ToolCategory::Delegation,
        "call_sub_agent" | "list_sub_agents" | "notify_user_progress" => ToolCategory::SubAgent,
        _ => ToolCategory::Other,
    }
}

fn access_for_name(name: &str, category: ToolCategory) -> ToolAccess {
    match name {
        "read_file" | "list_dir" | "glob" | "grep" | "read_plan_document" => {
            ToolAccess::readonly_workspace()
        }
        "write_file"
        | "edit_file"
        | "create_plan_document"
        | "replace_plan_document"
        | "edit_plan_document"
        | "mark_plan_implemented" => ToolAccess::mutates_workspace(),
        "browser_read_text" | "browser_visual_analyze" | "list_sub_agents" => ToolAccess {
            readonly: true,
            workspace_bound: false,
            requires_network: false,
            mutates_filesystem: false,
            mutates_external_state: false,
        },
        "message" | "update_plan" | "ask_plan_question" | "present_plan" => ToolAccess {
            readonly: false,
            workspace_bound: false,
            requires_network: false,
            mutates_filesystem: false,
            mutates_external_state: false,
        },
        _ if matches!(
            category,
            ToolCategory::Browser | ToolCategory::Ssh | ToolCategory::Image
        ) =>
        {
            ToolAccess::external_effects()
        }
        _ => ToolAccess {
            readonly: false,
            workspace_bound: false,
            requires_network: false,
            mutates_filesystem: false,
            mutates_external_state: false,
        },
    }
}

fn safety_for_name(name: &str, access: &ToolAccess) -> ToolSafety {
    if matches!(name, "exec" | "local_zsh") || name.starts_with("ssh_") {
        ToolSafety::ReviewRequired
    } else if access.mutates_external_state {
        ToolSafety::ReviewRequired
    } else {
        ToolSafety::Safe
    }
}

fn default_timeout_for_name(name: &str) -> u64 {
    match name {
        "read_file" | "write_file" | "edit_file" | "list_dir" => 30,
        "grep" | "glob" => 60,
        "exec" | "local_zsh" => 60,
        name if name.starts_with("browser_") => 30,
        name if name.starts_with("ssh_") => 60,
        _ => 60,
    }
}

fn default_compress_for_name(name: &str) -> bool {
    matches!(name, "exec" | "local_zsh") || name.starts_with("ssh_")
}

fn is_known_parallel_readonly(name: &str) -> bool {
    matches!(
        name,
        "read_file" | "list_dir" | "glob" | "grep" | "browser_read_text" | "browser_visual_analyze"
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{ToolCategory, ToolSpec};

    #[test]
    fn converts_spec_to_llm_tool_definition() {
        let spec = ToolSpec::new(
            "read_file",
            "读取文件",
            json!({ "type": "object", "properties": {} }),
        );

        let definition = spec.to_definition();

        assert_eq!(definition.kind, "function");
        assert_eq!(definition.function.name, "read_file");
        assert_eq!(definition.function.description, "读取文件");
        assert_eq!(definition.function.parameters["type"], "object");
    }

    #[test]
    fn read_file_is_declared_as_parallel_readonly() {
        let spec = ToolSpec::new(
            "read_file",
            "读取文件",
            json!({ "type": "object", "properties": {} }),
        );

        assert_eq!(spec.category, ToolCategory::Filesystem);
        assert!(spec.access.readonly);
        assert!(spec.execution.parallelizable);
        assert!(spec.supports_parallel_readonly());
    }

    #[test]
    fn mcp_tools_are_conservative_by_default() {
        let spec = ToolSpec::mcp(
            "mcp__server__tool".to_string(),
            "外部工具".to_string(),
            json!({ "type": "object", "properties": {} }),
        );

        assert_eq!(spec.category, ToolCategory::Mcp);
        assert!(!spec.access.readonly);
        assert!(!spec.execution.parallelizable);
        assert!(!spec.supports_parallel_readonly());
    }
}
