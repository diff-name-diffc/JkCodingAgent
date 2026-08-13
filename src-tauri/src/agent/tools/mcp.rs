use std::path::{Component, Path};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use super::context::ToolContext;
use super::registry::DynamicToolProvider;
use super::spec::ToolSpec;
use crate::agent::llm::{ToolDefinition, ToolFunctionDefinition};
use crate::agent::ssh_review::{review_shell_command, CommandReviewPayload, CommandReviewTarget};
use crate::project::mcp::{tool_definitions_from_snapshot, ProjectMcpRegistry};

pub(super) fn mcp_tool_bridge(
    project_mcp_registry: ProjectMcpRegistry,
) -> Arc<dyn DynamicToolProvider> {
    Arc::new(McpToolBridge {
        project_mcp_registry,
    })
}

struct McpToolBridge {
    project_mcp_registry: ProjectMcpRegistry,
}

#[async_trait]
impl DynamicToolProvider for McpToolBridge {
    fn specs_for_workspace(&self, workspace: &Path) -> Vec<ToolSpec> {
        let snapshot = self.project_mcp_registry.cached_for_workspace(workspace);
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

    fn definitions_for_workspace(&self, workspace: &Path) -> Vec<ToolDefinition> {
        self.specs_for_workspace(workspace)
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

    async fn execute(&self, name: &str, args: &Value, context: &ToolContext) -> Option<String> {
        // 存在性校验与 execute_tool 同源（ensure_recent），避免用过期缓存快照做
        // 预检查导致 TOCTOU 误判（缓存缺失/过期时误报「工具未找到」）。
        let snapshot = match self
            .project_mcp_registry
            .ensure_recent(&context.workspace)
            .await
        {
            Ok(snapshot) => snapshot,
            Err(error) => return Some(normalize_tool_error(error)),
        };
        if snapshot.tool_by_name(name).is_none() {
            // 不是 MCP 工具：交回注册表统一报「未找到工具」。
            return None;
        }

        // 参数边界防护：拒绝疑似路径穿越的参数（MCP 工具 schema 不透明且
        // workspace_bound=false，静态拦截 ".." 穿越，其余交给安全审查评估）。
        if let Some(offending) = traversal_risk_arg(args) {
            return Some(format!(
                "错误：MCP 工具 `{name}` 参数疑似路径穿越，已拒绝执行：{offending}"
            ));
        }

        // 安全审查门禁（fail-closed）：MCP 工具统一标注 ReviewRequired，
        // 未配置审查 / 审查异常 / 判定不通过一律不得执行。
        if let Some(blocked) = review_mcp_call(name, args, context).await {
            return Some(blocked);
        }

        Some(
            self.project_mcp_registry
                .execute_tool(&context.workspace, name, args)
                .await
                .unwrap_or_else(normalize_tool_error),
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
    let payload = CommandReviewPayload {
        intent: context.session_title.clone(),
        task: context.user_task.clone().unwrap_or_default(),
        target: CommandReviewTarget::Mcp {
            workspace_path: context.workspace.display().to_string(),
            tool_name: name.to_string(),
        },
        command: format!("调用 MCP 工具 `{name}`，参数 JSON：{args_json}"),
        stdin: None,
    };
    match review_shell_command(review_config, &payload).await {
        Ok(verdict) if verdict.allowed => None,
        Ok(verdict) => Some(format!(
            "错误：MCP 工具 `{name}` 调用被安全审查拦截：{}",
            verdict.reason
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
