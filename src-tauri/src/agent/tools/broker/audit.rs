use super::*;

impl CapabilityBroker<'_> {
    pub(super) async fn start_child_run(
        &self,
        invocation: &CapabilityInvocation,
    ) -> anyhow::Result<Option<String>> {
        let Some(audit) = self.audit else {
            return Ok(None);
        };
        let tool_call = RequestedToolCall {
            id: invocation.id.clone(),
            name: invocation.name.clone(),
            arguments: invocation.arguments.clone(),
        };
        ToolRuntime::create_and_start_tool_run_with_trace(
            audit.db,
            self.registry,
            audit.workspace_id,
            &self.context.mcp_scope,
            audit.on_event,
            &tool_call,
            ToolRunTraceContext {
                parent_run_id: Some(audit.parent_run_id.to_string()),
                origin: invocation.origin.as_str().to_string(),
                step_id: invocation.step_id.clone(),
                sequence: invocation.sequence,
            },
        )
        .await
        .map(Some)
    }

    pub(super) async fn finish_child_run(
        &self,
        run_id: Option<String>,
        invocation: &CapabilityInvocation,
        mut result: ToolResult,
    ) -> ToolResult {
        let (Some(audit), Some(run_id)) = (self.audit, run_id) else {
            return result;
        };
        if !result.artifacts.is_empty() {
            if let Err(error) = audit
                .db
                .insert_tool_artifacts_for_run_async(
                    audit.workspace_id,
                    &run_id,
                    &invocation.id,
                    &invocation.name,
                    &result.artifacts,
                )
                .await
            {
                result = ToolResult::fatal_error(format!(
                    "错误：内部工具 '{}' 已执行，但产物审计失败：{error:#}",
                    invocation.name
                ));
            }
        }
        let result_text = result.output_for_llm();
        let metadata = result.run_metadata_json();
        if let Err(error) = ToolRuntime::finish_tool_run(
            audit.db,
            audit.on_event,
            &run_id,
            ToolRunFinishUpdate {
                status: result.status.as_run_status(),
                result_mode: Some("raw"),
                message_id: None,
                error_kind: result.status.error_kind(),
                error_message: result.status.error_kind().map(|_| result_text.as_str()),
                action_kind: result.action.as_ref().map(super::super::ToolAction::kind),
                metadata_json: metadata.as_deref(),
            },
        )
        .await
        {
            return ToolResult::fatal_error(format!(
                "错误：内部工具 '{}' 已执行，但审计记录收尾失败：{error:#}",
                invocation.name
            ));
        }
        result
    }
}
