use std::path::Path;

use anyhow::Result;
use tauri::ipc::Channel;
use tokio::sync::watch;

use super::{CapabilityBroker, CapabilityInvocation, CapabilitySet, ToolRegistry};
use crate::agent::common;
use crate::agent::db::{DispatcherDb, FinishToolRun, NewToolRun, ToolRunTraceContext};
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
    pub async fn execute_tool_with_cancellation(
        registry: &ToolRegistry,
        workspace: &Path,
        capabilities: &CapabilitySet,
        tool_call: &RequestedToolCall,
        context: &ToolContext,
        cancel_rx: watch::Receiver<bool>,
    ) -> ToolResult {
        CapabilityBroker::new(registry, workspace, capabilities.clone(), context)
            .with_cancellation(cancel_rx)
            .invoke(CapabilityInvocation::model(
                tool_call.id.clone(),
                tool_call.name.clone(),
                tool_call.arguments.clone(),
            ))
            .await
    }

    pub async fn create_and_start_tool_run(
        db: &DispatcherDb,
        registry: &ToolRegistry,
        workspace_id: &str,
        workspace: &Path,
        on_event: &Channel<AgentEvent>,
        tool_call: &RequestedToolCall,
    ) -> Result<String> {
        Self::create_and_start_tool_run_with_trace(
            db,
            registry,
            workspace_id,
            workspace,
            on_event,
            tool_call,
            ToolRunTraceContext::default(),
        )
        .await
    }

    pub async fn create_and_start_tool_run_with_trace(
        db: &DispatcherDb,
        registry: &ToolRegistry,
        workspace_id: &str,
        workspace: &Path,
        on_event: &Channel<AgentEvent>,
        tool_call: &RequestedToolCall,
        trace: ToolRunTraceContext,
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
        let effective_args = registry
            .prepare_input(workspace, &tool_call.name, &tool_call.arguments, true)
            .map(|input| input.effective_arguments)
            // 非法调用同样必须先进入台账。此时保留原始参数（内置工具仍补可确定
            // default）供审计，真正执行会在 Broker 的唯一 prepare_input 入口失败。
            .unwrap_or_else(|_| registry.effective_args(&tool_call.name, &tool_call.arguments));
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
            .create_tool_run_with_trace_async(
                NewToolRun {
                    workspace_id: workspace_id.to_string(),
                    tool_call_id: tool_call.id.clone(),
                    tool_name: tool_call.name.clone(),
                    provider: spec.provider.clone(),
                    category: spec.category.as_str().to_string(),
                    arguments_json: serde_json::to_string(&tool_call.arguments)?,
                    effective_arguments_json: serde_json::to_string(&effective_args)?,
                    metadata_json,
                },
                trace,
            )
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
                            status: "internal_error".to_string(),
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
        if let Some(message_id) = update.message_id {
            let tree = db
                .attach_tool_run_tree_message_async(&run.id, message_id)
                .await?;
            for run in tree {
                common::emit(on_event, AgentEvent::ToolRunUpdated { run });
            }
        } else {
            common::emit(on_event, AgentEvent::ToolRunUpdated { run });
        }
        Ok(())
    }
}
