use std::path::Path;

use async_trait::async_trait;

use super::context::ToolContext;
use super::result::{ToolInput, ToolResult};
use super::spec::ToolSpec;

#[async_trait]
pub trait ToolProvider: Send + Sync {
    fn provider_name(&self) -> &'static str;
    fn specs_for_workspace(&self, workspace: &Path) -> Vec<ToolSpec>;
    async fn execute(
        &self,
        name: &str,
        input: ToolInput,
        context: &ToolContext,
    ) -> Option<ToolResult>;
}
