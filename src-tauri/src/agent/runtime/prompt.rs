use std::collections::HashSet;
use std::path::Path;

use anyhow::Result;

use super::super::db::{DispatcherMode, DispatcherSessionRuntimeState};
use super::super::prompt::{build_system_prompt, PromptBundle};
use crate::project::mcp::build_workspace_mcp_prompt_block;
use crate::shared::truncate_for_display;

use super::helpers::is_implemented_plan_path;
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
        runtime_state: &DispatcherSessionRuntimeState,
    ) -> Vec<String> {
        let mut sections = Vec::new();

        sections.push(build_dispatcher_mode_block(runtime_state));

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
        runtime_state: &DispatcherSessionRuntimeState,
    ) -> Result<SystemPromptSnapshot> {
        let dynamic_sections = self.build_dynamic_prompt_sections(
            workspace_id,
            workspace,
            tool_definitions,
            runtime_state,
        );

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
}

// ─── Tool definitions ─────────────────────────────────────────────────────────

impl DispatcherAgent {
    pub(super) fn tool_definitions_for_workspace(
        &self,
        workspace_id: &str,
        workspace: &Path,
        runtime_state: &DispatcherSessionRuntimeState,
    ) -> Vec<crate::agent::llm::ToolDefinition> {
        let mut allowed = match runtime_state.mode {
            DispatcherMode::Default => default_mode_tool_allowlist(),
            DispatcherMode::Plan => plan_mode_tool_allowlist(),
        };

        let configured = self.allowed_tools.lock().clone();
        if !configured.is_empty() {
            let configured_set: HashSet<String> = configured.into_iter().collect();
            allowed.retain(|name| configured_set.contains(*name));
            allowed.insert("call_sub_agent");
            allowed.insert("list_sub_agents");
        }

        let include_dynamic = runtime_state.mode == DispatcherMode::Default;

        if runtime_state.mode == DispatcherMode::Default {
            if runtime_state
                .active_plan_path
                .as_deref()
                .is_some_and(|path| !is_implemented_plan_path(Path::new(path)))
            {
                allowed.insert("mark_plan_implemented");
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
        }

        self.tools
            .definitions_for_workspace(workspace, Some(allowed.into_iter()), include_dynamic)
    }
}

// ─── Tool allowlists ──────────────────────────────────────────────────────────

pub(crate) fn default_mode_tool_allowlist() -> HashSet<&'static str> {
    HashSet::from([
        "read_file",
        "write_file",
        "edit_file",
        "list_dir",
        "glob",
        "grep",
        "search_knowledge_base",
        "read_knowledge_page",
        "exec",
        "message",
        "update_plan",
        "call_sub_agent",
        "list_sub_agents",
    ])
}

pub(crate) fn plan_mode_tool_allowlist() -> HashSet<&'static str> {
    HashSet::from([
        "read_file",
        "list_dir",
        "glob",
        "grep",
        "exec",
        "message",
        "ask_plan_question",
        "create_plan_document",
        "read_plan_document",
        "replace_plan_document",
        "edit_plan_document",
        "present_plan",
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

// ─── Prompt rendering helpers ─────────────────────────────────────────────────

pub(crate) fn build_dispatcher_mode_block(runtime_state: &DispatcherSessionRuntimeState) -> String {
    let mut lines = Vec::new();
    match runtime_state.mode {
        DispatcherMode::Default => {
            lines.push("# 当前模式：Default".to_string());
            lines.push(
                "- 可以使用 `update_plan` 维护输入框上方的 Checklist；复杂任务应主动维护，简单任务可跳过。"
                    .to_string(),
            );
            lines.push(
                "- 如果本轮任务复杂到需要 Checklist，必须先调用 `update_plan` 创建本次任务规划步骤，再进行 glob/grep/read_file/exec 探索、委派或编码实践；不要把探索结果拿到以后才补建 Checklist。"
                    .to_string(),
            );
            lines.push(
                "- 例外：如果用户是在实施已经确认的 Plan 计划书，尤其消息中包含计划书路径，则不要调用 `update_plan` 重新规划；直接围绕计划书内容委派 Claude/Codex 子进程执行。"
                    .to_string(),
            );
            lines.push(
                "- Checklist 是子任务执行状态机：先列出待执行步骤；调用 `dispatch_claude`/`dispatch_codex` 时系统会把当前/下一个步骤绑定到该子 Agent，子进程启动后显示运行中，回流终态后显示完成。".to_string(),
            );
            lines.push(
                "- 可以使用 Claude/Codex 委派工具执行编码任务；实施计划时优先委派执行代理，Dispatcher 负责协调和验收。"
                    .to_string(),
            );
            if let Some(path) = runtime_state.active_plan_path.as_deref() {
                if is_implemented_plan_path(Path::new(path)) {
                    lines.push(format!(
                        "- 当前计划文件 `{path}` 文件名包含 `-已实现.md`，表示该计划已经实施完成，只能作为历史记录参考。"
                    ));
                } else {
                    lines.push(format!(
                        "- 当前待实施计划文件：`{path}`。实施完成后必须调用 `mark_plan_implemented`。"
                    ));
                }
            }
        }
        DispatcherMode::Plan => {
            lines.push("# 当前模式：Plan".to_string());
            lines.push("- 自主判断任务难度：简单咨询或无需落盘计划书的请求可以直接回复；只有需要形成实施计划时，才进入计划工具流程。".to_string());
            lines.push("- 需要规划时的流程：先探索当前代码与约束；若信息不足，调用 `ask_plan_question`；信息充分后创建/编辑计划书；最后调用 `present_plan`。".to_string());
            lines.push("- 禁止编码、禁止修改普通项目文件、禁止委派 Claude/Codex、禁止使用 `update_plan`。只能使用只读探索工具和计划书工具。".to_string());
            lines.push(
                "- 计划书必须写入当前项目 `.jkcodingagent/plan/*.md`，并且要足够详细到执行代理可直接开工。"
                    .to_string(),
            );
        }
    }
    lines.push(
        "- 任何文件名包含 `-已实现.md` 的计划都表示已经落地，不得重复当作待实施计划。".to_string(),
    );
    lines.join("\n")
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

#[cfg(test)]
mod tests {
    use super::super::super::db::DispatcherSessionRuntimeState;
    use super::*;

    #[test]
    fn default_mode_prompt_requires_checklist_before_exploration_when_used() {
        let state = DispatcherSessionRuntimeState {
            mode: DispatcherMode::Default,
            checklist: None,
            plan_interaction: None,
            active_plan_path: None,
        };

        let prompt = build_dispatcher_mode_block(&state);

        assert!(prompt.contains("复杂任务应主动维护，简单任务可跳过"));
        assert!(prompt.contains("必须先调用 `update_plan` 创建本次任务规划步骤"));
        assert!(prompt.contains("再进行 glob/grep/read_file/exec 探索"));
    }

    #[test]
    fn plan_mode_prompt_allows_simple_direct_reply() {
        let state = DispatcherSessionRuntimeState {
            mode: DispatcherMode::Plan,
            checklist: None,
            plan_interaction: None,
            active_plan_path: None,
        };

        let prompt = build_dispatcher_mode_block(&state);

        assert!(prompt.contains("简单咨询或无需落盘计划书的请求可以直接回复"));
        assert!(prompt.contains("只有需要形成实施计划时，才进入计划工具流程"));
        assert!(prompt.contains("禁止编码、禁止修改普通项目文件"));
        assert!(prompt.contains("禁止使用 `update_plan`"));
    }
}
