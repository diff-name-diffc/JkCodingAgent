use std::path::{Path, PathBuf};

use anyhow::Result;
use async_trait::async_trait;

use super::*;
use crate::agent::run_loop::core::{AgentRunAdapter, AgentRunRequest, RunPromptState};

#[async_trait]
impl AgentRunAdapter for ArchitectureAgent {
    async fn prepare_run_workspace(&self, request: &AgentRunRequest<'_>) -> Result<PathBuf> {
        // 架构 Agent 无 MCP：不需要 ensure_recent，直接准备会话沙箱。
        self.session_workspace(request.workspace_id).await
    }

    fn provider_snapshot(&self) -> OpenAiCompatProvider {
        self.provider.lock().clone()
    }

    fn provider_missing_message(&self) -> &'static str {
        "错误：未配置视觉模型。请在设置中心「模型服务」添加视觉模型后重试。"
    }

    async fn build_run_prompt(
        &self,
        _workspace_id: &str,
        _workspace: &Path,
    ) -> Result<RunPromptState> {
        Ok(RunPromptState {
            initial_system_prompt: self.build_effective_system_prompt(),
            project_prompt: None,
        })
    }
}
