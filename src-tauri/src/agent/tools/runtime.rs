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
        // 执行入口统一规范化路径边界输入，保证所有工具拿到的
        // workspace / extra_allowed_dirs 均为 canonical 形式。
        execution_context.normalize_paths();
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

        // unified_timeout=false：工具自管超时（exec / call_sub_agent 等）；
        // timeout_secs=0：策略层明确约定为「不设统一超时限制」，同样跳过包裹，
        // 避免把 0 曲解为 1 秒而误中止长耗时工具。
        if !spec.execution.unified_timeout || spec.execution.timeout_secs == 0 {
            return registry
                .execute(&tool_call.name, &tool_call.arguments, &execution_context)
                .await;
        }

        let timeout_secs = spec.execution.timeout_secs;
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
        let registered_spec = registry.spec_by_name(workspace, &tool_call.name, true);
        let registered = registered_spec.is_some();
        let spec = registered_spec.unwrap_or_else(|| {
            super::ToolSpec::new(
                &tool_call.name,
                "未注册工具调用",
                serde_json::json!({ "type": "object", "properties": {} }),
            )
        });
        let effective_args = registry.effective_args(&tool_call.name, &tool_call.arguments);
        // 未注册（LLM 幻觉）工具名：不落任何策略审计字段并显式标记
        // registered=false，避免审计侧误以为该调用经过真实策略评估。
        let metadata_json = if registered {
            serde_json::to_string(&serde_json::json!({
                "registered": true,
                "safety": spec.safety,
                "access": spec.access,
                "execution": spec.execution,
                "resultPolicy": spec.result_policy
            }))?
        } else {
            serde_json::to_string(&serde_json::json!({ "registered": false }))?
        };
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

        let started = match db.mark_tool_run_started_async(&run.id).await {
            Ok(started) => started,
            Err(error) => {
                // 补偿：标记启动失败时把记录收敛到失败终态，
                // 避免遗留「已创建未启动」的永久中间态记录。
                if let Ok(finished) = db
                    .finish_tool_run_async(
                        &run.id,
                        FinishToolRun {
                            status: "failed".to_string(),
                            result_mode: None,
                            message_id: None,
                            error_kind: Some("internal".to_string()),
                            error_message: Some(format!("标记工具运行启动失败：{error}")),
                            action_kind: None,
                            metadata_json: None,
                        },
                    )
                    .await
                {
                    common::emit(on_event, AgentEvent::ToolRunUpdated { run: finished });
                }
                return Err(error);
            }
        };
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
