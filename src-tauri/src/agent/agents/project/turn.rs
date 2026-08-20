//! `RunLoopAgent` + `AgentRunAdapter` 实现：把编排器接入公共 run_loop 骨架
//! （循环 / DB 持久化 / 用量统计 / ActiveRunStore 取消全部复用）。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use async_trait::async_trait;
use tauri::Manager;

use crate::agent::common::LlmStreamOutcome;
use crate::agent::db::{DispatcherMessageRecord, DEFAULT_CONTEXT_WINDOW_CAPACITY_TOKENS};
use crate::agent::debug::ContextDebugLogger;
use crate::agent::llm::{ChatMessage, LlmResponse, OpenAiCompatProvider, ToolDefinition};
use crate::agent::prompt::PromptBundle;
use crate::agent::run_loop::agent_loop::AgentLoop;
use crate::agent::run_loop::core::{
    AgentRunAdapter, AgentRunRequest, RunLoopAgent, RunLoopContext, RunLoopIteration,
    RunLoopToolOutcome, RunPromptState, RuntimeAgentKind,
};
use crate::agent::tools::{
    CapabilitySet, ToolContext, ToolSurface, ORCHESTRATOR_RUNTIME_TOOL_NAMES,
};

use super::helpers;
use super::OrchestratorAgent;

#[async_trait]
impl AgentRunAdapter for OrchestratorAgent {
    async fn prepare_run_workspace(&self, request: &AgentRunRequest<'_>) -> Result<PathBuf> {
        let workspace_path = request
            .workspace_path
            .ok_or_else(|| anyhow::anyhow!("错误：项目 Agent 启动缺少 workspace_path"))?
            .to_string();
        // 命令边界校验（审查项 G8-12）：只允许受管项目列表（projects 表）内的
        // 路径作为工作区，先解析符号链接再做包含性判断，防止前端可控路径被用于
        // 任意目录创建/读取。DB 读取放在 spawn_blocking，不阻塞执行器。
        let db = self.db.clone();
        let workspace = tokio::task::spawn_blocking(move || {
            validate_project_workspace(&db, &workspace_path)
        })
        .await
        .map_err(|error| anyhow::anyhow!("错误：工作区校验任务失败：{error}"))??;
        let workspace_for_create = workspace.clone();
        tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(&workspace_for_create)
                .with_context(|| format!("create workspace {}", workspace_for_create.display()))
        })
        .await
        .map_err(|error| anyhow::anyhow!("create workspace task failed: {error}"))??;
        Ok(workspace)
    }

    fn provider_snapshot(&self) -> OpenAiCompatProvider {
        self.models.lock().provider.clone()
    }

    fn provider_missing_message(&self) -> &'static str {
        "项目编排 Agent 的 LLM API Key 未配置。请在设置中配置，或设置 DASHSCOPE_API_KEY / OPENAI_API_KEY 环境变量。"
    }

    async fn build_run_prompt(
        &self,
        workspace_id: &str,
        _workspace: &Path,
    ) -> Result<RunPromptState> {
        let mut static_prompt = self.build_static_prompt().await?;
        let app = self
            .app_handle
            .as_ref()
            .context("项目 Agent 缺少 AppHandle")?;
        let state = app.state::<crate::agent::state::DispatcherState>();
        let catalog = crate::agent::graph::commands::catalog_for_workspace(&state, workspace_id)
            .await
            .map_err(anyhow::Error::msg)?;
        // 轻量学习回路：既往节点运行统计回注目录，辅助编排器选模型。
        // 统计查询失败时跳过历史统计、不阻塞提示词构建，但必须留下日志，
        // 否则学习回路静默失效时无任何可诊断痕迹。
        let stats = match crate::agent::graph::GraphStore::new(state.db())
            .node_run_stats_async(workspace_id)
            .await
        {
            Ok(stats) => stats,
            Err(error) => {
                // 持久化警告：打包后 stderr 不落盘，学习回路静默失效时必须
                // 有可诊断痕迹（审查项 G8-13）。
                helpers::log_warning(&format!(
                    "[graph] 读取节点运行统计失败（{workspace_id}），目录回注不含历史统计：{error:#}"
                ));
                Vec::new()
            }
        };
        static_prompt.push_str("\n\n---\n\n");
        static_prompt.push_str(&self.render_graph_harness_catalog(&catalog, &stats));
        Ok(RunPromptState {
            initial_system_prompt: static_prompt.clone(),
            project_prompt: Some(PromptBundle {
                static_content: static_prompt,
            }),
        })
    }
}

#[async_trait]
impl RunLoopAgent for OrchestratorAgent {
    async fn build_loop_tool_context(&self, ctx: &RunLoopContext<'_>) -> ToolContext {
        self.build_tool_context(ctx.db, ctx.workspace_id, ctx.workspace, &ctx.provider)
            .await
    }

    fn max_tool_iterations(&self) -> usize {
        self.config.max_tool_iterations
    }

    /// 模型只看到运行时入口与控制面工具；真实只读能力通过独立 grant 注入
    /// ToolProgram。项目工具设置只收窄该 grant，不会隐藏协议收口工具。
    fn tool_surface_for_loop(&self, _workspace_id: &str, workspace: &Path) -> ToolSurface {
        let mut definitions = self.tools.definitions_for_workspace(
            workspace,
            Option::<std::iter::Empty<&str>>::None,
            false,
        );
        definitions.retain(|definition| {
            matches!(
                definition.function.name.as_str(),
                "run_tool_program" | "message" | "submit_graph" | "graph_plan_report"
            )
        });

        let configured = self.allowed_runtime_tools.lock().clone();
        let runtime_names = ORCHESTRATOR_RUNTIME_TOOL_NAMES
            .into_iter()
            .filter(|name| configured.is_empty() || configured.iter().any(|item| item == name))
            .map(str::to_string)
            .collect::<Vec<_>>();
        if let Some(runtime_definition) = definitions
            .iter_mut()
            .find(|definition| definition.function.name == "run_tool_program")
        {
            let granted = if runtime_names.is_empty() {
                "（无；当前设置禁止全部数据面能力）".to_string()
            } else {
                runtime_names
                    .iter()
                    .map(|name| format!("`{name}`"))
                    .collect::<Vec<_>>()
                    .join("、")
            };
            runtime_definition
                .function
                .description
                .push_str(&format!(" 当前会话实际授权的数据面能力：{granted}。"));
        }
        ToolSurface::layered(definitions, CapabilitySet::new(runtime_names))
    }

    fn build_iteration_messages(
        &self,
        ctx: &RunLoopContext<'_>,
        agent_loop: &AgentLoop,
        tool_definitions: &[ToolDefinition],
    ) -> Result<Vec<ChatMessage>> {
        let Some(static_prompt) = ctx.project_prompt.as_ref() else {
            anyhow::bail!("编排器 run_loop 缺少静态提示词状态");
        };
        let debug_logger =
            ContextDebugLogger::new(self.context_debug_enabled(), PathBuf::from(ctx.workspace));
        let estimated_tokens = agent_loop.estimated_tokens();
        if estimated_tokens > DEFAULT_CONTEXT_WINDOW_CAPACITY_TOKENS * 8 / 10 {
            debug_logger.log(
                "上下文窗口接近上限",
                vec![
                    ("工作区".to_string(), ctx.workspace_id.to_string()),
                    ("估算tokens".to_string(), estimated_tokens.to_string()),
                    (
                        "容量".to_string(),
                        DEFAULT_CONTEXT_WINDOW_CAPACITY_TOKENS.to_string(),
                    ),
                ],
                vec![],
            );
        }

        let system_prompt = self.build_iteration_system_prompt(static_prompt, tool_definitions);
        let mut messages = vec![ChatMessage::system(system_prompt)];
        messages.extend(agent_loop.request_messages().into_iter().skip(1));
        Ok(messages)
    }

    fn provider_for_iteration(
        &self,
        ctx: &RunLoopContext<'_>,
        messages: &[ChatMessage],
        iteration: usize,
    ) -> Result<OpenAiCompatProvider> {
        self.provider_for_messages(&ctx.provider, messages, ctx.on_event, iteration == 0)
    }

    async fn stream_iteration_response(
        &self,
        ctx: &mut RunLoopContext<'_>,
        iteration: &RunLoopIteration,
        iteration_index: usize,
    ) -> Result<LlmStreamOutcome> {
        self.stream_llm_response_inner(ctx, iteration, iteration_index)
            .await
    }

    async fn handle_cancelled_loop(
        &self,
        ctx: &RunLoopContext<'_>,
        partial: &str,
        last_seq: Option<u64>,
    ) -> Result<DispatcherMessageRecord> {
        self.emit_stop_and_finish(
            ctx.db,
            ctx.workspace_id,
            ctx.on_event,
            partial,
            &ctx.usage_tracker,
            last_seq,
        )
        .await
    }

    async fn handle_no_tool_response(
        &self,
        ctx: &RunLoopContext<'_>,
        response: &LlmResponse,
        last_seq: Option<u64>,
    ) -> Result<DispatcherMessageRecord> {
        OrchestratorAgent::handle_no_tool_response(
            self,
            ctx.db,
            ctx.workspace_id,
            ctx.on_event,
            response,
            &ctx.usage_tracker,
            last_seq,
        )
        .await
    }

    async fn execute_loop_tool_calls(
        &self,
        ctx: &mut RunLoopContext<'_>,
        iteration: &RunLoopIteration,
        tool_context: &ToolContext,
        response: LlmResponse,
    ) -> Result<RunLoopToolOutcome> {
        // workspace 单一来源：统一取 tool_context.workspace（执行入口已规范化），
        // 不再额外传入可能与上下文漂移的平行路径（审查项 G8-23）。
        self.execute_tool_calls(
            ctx.db,
            ctx.workspace_id,
            ctx.on_event,
            response,
            &iteration.direct_capabilities,
            &iteration.runtime_capabilities,
            tool_context,
            &ctx.cancel_rx,
            &iteration.request_provider,
            &mut ctx.usage_tracker,
        )
        .await
    }

    async fn resolve_loop_outcome(
        &self,
        ctx: &RunLoopContext<'_>,
        outcome: RunLoopToolOutcome,
    ) -> Result<Option<DispatcherMessageRecord>> {
        OrchestratorAgent::resolve_loop_outcome(
            self,
            ctx.db,
            ctx.workspace_id,
            ctx.on_event,
            outcome,
            &ctx.usage_tracker,
        )
        .await
    }

    fn max_iterations_error(&self, kind: RuntimeAgentKind) -> String {
        match kind {
            RuntimeAgentKind::Project => format!(
                "已达到最大工具迭代次数（{}），本轮编排被终止。请检查模型是否陷入工具调用循环。",
                self.config.max_tool_iterations
            ),
            RuntimeAgentKind::PlainChat => format!(
                "已达到最大工具迭代次数（{}），本轮聊天被终止。请检查模型是否陷入工具调用循环。",
                self.config.max_tool_iterations
            ),
        }
    }
}

// ─── Workspace boundary validation ───────────────────────────────────────────

/// 项目编排器工作区边界校验（审查项 G8-12）：
/// 1. 路径必须为绝对路径且不含 `..` 组件；
/// 2. 解析符号链接（对最深的已存在祖先目录 canonicalize 后拼回缺失尾部）；
/// 3. 解析后的路径必须命中受管项目列表（全局 projects 表）。
/// 校验通过时返回解析后的路径，供后续建目录与工具路径边界统一使用。
fn validate_project_workspace(
    db: &crate::agent::db::DispatcherDb,
    workspace_path: &str,
) -> Result<PathBuf> {
    let raw = PathBuf::from(workspace_path);
    anyhow::ensure!(
        raw.is_absolute(),
        "错误：项目路径必须是绝对路径：{workspace_path}"
    );
    anyhow::ensure!(
        !raw.components()
            .any(|component| matches!(component, std::path::Component::ParentDir)),
        "错误：项目路径不允许包含 ..：{workspace_path}"
    );

    let projects = db
        .list_projects()
        .map_err(|error| anyhow::anyhow!("错误：读取受管项目列表失败：{error}"))?;
    anyhow::ensure!(
        !projects.is_empty(),
        "错误：受管项目列表为空，无法校验项目路径：{workspace_path}"
    );

    let candidate = resolve_with_existing_prefix(&raw)
        .ok_or_else(|| anyhow::anyhow!("错误：解析项目路径失败：{workspace_path}"))?;
    let managed = projects.iter().any(|project| {
        resolve_with_existing_prefix(Path::new(&project.path)).is_some_and(|path| path == candidate)
    });
    anyhow::ensure!(
        managed,
        "错误：项目路径不在受管项目列表中：{workspace_path}"
    );
    Ok(candidate)
}

/// 对可能尚不存在的路径做尽力解析：canonicalize 最深的已存在祖先目录，
/// 再拼回缺失的尾部组件；对已存在路径等价于 canonicalize。
/// 解析失败（无祖先可解析 / canonicalize 失败）返回 None。
fn resolve_with_existing_prefix(path: &Path) -> Option<PathBuf> {
    let mut missing: Vec<std::ffi::OsString> = Vec::new();
    let mut cursor = path;
    loop {
        match std::fs::symlink_metadata(cursor) {
            Ok(_) => {
                let mut resolved = cursor.canonicalize().ok()?;
                for name in missing.iter().rev() {
                    resolved.push(name);
                }
                return Some(resolved);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let name = cursor.file_name()?;
                missing.push(name.to_os_string());
                cursor = cursor.parent()?;
            }
            Err(_) => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_with_existing_prefix;
    use std::path::{Component, Path, PathBuf};

    #[test]
    fn resolve_with_existing_prefix_handles_missing_tail() {
        let base = std::env::temp_dir();
        let canonical_base = base.canonicalize().expect("canonicalize temp dir");
        let target = canonical_base
            .join("jk-no-such-dir-a")
            .join("jk-no-such-dir-b");
        let resolved = resolve_with_existing_prefix(&target).expect("resolve");
        assert_eq!(
            resolved,
            canonical_base
                .join("jk-no-such-dir-a")
                .join("jk-no-such-dir-b")
        );
    }

    #[test]
    fn resolve_with_existing_prefix_equivalent_to_canonicalize_for_existing() {
        let base = std::env::temp_dir()
            .canonicalize()
            .expect("canonicalize temp dir");
        assert_eq!(resolve_with_existing_prefix(&base).as_ref(), Some(&base));
    }

    #[test]
    fn parent_dir_component_detection() {
        // 与 validate_project_workspace 相同的 Component::ParentDir 判定逻辑。
        let hostile = PathBuf::from("/tmp/foo/../bar");
        assert!(hostile
            .components()
            .any(|component| matches!(component, Component::ParentDir)));
        let clean = Path::new("/tmp/foo/bar");
        assert!(!clean
            .components()
            .any(|component| matches!(component, Component::ParentDir)));
    }
}
