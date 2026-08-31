use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant as StdInstant};

use async_trait::async_trait;
use futures::future::BoxFuture;
use parking_lot::Mutex as ParkingMutex;
use serde_json::{json, Value};

use super::{execute_program, execute_program_with_cancellation, ProgramBroker};
use crate::agent::tools::program::{validate_program_value, CapabilityPolicy, ProgramLimits};
use crate::agent::tools::{
    AgentTool, CapabilityBroker, CapabilityInvocation, CapabilitySet, ToolContext, ToolRegistry,
    ToolResult, ToolSafety, ToolSpec, ToolStatus,
};

#[derive(Clone)]
struct MockReply {
    delay: Duration,
    result: ToolResult,
}

#[derive(Default)]
struct MockBroker {
    replies: BTreeMap<String, MockReply>,
    invocations: Mutex<Vec<CapabilityInvocation>>,
    active: AtomicUsize,
    max_active: AtomicUsize,
}

impl MockBroker {
    fn with_reply(mut self, step_id: &str, delay_ms: u64, result: ToolResult) -> Self {
        self.replies.insert(
            step_id.to_string(),
            MockReply {
                delay: Duration::from_millis(delay_ms),
                result,
            },
        );
        self
    }

    fn invocations(&self) -> Vec<CapabilityInvocation> {
        self.invocations.lock().unwrap().clone()
    }
}

impl ProgramBroker for MockBroker {
    fn invoke_capability(&self, invocation: CapabilityInvocation) -> BoxFuture<'_, ToolResult> {
        self.invocations.lock().unwrap().push(invocation.clone());
        let reply = invocation
            .step_id
            .as_deref()
            .and_then(|step_id| self.replies.get(step_id))
            .cloned()
            .unwrap_or_else(|| MockReply {
                delay: Duration::ZERO,
                result: ToolResult::success_data(
                    invocation.arguments.clone(),
                    invocation.arguments.to_string(),
                    invocation.arguments.to_string(),
                ),
            });
        let active = self.active.fetch_add(1, Ordering::AcqRel) + 1;
        self.max_active.fetch_max(active, Ordering::AcqRel);

        Box::pin(async move {
            tokio::time::sleep(reply.delay).await;
            self.active.fetch_sub(1, Ordering::AcqRel);
            reply.result
        })
    }
}

fn validate(program: Value, policy: CapabilityPolicy) -> super::ValidatedProgram {
    validate_program_value(
        &program,
        &|name: &str| (name == "echo").then_some(policy),
        &ProgramLimits::default(),
    )
    .expect("program should validate")
}

fn sequential_program() -> Value {
    json!({
        "version": 1,
        "root": {
            "op": "sequence",
            "steps": [
                {
                    "op": "call",
                    "id": "first",
                    "tool": "echo",
                    "arguments": { "value": "hello" }
                },
                {
                    "op": "call",
                    "id": "second",
                    "tool": "echo",
                    "arguments": {
                        "value": { "$ref": { "step": "first", "pointer": "/data/value" } }
                    }
                },
                {
                    "op": "return",
                    "value": {
                        "value": { "$ref": { "step": "second", "pointer": "/data/value" } },
                        "sequence": { "$ref": { "step": "second", "pointer": "/metadata/broker/sequence" } }
                    }
                }
            ]
        }
    })
}

fn parallel_program(step_ids: &[&str]) -> Value {
    let branches = step_ids
        .iter()
        .map(|step_id| {
            json!({
                "op": "call",
                "id": step_id,
                "tool": "echo",
                "arguments": { "value": step_id }
            })
        })
        .collect::<Vec<_>>();
    let returned = step_ids
        .iter()
        .map(|step_id| json!({ "$ref": { "step": step_id, "pointer": "/data/value" } }))
        .collect::<Vec<_>>();
    json!({
        "version": 1,
        "root": {
            "op": "sequence",
            "steps": [
                { "op": "parallel", "branches": branches },
                { "op": "return", "value": returned }
            ]
        }
    })
}

struct Echo;

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

fn context() -> ToolContext {
    ToolContext {
        workspace_id: "ws".to_string(),
        workspace: std::env::current_dir().expect("current dir"),
        mcp_scope: crate::mcp::McpScope::Global,
        session_title: "test".to_string(),
        user_task: None,
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
        sub_agent_trace_events: Some(Arc::new(ParkingMutex::new(Vec::new()))),
    }
}

#[tokio::test]
async fn real_broker_executes_echo_chain_and_exposes_stable_sequence() {
    let registry = ToolRegistry::new(vec![Box::new(Echo)]);
    let context = context();
    let broker = CapabilityBroker::new(
        &registry,
        CapabilitySet::new(["echo".to_string()]),
        &context,
    );
    let program = validate(sequential_program(), CapabilityPolicy::sequential());

    let result = execute_program(&program, &broker, "outer-1", &ProgramLimits::default()).await;

    assert_eq!(result.status, ToolStatus::Success);
    assert_eq!(
        result.data,
        Some(json!({ "value": "hello", "sequence": 2 }))
    );
    assert_eq!(
        result.metadata["toolProgram"]["completedSteps"],
        json!(["first", "second"])
    );
}

#[tokio::test]
async fn parallel_calls_are_bounded_and_merged_in_declaration_order() {
    let broker = MockBroker::default()
        .with_reply(
            "a",
            50,
            ToolResult::success_data(json!({ "value": "a" }), "a", "a"),
        )
        .with_reply(
            "b",
            5,
            ToolResult::success_data(json!({ "value": "b" }), "b", "b"),
        )
        .with_reply(
            "c",
            10,
            ToolResult::success_data(json!({ "value": "c" }), "c", "c"),
        );
    let program = validate(
        parallel_program(&["a", "b", "c"]),
        CapabilityPolicy::parallel_readonly(),
    );
    let limits = ProgramLimits {
        max_concurrency: 2,
        ..ProgramLimits::default()
    };

    let result = execute_program(&program, &broker, "outer-2", &limits).await;

    assert_eq!(result.status, ToolStatus::Success);
    assert_eq!(result.data, Some(json!(["a", "b", "c"])));
    assert_eq!(
        result.metadata["toolProgram"]["completedSteps"],
        json!(["a", "b", "c"])
    );
    assert_eq!(broker.max_active.load(Ordering::Acquire), 2);
    let mut sequences = broker
        .invocations()
        .into_iter()
        .map(|call| (call.step_id.unwrap(), call.sequence))
        .collect::<Vec<_>>();
    sequences.sort_by_key(|(_, sequence)| *sequence);
    assert_eq!(
        sequences,
        vec![
            ("a".to_string(), 1),
            ("b".to_string(), 2),
            ("c".to_string(), 3)
        ]
    );
}

#[tokio::test]
async fn parallel_failure_stops_unstarted_calls_and_awaits_in_flight_call() {
    let broker = MockBroker::default()
        .with_reply(
            "a",
            80,
            ToolResult::success_data(json!({ "value": "a" }), "a", "a"),
        )
        .with_reply("b", 5, ToolResult::recoverable_error("boom"));
    let program = validate(
        parallel_program(&["a", "b", "c", "d"]),
        CapabilityPolicy::parallel_readonly(),
    );
    let limits = ProgramLimits {
        max_concurrency: 2,
        ..ProgramLimits::default()
    };
    let started_at = StdInstant::now();

    let result = execute_program(&program, &broker, "outer-3", &limits).await;

    assert_eq!(result.status, ToolStatus::RecoverableError);
    assert!(started_at.elapsed() >= Duration::from_millis(70));
    let invoked = broker
        .invocations()
        .into_iter()
        .map(|call| call.step_id.unwrap())
        .collect::<Vec<_>>();
    assert_eq!(invoked, vec!["a", "b"]);
    assert_eq!(
        result.metadata["toolProgram"]["error"]["kind"],
        "child_recoverable"
    );
    assert_eq!(
        result.metadata["toolProgram"]["error"]["completedSteps"],
        json!(["a", "b"])
    );
}

#[tokio::test]
async fn enforces_resolved_arguments_envelope_environment_and_return_budgets() {
    let large = "x".repeat(256);
    let program_value = json!({
        "version": 1,
        "root": { "op": "sequence", "steps": [
            { "op": "call", "id": "first", "tool": "echo", "arguments": {} },
            { "op": "call", "id": "second", "tool": "echo", "arguments": {
                "value": { "$ref": { "step": "first", "pointer": "/data/blob" } }
            } },
            { "op": "return", "value": {
                "$ref": { "step": "first", "pointer": "/data/blob" }
            } }
        ] }
    });
    let program = validate(program_value, CapabilityPolicy::sequential());
    let large_result = ToolResult::success_data(json!({ "blob": large }), "ok", "ok");

    let arguments_broker = MockBroker::default().with_reply("first", 0, large_result.clone());
    let arguments_limits = ProgramLimits {
        max_resolved_arguments_bytes: 128,
        ..ProgramLimits::default()
    };
    let result = execute_program(
        &program,
        &arguments_broker,
        "budget-args",
        &arguments_limits,
    )
    .await;
    assert_eq!(result.status, ToolStatus::RecoverableError);
    assert_eq!(arguments_broker.invocations().len(), 1);

    let envelope_broker = MockBroker::default().with_reply("first", 0, large_result.clone());
    let envelope_limits = ProgramLimits {
        max_step_envelope_bytes: 128,
        ..ProgramLimits::default()
    };
    let result = execute_program(
        &program,
        &envelope_broker,
        "budget-envelope",
        &envelope_limits,
    )
    .await;
    assert_eq!(result.status, ToolStatus::RecoverableError);
    assert_eq!(envelope_broker.invocations().len(), 1);

    let environment_broker = MockBroker::default().with_reply("first", 0, large_result.clone());
    let environment_limits = ProgramLimits {
        max_environment_bytes: 128,
        ..ProgramLimits::default()
    };
    let result = execute_program(
        &program,
        &environment_broker,
        "budget-env",
        &environment_limits,
    )
    .await;
    assert_eq!(result.status, ToolStatus::RecoverableError);
    assert_eq!(environment_broker.invocations().len(), 1);

    let return_broker = MockBroker::default().with_reply("first", 0, large_result);
    let return_limits = ProgramLimits {
        max_return_bytes: 128,
        max_resolved_arguments_bytes: 1024,
        ..ProgramLimits::default()
    };
    // 让第二步成功，最终由解析后的大 return 触发预算。
    let return_broker = return_broker.with_reply(
        "second",
        0,
        ToolResult::success_data(json!({ "value": "ok" }), "ok", "ok"),
    );
    let result = execute_program(&program, &return_broker, "budget-return", &return_limits).await;
    assert_eq!(result.status, ToolStatus::RecoverableError);
    assert_eq!(return_broker.invocations().len(), 2);
}

#[tokio::test]
async fn wall_time_stops_new_work_and_bounds_in_flight_drain() {
    let broker = MockBroker::default().with_reply(
        "first",
        3_000,
        ToolResult::success_data(json!({ "value": "late" }), "late", "late"),
    );
    let program = validate(sequential_program(), CapabilityPolicy::sequential());
    let limits = ProgramLimits {
        max_wall_time_secs: 1,
        max_drain_time_ms: 50,
        ..ProgramLimits::default()
    };
    let started_at = StdInstant::now();
    let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);

    let result =
        execute_program_with_cancellation(&program, &broker, "outer-timeout", &limits, cancel_tx)
            .await;

    assert_eq!(result.status, ToolStatus::RecoverableError);
    assert!(started_at.elapsed() >= Duration::from_millis(1_000));
    assert!(started_at.elapsed() < Duration::from_millis(1_500));
    assert_eq!(broker.invocations().len(), 1);
    assert!(*cancel_rx.borrow());
    assert_eq!(
        result.metadata["toolProgram"]["error"]["kind"],
        "deadline_exceeded"
    );
}

#[tokio::test]
async fn maps_fatal_and_cancelled_child_results() {
    for (reply, expected_status, expected_kind) in [
        (
            ToolResult::fatal_error("fatal"),
            ToolStatus::FatalError,
            "child_fatal",
        ),
        (
            ToolResult::cancelled("cancelled"),
            ToolStatus::Cancelled,
            "cancelled",
        ),
    ] {
        let broker = MockBroker::default().with_reply("first", 0, reply);
        let program = validate(sequential_program(), CapabilityPolicy::sequential());

        let result =
            execute_program(&program, &broker, "outer-error", &ProgramLimits::default()).await;

        assert_eq!(result.status, expected_status);
        assert_eq!(
            result.metadata["toolProgram"]["error"]["kind"],
            expected_kind
        );
        assert_eq!(broker.invocations().len(), 1);
    }
}
