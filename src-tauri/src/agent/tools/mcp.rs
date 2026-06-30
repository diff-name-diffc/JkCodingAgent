use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use super::context::ToolContext;
use super::registry::DynamicToolProvider;
use super::spec::ToolSpec;
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
        let snapshot = self
            .project_mcp_registry
            .cached_for_workspace(&context.workspace);
        let snapshot = snapshot?;
        snapshot.tool_by_name(name)?;

        Some(
            self.project_mcp_registry
                .execute_tool(&context.workspace, name, args)
                .await
                .unwrap_or_else(|error| error),
        )
    }
}
