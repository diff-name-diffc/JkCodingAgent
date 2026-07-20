use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use tauri::ipc::Channel;

use super::ToolRegistry;
use crate::agent::common;
use crate::agent::db::{DispatcherDb, FinishToolRun, NewToolRun};
use crate::agent::llm::RequestedToolCall;
use crate::agent::run_loop::AgentEvent;
use crate::agent::tools::{ToolContext, ToolResult};

#[derive(Debug, Clone, Copy, Default)]
pub struct ToolRunFinishUpdate<'a> {
    pub status: &'a str,
    pub result_mode: Option<&'a str>,
    pub message_id: Option<&'a str>,
    pub error_kind: Option<&'a str>,
    pub error_message: Option<&'a str>,
    pub action_kind: Option<&'a str>,
    pub metadata_json: Option<&'a str>,
}

pub struct ToolRuntime;

impl ToolRuntime {
    pub async fn execute_tool(
        registry: &ToolRegistry,
        workspace: &Path,
        allowed_tool_names: &std::collections::HashSet<String>,
        tool_call: &RequestedToolCall,
        context: &ToolContext,
    ) -> ToolResult {
        let mut execution_context = context.clone();
        execution_context.current_tool_call_id = Some(tool_call.id.clone());
        let spec = registry.spec_by_name(workspace, &tool_call.name, true);
        if !allowed_tool_names.contains(&tool_call.name) {
            return ToolResult::recoverable_error(format!(
                "错误：禁止调用工具 '{}'；它未在当前模式、运行状态或用户配置的可用工具列表中。",
                tool_call.name
            ));
        }

        let Some(spec) = spec else {
            return ToolResult::recoverable_error(format!("错误：未找到工具 '{}'", tool_call.name));
        };

        if !spec.execution.unified_timeout {
            return registry
                .execute(&tool_call.name, &tool_call.arguments, &execution_context)
                .await;
        }

        let timeout_secs = spec.execution.timeout_secs.max(1);
        match tokio::time::timeout(
            Duration::from_secs(timeout_secs),
            registry.execute(&tool_call.name, &tool_call.arguments, &execution_context),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => ToolResult::recoverable_error(format!(
                "错误：工具 '{}' 执行超过统一策略超时 {} 秒，已中止等待。",
                tool_call.name, timeout_secs
            )),
        }
    }

    pub async fn create_and_start_tool_run(
        db: &DispatcherDb,
        registry: &ToolRegistry,
        workspace_id: &str,
        workspace: &Path,
        on_event: &Channel<AgentEvent>,
        tool_call: &RequestedToolCall,
    ) -> Result<String> {
        let spec = registry
            .spec_by_name(workspace, &tool_call.name, true)
            .unwrap_or_else(|| {
                super::ToolSpec::new(
                    &tool_call.name,
                    "未注册工具调用",
                    serde_json::json!({ "type": "object", "properties": {} }),
                )
            });
        let effective_args = registry.effective_args(&tool_call.name, &tool_call.arguments);
        let metadata_json = serde_json::to_string(&serde_json::json!({
            "safety": spec.safety,
            "access": spec.access,
            "execution": spec.execution,
            "resultPolicy": spec.result_policy
        }))?;
        let run = db
            .create_tool_run_async(NewToolRun {
                workspace_id: workspace_id.to_string(),
                tool_call_id: tool_call.id.clone(),
                tool_name: tool_call.name.clone(),
                provider: spec.provider.clone(),
                category: spec.category.as_str().to_string(),
                arguments_json: serde_json::to_string(&tool_call.arguments)?,
                effective_arguments_json: serde_json::to_string(&effective_args)?,
                metadata_json,
            })
            .await?;
        common::emit(on_event, AgentEvent::ToolRunUpdated { run: run.clone() });

        let started = db.mark_tool_run_started_async(&run.id).await?;
        common::emit(
            on_event,
            AgentEvent::ToolRunUpdated {
                run: started.clone(),
            },
        );
        Ok(started.id)
    }

    pub async fn finish_tool_run(
        db: &DispatcherDb,
        on_event: &Channel<AgentEvent>,
        run_id: &str,
        update: ToolRunFinishUpdate<'_>,
    ) -> Result<()> {
        let run = db
            .finish_tool_run_async(
                run_id,
                FinishToolRun {
                    status: update.status.to_string(),
                    result_mode: update.result_mode.map(str::to_string),
                    message_id: update.message_id.map(str::to_string),
                    error_kind: update.error_kind.map(str::to_string),
                    error_message: update.error_message.map(str::to_string),
                    action_kind: update.action_kind.map(str::to_string),
                    metadata_json: update.metadata_json.map(str::to_string),
                },
            )
            .await?;
        common::emit(on_event, AgentEvent::ToolRunUpdated { run });
        Ok(())
    }
}
