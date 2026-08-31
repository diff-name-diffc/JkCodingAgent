use std::collections::HashSet;

use crate::agent::llm::ToolDefinition;
use crate::mcp::ResolvedMcpTool;

use super::{is_mcp_tool_name, SUB_AGENT_TOOL_NAMES};

/// 判断工具是否被用户配置的允许列表放行（仅用于内置工具名；MCP 工具走
/// `retain_allowed_definitions` 的显式名单分支）。
///
/// 契约（G9-03，显式声明）：**空列表 = 全部放行**（fail-open 默认）。理由：
/// 1) 普通聊天的工具注册表本身是精选集（plain_chat_tools + 可选子智能体工具），
///    并非全量工具面；2) 设置页与分类配置以「空」表达「不限制」，若改为
///    fail-closed，默认/未配置用户将直接失去全部工具且 UI 无「全部」表达方式。
///
/// 可执行工具的安全由命令审查门禁（SSH/local_zsh fail-closed 审查）与工作区
/// 限制兜底，而非依赖此处默认拒绝。
///
/// MCP 工具名（`mcp__` 前缀）同样受允许列表约束，但语义相反——必须显式
/// 配置才放行（见 `retain_allowed_definitions`）；服务器级启停仍由
/// MCP 注册表层（设置中心全局页与项目 mcp.json）治理。
pub(super) fn is_tool_allowed_by_config(configured: &[String], tool_name: &str) -> bool {
    configured.is_empty() || configured.iter().any(|name| name == tool_name)
}

/// 按允许列表过滤发给模型的工具定义（混合契约）：
/// - 名字以 `mcp__` 开头的定义：**无论允许列表是否为空**，仅当名字显式
///   出现在 `configured` 中才保留（MCP 显式名单制）；
/// - 内置定义：`builtin_allowed` 为 `None` = 全部放行（配置为空的
///   fail-open 默认），`Some(list)` = 精确匹配（列表已含子智能体豁免）。
///
/// 系统提示词的 MCP 清单由 `allowed_mcp_tools_by_config` 用同一名单过滤，
/// 两处结构性地保持一致。
pub(super) fn retain_allowed_definitions(
    defs: Vec<ToolDefinition>,
    builtin_allowed: Option<&HashSet<String>>,
    configured: &[String],
) -> Vec<ToolDefinition> {
    let mcp_allowed: HashSet<&str> = configured
        .iter()
        .filter(|name| is_mcp_tool_name(name))
        .map(String::as_str)
        .collect();
    defs.into_iter()
        .filter(|def| {
            if is_mcp_tool_name(&def.function.name) {
                mcp_allowed.contains(def.function.name.as_str())
            } else {
                builtin_allowed.is_none_or(|allowed| allowed.contains(&def.function.name))
            }
        })
        .collect()
}

/// 按允许列表过滤快照中的 MCP 工具：允许列表为空 = 无任何 MCP 工具；
/// 非空 = 取 `canonical_name` 在列表中显式出现的交集。
pub(super) fn allowed_mcp_tools_by_config(
    tools: Vec<ResolvedMcpTool>,
    configured: &[String],
) -> Vec<ResolvedMcpTool> {
    if configured.is_empty() {
        return Vec::new();
    }
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

pub(super) fn effective_allowed_tools_for_chat_category(
    configured: Vec<String>,
    has_enabled_sub_agents: bool,
) -> HashSet<String> {
    let mut allowed = configured.into_iter().collect::<HashSet<_>>();
    if has_enabled_sub_agents {
        allowed.extend(SUB_AGENT_TOOL_NAMES.iter().map(|name| name.to_string()));
    }
    allowed
}

/// 由会话 ID 生成会话工作区子目录名（G9-04）。
///
/// 合法形态的 ID（字母数字 / `-` / `_`，≤64 字符，非 `.` 开头）原样使用；
/// 其余先过滤为安全字符，再追加确定性 FNV-1a 哈希后缀，保证：
/// 1) 不含路径分隔符与 `..`，无法越界（workspace_id 来自前端输入）；
/// 2) 不同会话 ID 不折叠到同一目录；
/// 3) 同一会话 ID 跨进程 / 重启始终得到同一目录（哈希确定性，不依赖随机态）。
pub(super) fn session_workspace_dir_name(workspace_id: &str) -> String {
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

    #[test]
    fn category_sub_agents_expose_sub_agent_tools_even_when_tool_allowlist_omits_them() {
        let allowed =
            effective_allowed_tools_for_chat_category(vec!["browser_read_text".to_string()], true);

        assert!(allowed.contains("browser_read_text"));
        assert!(allowed.contains("list_sub_agents"));
        assert!(allowed.contains("call_sub_agent"));
    }

    #[test]
    fn category_without_sub_agents_keeps_tool_allowlist_exact() {
        let allowed =
            effective_allowed_tools_for_chat_category(vec!["browser_read_text".to_string()], false);

        assert!(allowed.contains("browser_read_text"));
        assert!(!allowed.contains("list_sub_agents"));
        assert!(!allowed.contains("call_sub_agent"));
    }

    #[test]
    fn empty_allowlist_explicitly_allows_every_tool() {
        // G9-03 显式契约：空列表 = 全部放行（fail-open 默认）。
        // 注意：该契约只覆盖内置工具；MCP 工具的「空列表 = 无 MCP」
        // 由定义层 `retain_allowed_definitions` 单独处理。
        assert!(is_tool_allowed_by_config(&[], "local_zsh"));
        assert!(is_tool_allowed_by_config(&[], "call_sub_agent"));
        assert!(!is_tool_allowed_by_config(
            &["browser_read_text".to_string()],
            "local_zsh"
        ));
        assert!(is_tool_allowed_by_config(
            &["local_zsh".to_string()],
            "local_zsh"
        ));
    }

    fn tool_def(name: &str) -> ToolDefinition {
        ToolDefinition {
            kind: "function".to_string(),
            function: crate::agent::llm::ToolFunctionDefinition {
                name: name.to_string(),
                description: format!("{name} desc"),
                parameters: serde_json::json!({}),
            },
        }
    }

    fn mcp_tool(canonical_name: &str) -> ResolvedMcpTool {
        ResolvedMcpTool {
            canonical_name: canonical_name.to_string(),
            original_name: "tool".to_string(),
            server_name: "srv".to_string(),
            description: format!("{canonical_name} desc"),
            parameters: serde_json::json!({}),
            task_support: crate::mcp::McpToolTaskSupport::Optional,
        }
    }

    fn definition_names(defs: &[ToolDefinition]) -> Vec<&str> {
        defs.iter().map(|def| def.function.name.as_str()).collect()
    }

    #[test]
    fn empty_allowlist_keeps_builtin_definitions_and_drops_all_mcp() {
        let defs = vec![
            tool_def("local_zsh"),
            tool_def("browser_read_text"),
            tool_def("mcp__any_file_server__list_files"),
        ];

        let filtered = retain_allowed_definitions(defs, None, &[]);

        assert_eq!(
            definition_names(&filtered),
            vec!["local_zsh", "browser_read_text"]
        );
    }

    #[test]
    fn mcp_definitions_require_explicit_allowlist_entry() {
        let defs = vec![
            tool_def("local_zsh"),
            tool_def("browser_read_text"),
            tool_def("mcp__srv__a"),
            tool_def("mcp__srv__b"),
        ];
        let configured = vec!["browser_read_text".to_string(), "mcp__srv__a".to_string()];
        let builtin_allowed = effective_allowed_tools_for_chat_category(configured.clone(), false);

        let filtered = retain_allowed_definitions(defs, Some(&builtin_allowed), &configured);

        assert_eq!(
            definition_names(&filtered),
            vec!["browser_read_text", "mcp__srv__a"]
        );
    }

    #[test]
    fn sub_agent_tool_exemption_coexists_with_mcp_gating() {
        let defs = vec![
            tool_def("browser_read_text"),
            tool_def("list_sub_agents"),
            tool_def("call_sub_agent"),
            tool_def("mcp__srv__a"),
        ];
        let configured = vec!["browser_read_text".to_string()];
        let builtin_allowed = effective_allowed_tools_for_chat_category(configured.clone(), true);
        assert!(builtin_allowed.contains("call_sub_agent"));

        let filtered = retain_allowed_definitions(defs, Some(&builtin_allowed), &configured);

        // 子智能体豁免只作用于内置分支：未显式配置的 MCP 定义仍被剔除。
        assert_eq!(
            definition_names(&filtered),
            vec!["browser_read_text", "list_sub_agents", "call_sub_agent"]
        );
    }

    #[test]
    fn mcp_snapshot_tools_filtered_by_category_allowlist() {
        let tools = vec![mcp_tool("mcp__srv__a"), mcp_tool("mcp__srv__b")];
        let configured = vec!["local_zsh".to_string(), "mcp__srv__b".to_string()];

        let allowed = allowed_mcp_tools_by_config(tools, &configured);

        assert_eq!(allowed.len(), 1);
        assert_eq!(allowed[0].canonical_name, "mcp__srv__b");
    }

    #[test]
    fn empty_allowlist_yields_no_mcp_tools_even_with_healthy_snapshot() {
        let tools = vec![mcp_tool("mcp__srv__a"), mcp_tool("mcp__srv__b")];

        assert!(allowed_mcp_tools_by_config(tools, &[]).is_empty());
    }

    #[test]
    fn session_workspace_dir_name_keeps_safe_ids_unchanged() {
        assert_eq!(session_workspace_dir_name("abc-123_XYZ"), "abc-123_XYZ");
        assert_eq!(
            session_workspace_dir_name("5f9b2c8e-1a2d-4e3f-9a8b-7c6d5e4f3a2b"),
            "5f9b2c8e-1a2d-4e3f-9a8b-7c6d5e4f3a2b"
        );
    }

    #[test]
    fn session_workspace_dir_name_sanitizes_traversal_and_stays_deterministic() {
        let dotted = session_workspace_dir_name("../etc");
        assert!(!dotted.contains(".."));
        assert!(!dotted.contains('/'));

        let slashed = session_workspace_dir_name("a/b");
        assert!(!slashed.contains('/'));
        // 确定性：同一 ID 每次得到同一目录
        assert_eq!(slashed, session_workspace_dir_name("a/b"));
        // 不折叠：不同 ID 不会撞到同一目录
        assert_ne!(slashed, session_workspace_dir_name("a_b"));

        // 全非法字符退化为 session-哈希
        let blank = session_workspace_dir_name("  ");
        assert!(blank.starts_with("session-"));
        assert_eq!(blank, session_workspace_dir_name("  "));
    }
}
