use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use super::context::ToolContext;
use super::registry::DynamicToolProvider;
use crate::agent::llm::{ToolDefinition, ToolFunctionDefinition};
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
    fn definitions_for_workspace(&self, workspace: &Path) -> Vec<ToolDefinition> {
        let snapshot = self.project_mcp_registry.cached_for_workspace(workspace);
        tool_definitions_from_snapshot(snapshot.as_ref())
            .into_iter()
            .map(|tool| ToolDefinition {
                kind: "function".to_string(),
                function: ToolFunctionDefinition {
                    name: tool.canonical_name,
                    description: format!("[MCP/{}] {}", tool.server_name, tool.description),
                    parameters: tool.parameters,
                },
            })
            .collect()
    }

    async fn execute(&self, name: &str, args: &Value, context: &ToolContext) -> Option<String> {
        let snapshot = self
            .project_mcp_registry
            .cached_for_workspace(&context.workspace);
        let Some(snapshot) = snapshot else {
            return None;
        };
        if snapshot.tool_by_name(name).is_none() {
            return None;
        }

        Some(
            self.project_mcp_registry
                .execute_tool(&context.workspace, name, args)
                .await
                .unwrap_or_else(|error| error),
        )
    }
}
