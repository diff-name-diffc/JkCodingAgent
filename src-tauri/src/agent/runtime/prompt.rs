use std::collections::HashSet;
use std::path::Path;

use anyhow::Result;

use super::super::prompt::{build_system_prompt, PromptBundle};
use crate::project::mcp::build_workspace_mcp_prompt_block;
use crate::shared::truncate_for_display;

use super::subprocess::{subprocess_phase_label, RegisteredSubprocessPhase};
use super::DispatcherAgent;

// ─── Internal types (module-local) ────────────────────────────────────────────

#[derive(Debug, Clone)]
pub(super) struct SystemPromptSnapshot {
    pub rendered: String,
}

// ─── Prompt building impl ─────────────────────────────────────────────────────

impl DispatcherAgent {
    pub(super) async fn build_system_prompt(&self) -> Result<PromptBundle> {
        let root = self.config.root_dir.clone();
        tokio::task::spawn_blocking(move || build_system_prompt(&root))
            .await
            .map_err(|e| anyhow::anyhow!("build_system_prompt panicked: {e}"))?
    }

    /// Build dynamic prompt sections without reading from disk.
    pub(super) fn build_dynamic_prompt_sections(
        &self,
        workspace_id: &str,
        workspace: &Path,
        tool_definitions: &[crate::agent::llm::ToolDefinition],
    ) -> Vec<String> {
        let mut sections = Vec::new();

        let tool_block = render_available_tools_block(tool_definitions);
        if !tool_block.is_empty() {
            sections.push(tool_block);
        }

        let state_block = self.build_subprocess_state_block(workspace_id);
        if !state_block.is_empty() {
            sections.push(state_block);
        }

        let mcp_block = build_workspace_mcp_prompt_block(
            self.project_mcp_registry
                .cached_for_workspace(workspace)
                .as_ref(),
            workspace,
        );
        if !mcp_block.is_empty() {
            sections.push(mcp_block);
        }

        let sub_agent_block = self.build_sub_agent_block(workspace_id);
        if !sub_agent_block.is_empty() {
            sections.push(sub_agent_block);
        }

        sections
    }

    /// Build a full system prompt by combining cached static content with
    /// freshly-computed dynamic sections.  Avoids re-reading files from disk.
    pub(super) fn build_system_prompt_from_static(
        &self,
        static_bundle: &PromptBundle,
        workspace_id: &str,
        workspace: &Path,
        tool_definitions: &[crate::agent::llm::ToolDefinition],
    ) -> Result<SystemPromptSnapshot> {
        let dynamic_sections =
            self.build_dynamic_prompt_sections(workspace_id, workspace, tool_definitions);

        let mut rendered = static_bundle.static_content.clone();
        for section in &dynamic_sections {
            rendered.push_str("\n\n---\n\n");
            rendered.push_str(section);
        }

        Ok(SystemPromptSnapshot { rendered })
    }

    pub(super) fn build_subprocess_state_block(&self, workspace_id: &str) -> String {
        let subprocesses = self.active_subprocesses_for_workspace(workspace_id);
        if subprocesses.is_empty() {
            return String::new();
        }

        let mut lines = vec![
            "# 当前子进程运行态".to_string(),
            "以下状态是系统权威状态，不要用聊天历史猜测：".to_string(),
        ];

        for subprocess in &subprocesses {
            lines.push(format!(
                "- agent={} dispatch_id={} task_id={} phase={} task={}",
                subprocess.agent,
                subprocess.dispatch_id,
                subprocess.task_id,
                subprocess_phase_label(subprocess.phase),
                truncate_for_display(&subprocess.description, 120, "...")
            ));
        }

        lines.push(
            "规则：如果某个 agent 已有 active subprocess，则禁止再次调用同 agent 的 dispatch_*。"
                .to_string(),
        );
        lines.push(
            "规则：phase=round_completed 时，只能在 continue_* / exit_* / 直接回复用户 之间选择。"
                .to_string(),
        );
        lines.push(
            "规则：phase=stopped 时，说明子进程已被 UI 手动停止但会话仍可恢复；此时不要继续 dispatch/continue/exit，而是先让用户决定是否恢复。"
                .to_string(),
        );
        lines.push(
            "规则：phase=exit_requested 时，不要再次调用该 agent 的 dispatch_* / continue_* / exit_*，只等待进程结束。"
                .to_string(),
        );

        lines.join("\n")
    }

    pub(super) fn build_sub_agent_block(&self, workspace_id: &str) -> String {
        if !self.is_tool_allowed_by_config("call_sub_agent")
            && !self.is_tool_allowed_by_config("list_sub_agents")
        {
            return String::new();
        }
        let Some(manager) = &self.sub_agent_manager else {
            return String::new();
        };
        let Ok(agents) = manager.get_enabled_for_session(workspace_id) else {
            return String::new();
        };
        if agents.is_empty() {
            return String::new();
        }

        let mut lines = vec![
            "# 当前可用子智能体".to_string(),
            "以下是当前会话已启用的子智能体，你可以直接调用 call_sub_agent(agent_id, task) 来处理特定领域的复杂任务：".to_string(),
        ];
        for agent in &agents {
            lines.push(format!(
                "- **{}** (`{}`): {}",
                agent.agent_name, agent.agent_id, agent.description
            ));
        }
        lines.join("\n")
    }

    pub(super) fn is_tool_allowed_by_config(&self, tool_name: &str) -> bool {
        let configured = self.allowed_tools.lock();
        configured.is_empty() || configured.iter().any(|name| name == tool_name)
    }
}

// ─── Tool definitions ─────────────────────────────────────────────────────────

impl DispatcherAgent {
    pub(super) fn tool_definitions_for_workspace(
        &self,
        workspace_id: &str,
        workspace: &Path,
    ) -> Vec<crate::agent::llm::ToolDefinition> {
        let mut allowed = default_tool_allowlist();

        let configured = self.allowed_tools.lock().clone();
        let configured_set = if configured.is_empty() {
            None
        } else {
            Some(configured.into_iter().collect::<HashSet<_>>())
        };
        if let Some(configured_set) = &configured_set {
            allowed.retain(|name| configured_set.contains(*name));
        }

        for (agent_slug, has_active, phase) in self.agent_runtime_flags(workspace_id) {
            match (has_active, phase) {
                (false, _) => {
                    allowed.insert(dispatch_tool_name(agent_slug));
                }
                (true, Some(RegisteredSubprocessPhase::Running))
                | (true, Some(RegisteredSubprocessPhase::RoundCompleted)) => {
                    allowed.insert(continue_tool_name(agent_slug));
                    allowed.insert(exit_tool_name(agent_slug));
                }
                (true, Some(RegisteredSubprocessPhase::Stopped)) => {}
                (true, Some(RegisteredSubprocessPhase::ExitRequested)) => {}
                (true, None) => {}
            }
        }

        let mut definitions =
            self.tools
                .definitions_for_workspace(workspace, Some(allowed.into_iter()), true);
        if let Some(configured_set) = configured_set {
            definitions.retain(|definition| configured_set.contains(&definition.function.name));
        }
        definitions
    }
}

// ─── Tool allowlists ──────────────────────────────────────────────────────────

pub(crate) fn default_tool_allowlist() -> HashSet<&'static str> {
    HashSet::from([
        "read_file",
        "write_file",
        "edit_file",
        "list_dir",
        "glob",
        "grep",
        "exec",
        "message",
        "call_sub_agent",
        "list_sub_agents",
    ])
}

// ─── Tool name helpers ────────────────────────────────────────────────────────

pub(crate) fn dispatch_tool_name(agent: &str) -> &'static str {
    match agent {
        "claude" => "dispatch_claude",
        "codex" => "dispatch_codex",
        _ => "dispatch_claude",
    }
}

pub(crate) fn continue_tool_name(agent: &str) -> &'static str {
    match agent {
        "claude" => "continue_claude_session",
        "codex" => "continue_codex_session",
        _ => "continue_claude_session",
    }
}

pub(crate) fn exit_tool_name(agent: &str) -> &'static str {
    match agent {
        "claude" => "exit_claude_session",
        "codex" => "exit_codex_session",
        _ => "exit_claude_session",
    }
}

pub(crate) fn render_available_tools_block(
    tool_definitions: &[crate::agent::llm::ToolDefinition],
) -> String {
    if tool_definitions.is_empty() {
        return String::new();
    }

    let mut lines = vec![
        "# 当前实际可用工具".to_string(),
        "以下列表来自本轮运行时实际注入的工具定义，是当前可调用工具的唯一准确信息源。".to_string(),
    ];

    let mut tools = tool_definitions
        .iter()
        .map(|tool| {
            (
                tool.function.name.clone(),
                tool.function.description.trim().to_string(),
            )
        })
        .collect::<Vec<_>>();
    tools.sort_by(|left, right| left.0.cmp(&right.0));

    for (name, description) in tools {
        lines.push(format!("- `{name}`：{description}"));
    }

    lines.join("\n")
}
