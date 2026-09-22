//! rig 形态的 Agent 装配层。
//!
//! 每个 Agent 负责「设置/上下文 → (模型, 工具面, 系统提示, 历史)」的装配，
//! 运行统一交给 `rig_ext::r#loop::run_rig_loop`（三段式策略：审查门禁 +
//! 台账 + 超时）。Phase 3 逐个迁移：plain_chat（本阶段）→ project → architecture。

pub(crate) mod plain_chat;

use std::collections::HashSet;

use rig::tool::PortableDynamicTool;

use crate::agent::rig_ext::tool_result::RigToolResultPolicy;
use crate::agent::tools::spec::ToolSpec;
use crate::mcp::ResolvedMcpTool;

/// MCP 工具名契约前缀（见 `mcp/registry.rs` 的 canonical 名）。
const MCP_TOOL_NAME_PREFIX: &str = "mcp__";

/// 子智能体工具名（允许列表豁免用）。
pub(crate) const SUB_AGENT_TOOL_NAMES: [&str; 2] = ["list_sub_agents", "call_sub_agent"];

fn is_mcp_tool_name(name: &str) -> bool {
    name.starts_with(MCP_TOOL_NAME_PREFIX)
}

/// 按允许列表过滤工具面（迁移自旧 `plain_chat/policy.rs::retain_allowed_definitions`）：
/// - `mcp__` 前缀工具：一律显式名单制（允许列表为空 = 无任何 MCP 工具）；
/// - 内置工具：允许列表为空 = 全部放行（fail-open 默认），非空时精确匹配
///   （启用子智能体时子智能体工具豁免，与旧 `effective_allowed_tools_for_chat_category` 一致）。
pub(crate) fn retain_allowed_tools(
    tools: Vec<PortableDynamicTool>,
    configured: &[String],
    has_enabled_sub_agents: bool,
) -> Vec<PortableDynamicTool> {
    let mcp_allowed: HashSet<&str> = configured
        .iter()
        .filter(|name| is_mcp_tool_name(name))
        .map(String::as_str)
        .collect();
    let mut builtin_allowed: HashSet<String> = configured.iter().cloned().collect();
    if has_enabled_sub_agents {
        builtin_allowed.extend(SUB_AGENT_TOOL_NAMES.iter().map(|name| name.to_string()));
    }

    tools
        .into_iter()
        .filter(|tool| {
            let name = tool.name();
            if is_mcp_tool_name(name) {
                mcp_allowed.contains(name)
            } else {
                configured.is_empty() || builtin_allowed.contains(name)
            }
        })
        .collect()
}

/// 按允许列表过滤快照中的 MCP 工具（允许列表为空 = 无任何 MCP 工具）。
pub(crate) fn allowed_mcp_tools_by_config(
    tools: Vec<ResolvedMcpTool>,
    configured: &[String],
) -> Vec<ResolvedMcpTool> {
    let allowed: HashSet<&str> = configured
        .iter()
        .filter(|name| is_mcp_tool_name(name))
        .map(String::as_str)
        .collect();
    tools
        .into_iter()
        .filter(|tool| allowed.contains(tool.canonical_name.as_str()))
        .collect()
}

/// 工具名 → 结果策略（压缩阈值等）的统一来源：策略表（`ToolSpec`）为唯一权威，
/// 避免工具面装配时手写阈值表与策略表漂移。MCP 工具走 `ToolSpec::mcp`
/// （default_compress=true + 5000），未收录工具名走 fail-closed 兜底。
pub(crate) fn tool_result_policies_from_specs(
) -> Vec<(String, RigToolResultPolicy)> {
    // 工具名列表由策略表派生：`ToolSpec::new` 对未收录名字回退 fail-closed，
    // 因此这里只登记策略表中真实存在的名字。
    crate::agent::tools::spec::registered_tool_names()
        .into_iter()
        .map(|name| {
            let spec = ToolSpec::new(name, "", serde_json::json!({}));
            (
                name.to_string(),
                RigToolResultPolicy {
                    default_compress: spec.result_policy.default_compress,
                    force_compress_after_chars: spec.result_policy.force_compress_after_chars,
                    persist_raw_artifact: spec.result_policy.persist_raw_artifact,
                },
            )
        })
        .collect()
}

/// 由会话 ID 生成会话工作区子目录名（G9-04，迁移自旧
/// `plain_chat/policy.rs::session_workspace_dir_name`）：
/// 合法形态的 ID 原样使用，其余过滤安全字符 + 确定性 FNV-1a 哈希后缀，
/// 保证不含路径分隔符与 `..`、不同 ID 不折叠、同 ID 跨进程稳定。
pub(crate) fn session_workspace_dir_name(workspace_id: &str) -> String {
    let trimmed = workspace_id.trim();
    let is_plain_safe = !trimmed.is_empty()
        && trimmed.len() <= 64
        && !trimmed.starts_with('.')
        && trimmed
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'));
    if is_plain_safe {
        return trimmed.to_string();
    }

    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in trimmed.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let sanitized: String = trimmed
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
        .take(32)
        .collect();
    if sanitized.is_empty() {
        format!("session-{hash:016x}")
    } else {
        format!("{sanitized}-{hash:016x}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rig::tool::PortableDynamicTool;

    fn tool(name: &str) -> PortableDynamicTool {
        PortableDynamicTool::new(
            name,
            "测试工具",
            serde_json::json!({"type": "object", "properties": {}}),
            |_args| Box::pin(async { Ok(rig::tool::ToolOutput::text("ok")) }),
        )
    }

    fn names(tools: &[PortableDynamicTool]) -> Vec<&str> {
        tools.iter().map(PortableDynamicTool::name).collect()
    }

    #[test]
    fn empty_allowlist_keeps_builtins_and_drops_all_mcp() {
        let tools = vec![
            tool("local_zsh"),
            tool("browser_read_text"),
            tool("mcp__srv__list"),
        ];
        let filtered = retain_allowed_tools(tools, &[], false);
        assert_eq!(names(&filtered), vec!["local_zsh", "browser_read_text"]);
    }

    #[test]
    fn mcp_tools_require_explicit_allowlist_entry() {
        let tools = vec![
            tool("local_zsh"),
            tool("browser_read_text"),
            tool("mcp__srv__a"),
            tool("mcp__srv__b"),
        ];
        let configured = vec!["browser_read_text".to_string(), "mcp__srv__a".to_string()];
        let filtered = retain_allowed_tools(tools, &configured, false);
        assert_eq!(names(&filtered), vec!["browser_read_text", "mcp__srv__a"]);
    }

    #[test]
    fn sub_agent_tools_are_exempt_only_for_builtin_branch() {
        let tools = vec![
            tool("browser_read_text"),
            tool("list_sub_agents"),
            tool("call_sub_agent"),
            tool("mcp__srv__a"),
        ];
        let configured = vec!["browser_read_text".to_string()];
        let filtered = retain_allowed_tools(tools, &configured, true);
        assert_eq!(
            names(&filtered),
            vec!["browser_read_text", "list_sub_agents", "call_sub_agent"]
        );
    }

    #[test]
    fn session_workspace_dir_name_is_safe_and_deterministic() {
        assert_eq!(session_workspace_dir_name("abc-123_XYZ"), "abc-123_XYZ");
        let dotted = session_workspace_dir_name("../etc");
        assert!(!dotted.contains("..") && !dotted.contains('/'));
        assert_eq!(dotted, session_workspace_dir_name("../etc"));
        assert_ne!(dotted, session_workspace_dir_name("a_b"));
        let blank = session_workspace_dir_name("  ");
        assert!(blank.starts_with("session-"));
    }

    #[test]
    fn result_policies_come_from_the_spec_table() {
        let policies = tool_result_policies_from_specs();
        let value = |name: &str| {
            policies
                .iter()
                .find(|(tool, _)| tool == name)
                .map(|(_, policy)| *policy)
        };
        let local_zsh = value("local_zsh").expect("local_zsh 在策略表中");
        assert!(local_zsh.default_compress);
        assert_eq!(
            local_zsh.force_compress_after_chars,
            crate::agent::tools::spec::COMMAND_FORCE_COMPRESS_AFTER_CHARS
        );
        let read_file = value("read_file").expect("read_file 在策略表中");
        assert!(!read_file.default_compress);
    }
}
