//! 项目编排器的 ToolProgram 协议拦截。
//!
//! 外层 LLM 只看到一次 run_tool_program 调用；内部 call 由 CapabilityBroker
//! 执行并写入 parent_run_id 关联的审计树，不生成额外 LLM tool message。

use tauri::ipc::Channel;
use tokio::sync::watch;

use crate::agent::db::DispatcherDb;
use crate::agent::llm::RequestedToolCall;
use crate::agent::run_loop::AgentEvent;
use crate::agent::tools::program::{
    execute_program_with_cancellation, program_error_result, validate_program_value,
    CapabilityPolicy, ProgramLimits,
};
use crate::agent::tools::{BrokerAudit, CapabilityBroker, CapabilitySet, ToolContext, ToolResult};

use super::OrchestratorAgent;

impl OrchestratorAgent {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn intercept_tool_program(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        on_event: &Channel<AgentEvent>,
        tool_call: &RequestedToolCall,
        parent_run_id: &str,
        runtime_capabilities: &CapabilitySet,
        tool_context: &ToolContext,
        cancel_rx: &watch::Receiver<bool>,
    ) -> ToolResult {
        let limits = ProgramLimits::default();
        let catalog = |name: &str| {
            if !runtime_capabilities.contains(name) {
                return None;
            }
            self.tools
                .spec_by_name(&tool_context.mcp_scope, name, false)
                .map(|spec| CapabilityPolicy {
                    supports_parallel_readonly: spec.supports_parallel_readonly(),
                })
        };
        let program = match validate_program_value(&tool_call.arguments, &catalog, &limits) {
            Ok(program) => program,
            Err(error) => return program_error_result(error),
        };

        // ToolProgram 的 wall-time 也是能力调用的真实取消边界。把它与父循环
        // 取消合并成独立通道，确保 executor 进入 drain 阶段时 Broker 已经在
        // 请求子工具终止，而不是仅仅停止调度新步骤。
        let parent_cancelled = *cancel_rx.borrow();
        let (program_cancel_tx, program_cancel_rx) = watch::channel(parent_cancelled);
        let cancel_forwarder = if parent_cancelled {
            None
        } else {
            let mut parent_cancel_rx = cancel_rx.clone();
            let program_cancel_tx = program_cancel_tx.clone();
            Some(tokio::spawn(async move {
                loop {
                    match parent_cancel_rx.changed().await {
                        Ok(()) if *parent_cancel_rx.borrow() => {
                            let _ = program_cancel_tx.send(true);
                            return;
                        }
                        Ok(()) => {}
                        Err(_) => {
                            let _ = program_cancel_tx.send(true);
                            return;
                        }
                    }
                }
            }))
        };

        let broker = CapabilityBroker::new(&self.tools, runtime_capabilities.clone(), tool_context)
            .include_dynamic(false)
            .with_cancellation(program_cancel_rx)
            .with_audit(BrokerAudit {
                db,
                workspace_id,
                on_event,
                parent_run_id,
            });

        let result = execute_program_with_cancellation(
            &program,
            &broker,
            &tool_call.id,
            &limits,
            program_cancel_tx,
        )
        .await;
        if let Some(cancel_forwarder) = cancel_forwarder {
            cancel_forwarder.abort();
        }
        result
    }
}
