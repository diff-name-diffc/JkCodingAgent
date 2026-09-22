//! MCP 动态工具桥（T2.4）：把 `mcp/` 注册表（Global/Project 作用域合并）
//! 枚举到的工具包装为 `PortableDynamicTool`（canonical 名 `mcp__<server>__<tool>`）。
//!
//! 移植自旧 `agent/tools/mcp.rs`（DynamicToolProvider 桥），语义映射：
//! - 枚举：旧实现走 `cached_for_scope` 只读缓存、由调用方预热（如
//!   `agents/plain_chat/adapter.rs` 的 `ensure_recent`）；本桥把预热并入枚举
//!   本身（`ensure_recent`），失败时对齐旧「缓存缺失 → 空工具面」语义，
//!   留痕后返回空列表。服务器启停/作用域合并完全由 `crate::mcp` 注册表承担，
//!   本桥不做任何过滤（分类 allowed_tools 过滤是 Phase 3 工具面组装的事）；
//! - 执行：回调内按名重新解析最新目录快照做 TOCTOU 复核（对齐旧 broker
//!   注入 spec hash 的复核语义，见 `execute_bridged` 注释），再通过
//!   `execute_tool_from_snapshot` 走注册表执行路径；
//! - 安全审查门禁（ssh_review 链路）属 Phase 3 runtime 策略层
//!   （见 `deps.rs` 头注），不在本桥重复；静态路径穿越防护为纯函数
//!   参数卫生检查，随本桥迁移。

use std::path::{Component, Path};

use rig::tool::{PortableDynamicTool, ToolExecutionError, ToolOutput};
use serde_json::Value;

use crate::mcp::{tool_definitions_from_snapshot, McpRegistry, McpScope, ResolvedMcpTool};

use super::deps::RigToolDeps;

/// 按 `deps.mcp_scope` 从 `deps.mcp_registry` 枚举已解析 MCP 工具，
/// 每个包装为一个 `PortableDynamicTool`。注册表枚举是异步的
/// （`ensure_recent` 可能触发作用域刷新），入口保持 async。
pub(crate) async fn mcp_tools(deps: &RigToolDeps) -> Vec<PortableDynamicTool> {
    let registry = deps.mcp_registry.clone();
    let scope = deps.mcp_scope.clone();

    let snapshot = match registry.ensure_recent(&scope).await {
        Ok(snapshot) => snapshot,
        Err(error) => {
            eprintln!("[agent] 警告：MCP 工具目录刷新失败（{scope}）：{error}");
            return Vec::new();
        }
    };

    tool_definitions_from_snapshot(Some(&snapshot))
        .into_iter()
        .map(|tool| bridge_tool(&registry, &scope, tool))
        .collect()
}

/// 单个已解析 MCP 工具 → PortableDynamicTool。description/parameters 用
/// 注册表快照原值（description 与旧桥一致带 `[MCP/<server>]` 前缀）；
/// 枚举期捕获的 definition 同时作为执行期 TOCTOU 复核的基准。
fn bridge_tool(registry: &McpRegistry, scope: &McpScope, tool: ResolvedMcpTool) -> PortableDynamicTool {
    let name = tool.canonical_name.clone();
    let description = mcp_description(&tool);
    let parameters = tool.parameters.clone();

    let callback_registry = registry.clone();
    let callback_scope = scope.clone();
    let callback_name = name.clone();
    let prepared_description = description.clone();
    let prepared_parameters = parameters.clone();

    PortableDynamicTool::new(name, description, parameters, move |args| {
        let registry = callback_registry.clone();
        let scope = callback_scope.clone();
        let name = callback_name.clone();
        let prepared_description = prepared_description.clone();
        let prepared_parameters = prepared_parameters.clone();
        Box::pin(async move {
            execute_bridged(
                &registry,
                &scope,
                &name,
                &prepared_description,
                &prepared_parameters,
                args,
            )
            .await
        })
    })
}

async fn execute_bridged(
    registry: &McpRegistry,
    scope: &McpScope,
    name: &str,
    prepared_description: &str,
    prepared_parameters: &Value,
    args: Value,
) -> Result<ToolOutput, ToolExecutionError> {
    // TOCTOU 复核（对齐旧 tools/mcp.rs 的 spec-hash 复核语义）：旧实现由
    // broker 在参数准备阶段注入 ToolSpec 指纹、执行前对 ensure_recent 快照
    // 重算比对；新实现无 broker，改为在执行回调内按名重新解析该工具的
    // 最新目录定义，与枚举期捕获的 definition 比对——解析不到或定义漂移
    // 一律 fail-closed 拒绝。存在性校验与执行同源（同一份 ensure_recent
    // 快照），避免用过期缓存做预检查导致 TOCTOU 误判。
    let snapshot = registry
        .ensure_recent(scope)
        .await
        .map_err(|error| recoverable(normalize_tool_error(error)))?;
    let Some(current) = snapshot.tool_by_name(name) else {
        return Err(ToolExecutionError::refused(format!(
            "错误：MCP 工具 `{name}` 在最新目录快照中已不存在，已拒绝本次调用；请刷新工具目录并重新规划。"
        )));
    };
    if definition_drifted(current, prepared_description, prepared_parameters) {
        return Err(ToolExecutionError::refused(format!(
            "错误：MCP 工具 `{name}` 的目录定义在参数准备后发生变化，已拒绝本次调用；请刷新工具目录并重新规划。"
        )));
    }

    // 参数边界防护：拒绝疑似路径穿越的参数（MCP 工具 schema 不透明且
    // workspace_bound=false，静态拦截 ".." 穿越，其余交给安全审查评估）。
    if let Some(offending) = traversal_risk_arg(&args) {
        return Err(ToolExecutionError::refused(format!(
            "错误：MCP 工具 `{name}` 参数疑似路径穿越，已拒绝执行：{offending}"
        )));
    }

    // 在 TOCTOU 复核通过的同一份快照上执行，避免刷新缓存后把同名但
    // Schema/server 已变化的工具偷换进当前 invocation。
    match registry
        .execute_tool_from_snapshot(&snapshot, name, &args)
        .await
    {
        Ok(output) => match serde_json::from_str::<Value>(&output) {
            Ok(data) => Ok(ToolOutput::json(data)),
            Err(error) => Err(recoverable(format!(
                "错误：解析 MCP 工具 `{name}` 的结构化结果失败：{error}"
            ))),
        },
        Err(error) => Err(recoverable(normalize_tool_error(error))),
    }
}

/// 工具目录条目 → 模型可见 description（与旧桥文案一致）。
fn mcp_description(tool: &ResolvedMcpTool) -> String {
    format!("[MCP/{}] {}", tool.server_name, tool.description)
}

/// TOCTOU 漂移判定：最新解析结果与枚举期捕获的 definition 比对。
/// description 内嵌 `[MCP/<server>]` 前缀，server 偷换经 description 覆盖
/// （与旧 ToolSpec::mcp 指纹的覆盖域一致：name/description/parameters）。
fn definition_drifted(
    current: &ResolvedMcpTool,
    prepared_description: &str,
    prepared_parameters: &Value,
) -> bool {
    mcp_description(current) != prepared_description || &current.parameters != prepared_parameters
}

/// 可恢复执行错误：旧 `ToolResult::recoverable_error` 的 rig 映射——
/// 错误文本回灌给模型、run 继续，retryable 提示置 true。
fn recoverable(message: String) -> ToolExecutionError {
    ToolExecutionError::other(message).with_retryable(true)
}

/// 递归扫描参数：返回第一个含 ".." 路径穿越的字符串值（截断展示）。
fn traversal_risk_arg(value: &Value) -> Option<String> {
    fn walk(value: &Value) -> Option<String> {
        match value {
            Value::String(text) => {
                if looks_like_path_traversal(text) {
                    let preview: String = text.trim().chars().take(120).collect();
                    Some(preview)
                } else {
                    None
                }
            }
            Value::Array(items) => items.iter().find_map(walk),
            Value::Object(map) => map.values().find_map(walk),
            _ => None,
        }
    }
    walk(value)
}

fn looks_like_path_traversal(text: &str) -> bool {
    let text = text.trim();
    // URL / 非路径字符串不做静态判定，交给安全审查评估。
    if text.is_empty() || text.contains("://") {
        return false;
    }
    Path::new(text)
        .components()
        .any(|component| matches!(component, Component::ParentDir))
}

/// 统一错误前缀：execute_tool 内部存在多条未带「错误：」前缀的错误路径，
/// 在桥接层归一化，保证模型/前端按统一规则识别失败态。
fn normalize_tool_error(error: String) -> String {
    if error.starts_with("错误：") {
        error
    } else {
        format!("错误：{error}")
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        definition_drifted, looks_like_path_traversal, normalize_tool_error, traversal_risk_arg,
    };
    use crate::mcp::{McpToolTaskSupport, ResolvedMcpTool};

    fn resolved_tool(description: &str, parameters: serde_json::Value) -> ResolvedMcpTool {
        ResolvedMcpTool {
            canonical_name: "mcp__server__tool".to_string(),
            original_name: "tool".to_string(),
            server_name: "server".to_string(),
            description: description.to_string(),
            parameters,
            task_support: McpToolTaskSupport::Forbidden,
        }
    }

    #[test]
    fn traversal_guard_flags_parent_dir_components() {
        assert!(looks_like_path_traversal("../etc/passwd"));
        assert!(looks_like_path_traversal("a/../../b"));
        assert!(looks_like_path_traversal("  ..  "));
        assert!(!looks_like_path_traversal("src/main.rs"));
        assert!(!looks_like_path_traversal(""));
        // URL 与含 .. 的非路径字符串不做静态判定。
        assert!(!looks_like_path_traversal("https://example.com/../x"));
        assert!(!looks_like_path_traversal("版本 v1..v2"));
    }

    #[test]
    fn traversal_guard_scans_nested_values() {
        assert_eq!(
            traversal_risk_arg(&json!({"path": "../secret"})).as_deref(),
            Some("../secret")
        );
        assert_eq!(
            traversal_risk_arg(&json!({"items": ["ok", "a/../b"]})).as_deref(),
            Some("a/../b")
        );
        assert_eq!(
            traversal_risk_arg(&json!({"nested": {"deep": ["../../root"]}})).as_deref(),
            Some("../../root")
        );
        assert_eq!(traversal_risk_arg(&json!({"path": "src/main.rs"})), None);
        assert_eq!(traversal_risk_arg(&json!({"count": 3})), None);
    }

    #[test]
    fn drift_detection_covers_description_schema_and_server_swap() {
        let prepared_parameters = json!({"type": "object", "properties": {}});
        let prepared_description = "[MCP/server] 工具";

        let unchanged = resolved_tool("工具", prepared_parameters.clone());
        assert!(!definition_drifted(
            &unchanged,
            prepared_description,
            &prepared_parameters
        ));

        let changed_schema = resolved_tool("工具", json!({"type": "object"}));
        assert!(definition_drifted(
            &changed_schema,
            prepared_description,
            &prepared_parameters
        ));

        let changed_description = resolved_tool("改名", prepared_parameters.clone());
        assert!(definition_drifted(
            &changed_description,
            prepared_description,
            &prepared_parameters
        ));

        let mut swapped_server = resolved_tool("工具", prepared_parameters.clone());
        swapped_server.server_name = "other".to_string();
        assert!(definition_drifted(
            &swapped_server,
            prepared_description,
            &prepared_parameters
        ));
    }

    #[test]
    fn normalize_tool_error_enforces_prefix() {
        assert_eq!(normalize_tool_error("错误：已带前缀".to_string()), "错误：已带前缀");
        assert_eq!(normalize_tool_error("未带前缀".to_string()), "错误：未带前缀");
    }
}
