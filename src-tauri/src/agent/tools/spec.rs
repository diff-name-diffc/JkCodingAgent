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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolAccess {
    pub readonly: bool,
    pub workspace_bound: bool,
    pub requires_network: bool,
    pub mutates_filesystem: bool,
    pub mutates_external_state: bool,
}

impl ToolAccess {
    /// 只读且限定工作区内（read_file / list_dir / glob / grep）。
    pub const READONLY_WORKSPACE: Self = Self {
        readonly: true,
        workspace_bound: true,
        requires_network: false,
        mutates_filesystem: false,
        mutates_external_state: false,
    };

    /// 可写但限定工作区内（write_file / edit_file）。
    pub const MUTATES_WORKSPACE: Self = Self {
        readonly: false,
        workspace_bound: true,
        requires_network: false,
        mutates_filesystem: true,
        mutates_external_state: false,
    };

    /// 外部效应：访问网络、可变更外部状态（browser 交互 / image / ssh 等）。
    pub const EXTERNAL_EFFECTS: Self = Self {
        readonly: false,
        workspace_bound: false,
        requires_network: true,
        mutates_filesystem: false,
        mutates_external_state: true,
    };

    /// 完整外部效应：可改文件系统、可访问网络、可变更外部状态
    /// （exec / local_zsh 等命令执行类工具，能力边界按最坏情况声明，fail-closed）。
    pub const FULL_EFFECTS: Self = Self {
        readonly: false,
        workspace_bound: false,
        requires_network: true,
        mutates_filesystem: true,
        mutates_external_state: true,
    };

    /// 只读但不限工作区（list_sub_agents 等枚举类查询）。
    pub const READONLY_UNBOUND: Self = Self {
        readonly: true,
        workspace_bound: false,
        requires_network: false,
        mutates_filesystem: false,
        mutates_external_state: false,
    };

    /// 非只读但不直接落盘/变更外部状态：效果由对应子系统托管
    /// （message 最终消息、submit_graph / graph_plan_report 协议壳、
    /// notify_user_progress / call_sub_agent 子智能体调度）。
    pub const SUBSYSTEM_MANAGED: Self = Self {
        readonly: false,
        workspace_bound: false,
        requires_network: false,
        mutates_filesystem: false,
        mutates_external_state: false,
    };
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolExecutionPolicy {
    pub parallelizable: bool,
    /// 统一超时（秒）。语义约定：**0 表示不设统一超时限制**——
    /// ToolRuntime 会跳过统一超时包裹（与 unified_timeout=false 等效），
    /// 由工具自管生命周期；策略表中的已知工具必须 > 0。
    pub timeout_secs: u64,
    pub cancellable: bool,
    #[serde(default = "default_unified_timeout")]
    pub unified_timeout: bool,
}

impl ToolExecutionPolicy {
    pub fn sequential(timeout_secs: u64) -> Self {
        Self {
            parallelizable: false,
            timeout_secs,
            cancellable: true,
            unified_timeout: true,
        }
    }

    pub fn parallel_readonly(timeout_secs: u64) -> Self {
        Self {
            parallelizable: true,
            timeout_secs,
            cancellable: true,
            unified_timeout: true,
        }
    }

    pub fn tool_managed_timeout(timeout_secs: u64) -> Self {
        Self {
            parallelizable: false,
            timeout_secs,
            cancellable: true,
            unified_timeout: false,
        }
    }
}

fn default_unified_timeout() -> bool {
    true
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

/// 未注册/未知工具名的兜底统一超时（秒）。
const DEFAULT_UNKNOWN_TIMEOUT_SECS: u64 = 60;

/// 工具策略表条目：一行声明式定义一个工具的全部策略字段
/// （category / access / safety / timeout / compress / parallel / self-managed）。
/// 新增工具只需在此表补一行；未收录的工具名一律走 fail-closed 兜底
/// （见 `ToolProfile::fail_closed`），不再静默回退到宽松默认。
struct ToolPolicyRow {
    name: &'static str,
    category: ToolCategory,
    access: ToolAccess,
    safety: ToolSafety,
    timeout_secs: u64,
    default_compress: bool,
    parallel_readonly: bool,
    self_managed_timeout: bool,
}

const fn policy_row(
    name: &'static str,
    category: ToolCategory,
    access: ToolAccess,
    safety: ToolSafety,
    timeout_secs: u64,
    default_compress: bool,
    parallel_readonly: bool,
    self_managed_timeout: bool,
) -> ToolPolicyRow {
    ToolPolicyRow {
        name,
        category,
        access,
        safety,
        timeout_secs,
        default_compress,
        parallel_readonly,
        self_managed_timeout,
    }
}

/// 已知工具策略表（唯一事实来源）。
///
/// 约定：
/// - 浏览器共用同一会话，browser_* 一律不参与并行调度（含只读读取类）；
/// - exec / local_zsh / ssh_* 必须 ReviewRequired，access 按最坏能力声明；
/// - 表中未收录的工具名视为未知工具，走 fail-closed 兜底。
static TOOL_POLICY_TABLE: &[ToolPolicyRow] = &[
    // ── 文件系统 ──
    policy_row("read_file", ToolCategory::Filesystem, ToolAccess::READONLY_WORKSPACE, ToolSafety::Safe, 30, false, true, false),
    policy_row("write_file", ToolCategory::Filesystem, ToolAccess::MUTATES_WORKSPACE, ToolSafety::Safe, 30, false, false, false),
    policy_row("edit_file", ToolCategory::Filesystem, ToolAccess::MUTATES_WORKSPACE, ToolSafety::Safe, 30, false, false, false),
    policy_row("list_dir", ToolCategory::Filesystem, ToolAccess::READONLY_WORKSPACE, ToolSafety::Safe, 30, false, true, false),
    // ── 搜索 ──
    policy_row("glob", ToolCategory::Search, ToolAccess::READONLY_WORKSPACE, ToolSafety::Safe, 60, false, true, false),
    policy_row("grep", ToolCategory::Search, ToolAccess::READONLY_WORKSPACE, ToolSafety::Safe, 60, false, true, false),
    // ── 命令执行（能力边界按最坏情况声明，强制审查）──
    policy_row("exec", ToolCategory::Shell, ToolAccess::FULL_EFFECTS, ToolSafety::ReviewRequired, 60, true, false, true),
    policy_row("local_zsh", ToolCategory::Shell, ToolAccess::FULL_EFFECTS, ToolSafety::ReviewRequired, 60, true, false, true),
    // message 是面向用户的最终消息通知工具（映射为 ToolAction::FinalMessage），
    // 并非 shell 命令，不归入 Shell 类。
    policy_row("message", ToolCategory::Other, ToolAccess::SUBSYSTEM_MANAGED, ToolSafety::Safe, 60, false, false, false),
    // ── 浏览器（共享会话：统一 requires_network、禁止并行）──
    policy_row("browser_open_url", ToolCategory::Browser, ToolAccess::EXTERNAL_EFFECTS, ToolSafety::ReviewRequired, 30, false, false, false),
    policy_row("browser_click", ToolCategory::Browser, ToolAccess::EXTERNAL_EFFECTS, ToolSafety::ReviewRequired, 30, false, false, false),
    policy_row("browser_type", ToolCategory::Browser, ToolAccess::EXTERNAL_EFFECTS, ToolSafety::ReviewRequired, 30, false, false, false),
    policy_row("browser_press", ToolCategory::Browser, ToolAccess::EXTERNAL_EFFECTS, ToolSafety::ReviewRequired, 30, false, false, false),
    policy_row("browser_wait_for", ToolCategory::Browser, ToolAccess::EXTERNAL_EFFECTS, ToolSafety::ReviewRequired, 30, false, false, false),
    policy_row("browser_close", ToolCategory::Browser, ToolAccess::EXTERNAL_EFFECTS, ToolSafety::ReviewRequired, 30, false, false, false),
    // 只读读取类同样驱动浏览器会话：requires_network=true、不参与并行。
    policy_row("browser_read_text", ToolCategory::Browser, ToolAccess { readonly: true, workspace_bound: false, requires_network: true, mutates_filesystem: false, mutates_external_state: false }, ToolSafety::Safe, 30, false, false, false),
    policy_row("browser_visual_analyze", ToolCategory::Browser, ToolAccess { readonly: true, workspace_bound: false, requires_network: true, mutates_filesystem: false, mutates_external_state: false }, ToolSafety::Safe, 30, false, false, false),
    // ── 图像生成 ──
    policy_row("generate_image", ToolCategory::Image, ToolAccess::EXTERNAL_EFFECTS, ToolSafety::ReviewRequired, 60, false, false, false),
    policy_row("edit_image", ToolCategory::Image, ToolAccess::EXTERNAL_EFFECTS, ToolSafety::ReviewRequired, 60, false, false, false),
    // ── SSH（强制审查，默认声明压缩参数）──
    policy_row("ssh_list_servers", ToolCategory::Ssh, ToolAccess::EXTERNAL_EFFECTS, ToolSafety::ReviewRequired, 60, true, false, false),
    policy_row("ssh_exec", ToolCategory::Ssh, ToolAccess::EXTERNAL_EFFECTS, ToolSafety::ReviewRequired, 60, true, false, false),
    // ── 子智能体 ──
    policy_row("call_sub_agent", ToolCategory::SubAgent, ToolAccess::SUBSYSTEM_MANAGED, ToolSafety::Safe, 600, false, false, true),
    policy_row("list_sub_agents", ToolCategory::SubAgent, ToolAccess::READONLY_UNBOUND, ToolSafety::Safe, 60, false, false, false),
    policy_row("notify_user_progress", ToolCategory::SubAgent, ToolAccess::SUBSYSTEM_MANAGED, ToolSafety::Safe, 60, false, false, false),
    // ── 图编排协议壳 ──
    policy_row("submit_graph", ToolCategory::Other, ToolAccess::SUBSYSTEM_MANAGED, ToolSafety::Safe, 60, false, false, false),
    policy_row("graph_plan_report", ToolCategory::Other, ToolAccess::SUBSYSTEM_MANAGED, ToolSafety::Safe, 60, false, false, false),
];

fn lookup_policy(name: &str) -> Option<&'static ToolPolicyRow> {
    TOOL_POLICY_TABLE.iter().find(|row| row.name == name)
}

impl ToolProfile {
    fn from_name(name: &str) -> Self {
        match lookup_policy(name) {
            Some(row) => Self::from_row(row),
            // fail-closed：未知/未注册工具名一律保守处理，杜绝 fail-open。
            None => Self::fail_closed(),
        }
    }

    fn from_row(row: &'static ToolPolicyRow) -> Self {
        let execution = if row.self_managed_timeout {
            ToolExecutionPolicy::tool_managed_timeout(row.timeout_secs)
        } else if row.parallel_readonly {
            ToolExecutionPolicy::parallel_readonly(row.timeout_secs)
        } else {
            ToolExecutionPolicy::sequential(row.timeout_secs)
        };
        Self {
            category: row.category,
            access: row.access,
            safety: row.safety,
            execution,
            result_policy: ToolResultPolicy::new(row.default_compress),
        }
    }

    /// fail-closed 兜底策略：未知（未注册/幻觉）工具名按「只读 + 工作区内 +
    /// 需审查 + 串行执行」的最保守组合处理，确保任何未知工具都不会被标记为
    /// Safe 而绕过审查门禁，也不会被误判为可并行。
    fn fail_closed() -> Self {
        Self {
            category: ToolCategory::Other,
            access: ToolAccess::READONLY_WORKSPACE,
            safety: ToolSafety::ReviewRequired,
            execution: ToolExecutionPolicy::sequential(DEFAULT_UNKNOWN_TIMEOUT_SECS),
            result_policy: ToolResultPolicy::new(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use serde_json::json;

    use super::{ToolCategory, ToolSafety, ToolSpec, TOOL_POLICY_TABLE};

    fn spec_for(name: &str) -> ToolSpec {
        ToolSpec::new(name, "测试工具", json!({ "type": "object", "properties": {} }))
    }

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

    #[test]
    fn call_sub_agent_uses_its_own_timeout_policy() {
        let spec = ToolSpec::new(
            "call_sub_agent",
            "调用子智能体",
            json!({ "type": "object", "properties": {} }),
        );

        assert_eq!(spec.category, ToolCategory::SubAgent);
        assert_eq!(spec.execution.timeout_secs, 600);
        assert!(!spec.execution.unified_timeout);
        assert!(!spec.execution.parallelizable);
    }

    #[test]
    fn shell_tools_use_runtime_context_timeout_policy() {
        for name in ["exec", "local_zsh"] {
            let spec = ToolSpec::new(
                name,
                "执行命令",
                json!({ "type": "object", "properties": {} }),
            );

            assert_eq!(spec.category, ToolCategory::Shell);
            assert!(!spec.execution.unified_timeout);
            assert!(!spec.execution.parallelizable);
        }
    }

    #[test]
    fn unknown_tool_falls_back_to_fail_closed_policy() {
        let spec = spec_for("totally_unknown_tool");

        assert_eq!(spec.category, ToolCategory::Other);
        // fail-closed：按只读 + 工作区内 + 需审查 + 串行处理，
        // 绝不落入「Safe + 无边界」的宽松兜底。
        assert!(spec.access.readonly);
        assert!(spec.access.workspace_bound);
        assert!(!spec.access.requires_network);
        assert!(!spec.access.mutates_filesystem);
        assert!(!spec.access.mutates_external_state);
        assert_eq!(spec.safety, ToolSafety::ReviewRequired);
        assert!(!spec.execution.parallelizable);
        assert!(spec.execution.unified_timeout);
        assert!(!spec.supports_parallel_readonly());
        assert!(!spec.result_policy.default_compress);
    }

    #[test]
    fn exec_and_local_zsh_declare_full_effects_and_review_required() {
        for name in ["exec", "local_zsh"] {
            let spec = spec_for(name);

            assert_eq!(spec.category, ToolCategory::Shell);
            assert_eq!(spec.safety, ToolSafety::ReviewRequired);
            // 能力边界按最坏情况显式声明：可改文件、可联网、可变更外部状态。
            assert!(!spec.access.readonly);
            assert!(!spec.access.workspace_bound);
            assert!(spec.access.requires_network);
            assert!(spec.access.mutates_filesystem);
            assert!(spec.access.mutates_external_state);
            assert!(spec.result_policy.default_compress);
            assert!(!spec.execution.unified_timeout);
        }
    }

    #[test]
    fn browser_tools_share_network_metadata_and_are_sequential() {
        // 只读读取类：requires_network 与工具族其余成员一致；
        // 共享浏览器会话，一律排除出并行调度。
        for name in ["browser_read_text", "browser_visual_analyze"] {
            let spec = spec_for(name);

            assert_eq!(spec.category, ToolCategory::Browser);
            assert!(spec.access.readonly);
            assert!(spec.access.requires_network);
            assert!(!spec.execution.parallelizable);
            assert!(!spec.supports_parallel_readonly());
        }
        // 交互类：外部效应 + 需审查。
        for name in [
            "browser_open_url",
            "browser_click",
            "browser_type",
            "browser_press",
            "browser_wait_for",
            "browser_close",
        ] {
            let spec = spec_for(name);

            assert_eq!(spec.category, ToolCategory::Browser);
            assert!(spec.access.requires_network);
            assert!(spec.access.mutates_external_state);
            assert_eq!(spec.safety, ToolSafety::ReviewRequired);
            assert!(!spec.execution.parallelizable);
        }
    }

    #[test]
    fn message_tool_is_a_notification_tool_not_shell() {
        let spec = spec_for("message");

        assert_eq!(spec.category, ToolCategory::Other);
        assert!(!spec.access.requires_network);
        assert!(!spec.access.mutates_filesystem);
        assert!(!spec.access.mutates_external_state);
        assert_eq!(spec.safety, ToolSafety::Safe);
    }

    #[test]
    fn ssh_tools_are_review_required_and_compressed_by_default() {
        for name in ["ssh_exec", "ssh_list_servers"] {
            let spec = spec_for(name);

            assert_eq!(spec.category, ToolCategory::Ssh);
            assert_eq!(spec.safety, ToolSafety::ReviewRequired);
            assert!(spec.access.requires_network);
            assert!(spec.result_policy.default_compress);
        }
    }

    #[test]
    fn filesystem_write_tools_stay_workspace_bound() {
        for name in ["write_file", "edit_file"] {
            let spec = spec_for(name);

            assert_eq!(spec.category, ToolCategory::Filesystem);
            assert!(!spec.access.readonly);
            assert!(spec.access.workspace_bound);
            assert!(spec.access.mutates_filesystem);
            assert!(!spec.access.requires_network);
            assert_eq!(spec.safety, ToolSafety::Safe);
            assert!(!spec.execution.parallelizable);
        }
    }

    #[test]
    fn policy_table_rows_are_unique_and_consistent() {
        let mut seen = HashSet::new();
        for row in TOOL_POLICY_TABLE {
            assert!(
                seen.insert(row.name),
                "策略表存在重复工具名：{}",
                row.name
            );
            // 并行调度仅允许真正的只读工具。
            if row.parallel_readonly {
                assert!(row.access.readonly, "{} 声明并行但非只读", row.name);
                assert!(
                    !row.access.mutates_filesystem,
                    "{} 声明并行但会写文件",
                    row.name
                );
            }
            // 统一超时为 0 表示「不设限制」，已知工具必须给出明确正值。
            assert!(row.timeout_secs > 0, "{} 超时配置为 0", row.name);
            // 安全等级与能力声明的一致性：会变更外部状态/文件系统的工具不允许 Safe 之外
            // 的宽松组合（此处仅断言写/外部效应工具非 Safe 即需审查）。
            if row.access.mutates_external_state || row.access.mutates_filesystem {
                assert!(
                    row.safety != ToolSafety::Safe || row.access.workspace_bound,
                    "{} 有外部效应但既非 Safe 也非工作区内",
                    row.name
                );
            }
        }
    }
}
