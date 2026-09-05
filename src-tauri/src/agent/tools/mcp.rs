use std::path::{Component, Path};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use super::context::ToolContext;
use super::registry::DynamicToolProvider;
use super::result::ToolResult;
use super::spec::ToolSpec;
use crate::agent::llm::{ToolDefinition, ToolFunctionDefinition};
use crate::agent::ssh_review::{review_shell_command, CommandReviewTarget};
use crate::mcp::{tool_definitions_from_snapshot, McpRegistry, McpScope};

pub(super) fn mcp_tool_bridge(mcp_registry: McpRegistry) -> Arc<dyn DynamicToolProvider> {
    Arc::new(McpToolBridge { mcp_registry })
}

struct McpToolBridge {
    mcp_registry: McpRegistry,
}

#[async_trait]
impl DynamicToolProvider for McpToolBridge {
    fn specs_for_scope(&self, scope: &McpScope) -> Vec<ToolSpec> {
        let snapshot = self.mcp_registry.cached_for_scope(scope);
        tool_definitions_from_snapshot(snapshot.as_ref())
            .into_iter()
            .map(|tool| {
                ToolSpec::mcp(
                    tool.canonical_name,
                    format!("[MCP/{}] {}", tool.server_name, tool.description),
                    tool.parameters,
                )
            })
            .collect()
    }

    fn definitions_for_scope(&self, scope: &McpScope) -> Vec<ToolDefinition> {
        self.specs_for_scope(scope)
            .into_iter()
            .map(|spec| ToolDefinition {
                kind: "function".to_string(),
                function: ToolFunctionDefinition {
                    name: spec.name,
                    description: spec.description,
                    parameters: spec.parameters,
                },
            })
            .collect()
    }

    async fn execute(&self, name: &str, args: &Value, context: &ToolContext) -> Option<ToolResult> {
        // 存在性校验与 execute_tool 同源（ensure_recent），避免用过期缓存快照做
        // 预检查导致 TOCTOU 误判（缓存缺失/过期时误报「工具未找到」）。
        let snapshot = match self.mcp_registry.ensure_recent(&context.mcp_scope).await {
            Ok(snapshot) => snapshot,
            Err(error) => return Some(ToolResult::recoverable_error(error)),
        };
        let Some(tool) = snapshot.tool_by_name(name) else {
            // 不是 MCP 工具：交回注册表统一报「未找到工具」。
            return None;
        };
        let current_spec = ToolSpec::mcp(
            tool.canonical_name.clone(),
            format!("[MCP/{}] {}", tool.server_name, tool.description),
            tool.parameters.clone(),
        );
        let Some(prepared_hash) = context.current_tool_spec_hash.as_deref() else {
            return Some(ToolResult::fatal_error(format!(
                "错误：MCP 工具 `{name}` 缺少准备阶段的 ToolSpec 摘要，已按 fail-closed 拒绝执行。"
            )));
        };
        if prepared_hash != current_spec.fingerprint() {
            return Some(ToolResult::recoverable_error(format!(
                "错误：MCP 工具 `{name}` 的目录定义在参数准备后发生变化，已拒绝本次调用；请刷新工具目录并重新规划。"
            )));
        }

        // 参数边界防护：拒绝疑似路径穿越的参数（MCP 工具 schema 不透明且
        // workspace_bound=false，静态拦截 ".." 穿越，其余交给安全审查评估）。
        if let Some(offending) = traversal_risk_arg(args) {
            return Some(ToolResult::recoverable_error(format!(
                "错误：MCP 工具 `{name}` 参数疑似路径穿越，已拒绝执行：{offending}"
            )));
        }

        // 安全审查门禁（fail-closed）：MCP 工具统一标注 ReviewRequired，
        // 未配置审查 / 审查异常 / 判定不通过一律不得执行。
        if let Some(blocked) = review_mcp_call(name, args, context).await {
            return Some(ToolResult::recoverable_error(blocked));
        }

        Some(
            match self
                .mcp_registry
                .execute_tool_from_snapshot(&snapshot, name, args)
                .await
            {
                Ok(output) => match serde_json::from_str::<Value>(&output) {
                    Ok(data) => ToolResult::success_data(data, output.clone(), output),
                    Err(error) => ToolResult::recoverable_error(format!(
                        "错误：解析 MCP 工具 `{name}` 的结构化结果失败：{error}"
                    )),
                },
                Err(error) => ToolResult::recoverable_error(normalize_tool_error(error)),
            },
        )
    }
}

/// MCP 调用的安全审查：复用 ssh_review 链路，把工具名与完整参数 JSON 送审。
/// 返回 `Some(拦截消息)` 表示禁止执行（含未配置审查的 fail-closed 拦截）。
async fn review_mcp_call(name: &str, args: &Value, context: &ToolContext) -> Option<String> {
    let Some(review_config) = context.ssh_review.as_ref() else {
        return Some(format!(
            "错误：未配置安全审查，已拒绝执行 MCP 工具 `{name}`。请先在应用设置中配置安全审查模型。"
        ));
    };
    let args_json = serde_json::to_string(args).unwrap_or_else(|_| args.to_string());
    let payload = crate::agent::tools::review_context::build_review_payload(
        context,
        None,
        CommandReviewTarget::Mcp {
            workspace_path: context.workspace.display().to_string(),
            tool_name: name.to_string(),
        },
        format!("调用 MCP 工具 `{name}`，参数 JSON：{args_json}"),
        None,
    );
    match review_shell_command(review_config, &payload).await {
        Ok(verdict) if verdict.allowed => None,
        Ok(verdict) => Some(crate::agent::ssh_review::with_confirm_guidance(
            format!("错误：MCP 工具 `{name}` 调用被安全审查拦截：{}", verdict.reason),
            &verdict.reason,
        )),
        Err(error) => Some(format!(
            "错误：MCP 工具 `{name}` 安全审查异常，已拒绝执行：{error}"
        )),
    }
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
