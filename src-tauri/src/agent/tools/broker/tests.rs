use std::time::Duration;

use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio::sync::watch;

use super::{CapabilityBroker, CapabilityInvocation};
use crate::agent::tools::{
    AgentTool, CapabilitySet, ToolContext, ToolRegistry, ToolResult, ToolSafety, ToolSpec,
};

struct Echo;
struct WaitForCancel;
struct ScopedWrite;

#[async_trait]
impl AgentTool for Echo {
    fn name(&self) -> &'static str {
        "echo"
    }

    fn description(&self) -> &'static str {
        "echo"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "value": { "type": "string" } },
            "required": ["value"],
            "additionalProperties": false
        })
    }

    fn spec(&self) -> ToolSpec {
        let mut spec = ToolSpec::new(self.name(), self.description(), self.parameters());
        spec.safety = ToolSafety::Safe;
        spec
    }

    async fn execute(&self, args: &Value, _context: &ToolContext) -> ToolResult {
        ToolResult::success_data(
            json!({ "value": args["value"] }),
            args["value"].as_str().unwrap_or_default(),
            args["value"].as_str().unwrap_or_default(),
        )
    }
}

#[async_trait]
impl AgentTool for WaitForCancel {
    fn name(&self) -> &'static str {
        "wait_for_cancel"
    }

    fn description(&self) -> &'static str {
        "wait"
    }

    fn parameters(&self) -> Value {
        json!({ "type": "object", "additionalProperties": false })
    }

    fn spec(&self) -> ToolSpec {
        let mut spec = ToolSpec::new(self.name(), self.description(), self.parameters());
        spec.safety = ToolSafety::Safe;
        spec.execution.timeout_secs = 10;
        spec
    }

    async fn execute(&self, _args: &Value, context: &ToolContext) -> ToolResult {
        let Some(mut cancel_rx) = context.cancel_rx.clone() else {
            return ToolResult::fatal_error("测试工具缺少取消信号");
        };
        while !*cancel_rx.borrow() {
            if cancel_rx.changed().await.is_err() {
                return ToolResult::fatal_error("取消通道提前关闭");
            }
        }
        ToolResult::cancelled("测试工具已终止")
    }
}

#[async_trait]
impl AgentTool for ScopedWrite {
    fn name(&self) -> &'static str {
        "write_file"
    }

    fn description(&self) -> &'static str {
        "write"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "path": { "type": "string" },
                "content": { "type": "string" },
            },
            "required": ["path", "content"],
        })
    }

    async fn execute(&self, _args: &Value, _context: &ToolContext) -> ToolResult {
        ToolResult::success_text("written")
    }
}

fn context() -> ToolContext {
    ToolContext {
        workspace_id: "ws".to_string(),
        workspace: std::env::current_dir().expect("current dir"),
        mcp_scope: crate::mcp::McpScope::Global,
        session_title: "test".to_string(),
        user_task: None,
        executor_task: None,
        review_conversation: None,
        ssh_review: None,
        exec_timeout_secs: 1,
        restrict_to_workspace: true,
        extra_allowed_dirs: Vec::new(),
        app_handle: None,
        llm_provider: None,
        vision_model: String::new(),
        vision_provider: None,
        image_model_url: String::new(),
        image_model_api_key: String::new(),
        image_model: String::new(),
        image_edit_model: String::new(),
        sub_agent_tool_registry: None,
        current_sub_agent_id: None,
        current_sub_agent_name: None,
        current_tool_call_id: None,
        current_tool_spec_hash: None,
        cancel_rx: None,
        sub_agent_parent_tool_call_id: None,
        sub_agent_trace_events: Some(std::sync::Arc::new(Mutex::new(Vec::new()))),
    }
}

#[tokio::test]
async fn denies_calls_outside_the_grant_before_dispatch() {
    let registry = ToolRegistry::new(vec![Box::new(Echo)]);
    let context = context();
    let broker = CapabilityBroker::new(&registry, CapabilitySet::default(), &context);

    let result = broker
        .invoke(CapabilityInvocation::model(
            "call-1",
            "echo",
            json!({ "value": "hello" }),
        ))
        .await;

    assert!(result.output_for_llm().contains("未授予"));
}

#[tokio::test]
async fn validates_arguments_then_dispatches_with_structured_data() {
    let registry = ToolRegistry::new(vec![Box::new(Echo)]);
    let context = context();
    let broker = CapabilityBroker::new(
        &registry,
        CapabilitySet::new(["echo".to_string()]),
        &context,
    );

    let invalid = broker
        .invoke(CapabilityInvocation::model(
            "call-1",
            "echo",
            json!({ "value": 7 }),
        ))
        .await;
    assert!(invalid.output_for_llm().contains("参数"));

    let valid = broker
        .invoke(CapabilityInvocation::model(
            "call-2",
            "echo",
            json!({ "value": "hello" }),
        ))
        .await;
    assert_eq!(valid.data, Some(json!({ "value": "hello" })));
    assert_eq!(valid.metadata["broker"]["origin"], "model");
}

#[tokio::test]
async fn rejects_oversized_tool_program_result_before_building_envelope() {
    let registry = ToolRegistry::new(vec![Box::new(Echo)]);
    let context = context();
    let broker = CapabilityBroker::new(
        &registry,
        CapabilitySet::new(["echo".to_string()]),
        &context,
    );

    let result = broker
        .invoke(CapabilityInvocation::tool_program(
            "outer:1",
            "large",
            1,
            "echo",
            json!({ "value": "x".repeat(200 * 1024) }),
        ))
        .await;

    assert_eq!(
        result.status,
        crate::agent::tools::ToolStatus::RecoverableError
    );
    assert!(result.output_for_llm().contains("超过单步"));
    assert_eq!(result.metadata["resultBudget"]["exceeded"], true);
    assert!(result.data.is_none());
}

#[tokio::test]
async fn records_confirmed_cooperative_cancellation() {
    let registry = ToolRegistry::new(vec![Box::new(WaitForCancel)]);
    let context = context();
    let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
    let broker = CapabilityBroker::new(
        &registry,
        CapabilitySet::new(["wait_for_cancel".to_string()]),
        &context,
    )
    .with_cancellation(cancel_rx);

    let invocation = broker.invoke(CapabilityInvocation::model(
        "call-cancel",
        "wait_for_cancel",
        json!({}),
    ));
    let request_cancel = async move {
        tokio::task::yield_now().await;
        cancel_tx.send(true).expect("send cancellation");
    };
    let (result, ()) = tokio::join!(invocation, request_cancel);

    assert_eq!(result.status, crate::agent::tools::ToolStatus::Cancelled);
    assert_eq!(result.metadata["termination"]["state"], "terminated");
    assert_eq!(result.metadata["termination"]["confirmed"], true);
}

#[tokio::test]
async fn unified_timeout_requests_tool_cancellation_and_waits_for_settlement() {
    let registry = ToolRegistry::new(vec![]);
    let context = context();
    let broker = CapabilityBroker::new(&registry, CapabilitySet::default(), &context);
    let invocation = CapabilityInvocation::model("deadline", "slow_tool", json!({}));
    let (execution_cancel_tx, mut execution_cancel_rx) = watch::channel(false);
    let execution = async move {
        while !*execution_cancel_rx.borrow() {
            execution_cancel_rx
                .changed()
                .await
                .expect("cancellation channel remains open");
        }
        ToolResult::cancelled("cooperatively stopped")
    };
    tokio::pin!(execution);

    let result = broker
        .await_with_timeout(
            &invocation,
            true,
            Duration::from_millis(10),
            &execution_cancel_tx,
            execution,
        )
        .await;

    assert_eq!(
        result.status,
        crate::agent::tools::ToolStatus::RecoverableError
    );
    assert_eq!(result.metadata["termination"]["state"], "terminated");
    assert_eq!(result.metadata["termination"]["confirmed"], true);
    assert_eq!(result.metadata["deadline"]["originalStatus"], "cancelled");
}

#[tokio::test]
async fn enforces_graph_write_scopes_before_dispatch() {
    let registry = ToolRegistry::new(vec![Box::new(ScopedWrite)]);
    let context = context();
    let broker = CapabilityBroker::new(
        &registry,
        CapabilitySet::new(["write_file".to_string()])
            .restrict_writes_to(["src/allowed.rs".to_string()]),
        &context,
    );

    let denied = broker
        .invoke(CapabilityInvocation::model(
            "write-denied",
            "write_file",
            json!({ "path": "src/other.rs", "content": "x" }),
        ))
        .await;
    let allowed = broker
        .invoke(CapabilityInvocation::model(
            "write-allowed",
            "write_file",
            json!({ "path": "src/allowed.rs", "content": "x" }),
        ))
        .await;

    assert_eq!(
        denied.status,
        crate::agent::tools::ToolStatus::RecoverableError
    );
    assert!(denied.output_for_llm().contains("expectedFiles"));
    assert_eq!(allowed.status, crate::agent::tools::ToolStatus::Success);
    assert_eq!(
        allowed.metadata["policyDecision"]["resourceScope"]["mode"],
        "expected_files"
    );
}
