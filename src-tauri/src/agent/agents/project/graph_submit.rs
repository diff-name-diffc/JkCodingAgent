//! `submit_graph` 协议拦截与本轮收口。
//!
//! 壳工具只回显；这里完成真正的动作：解析图定义 → `graph/validate` 校验 →
//! 落 `graph_plans` → 广播 `graph-plan-updated`。校验失败按可重试工具错误
//! 交回模型自修复；成功则由 `resolve_loop_outcome` 以「图已生成，等待确认」收口。

use anyhow::Result;
use tauri::ipc::Channel;
use tauri::{Emitter, Manager};

use crate::agent::db::{DispatcherDb, DispatcherMessageRecord};
use crate::agent::graph::commands::catalog_for_workspace;
use crate::agent::graph::types::{GraphDefinition, GraphPlanUpdatedPayload};
use crate::agent::graph::validate::validate_graph;
use crate::agent::graph::GraphStore;
use crate::agent::llm::RequestedToolCall;
use crate::agent::run_loop::core::{LoopProtocolAction, RunLoopToolOutcome};
use crate::agent::run_loop::AgentEvent;

use super::helpers::emit;
use super::OrchestratorAgent;

/// submit_graph 拦截结果：成功时携带协议动作（收口用），失败时按可重试工具错误交回模型自修复。
pub(super) enum SubmitGraphInterception {
    Submitted {
        display_text: String,
        action: LoopProtocolAction,
    },
    Rejected {
        error: String,
    },
}

impl OrchestratorAgent {
    /// submit_graph 协议拦截：解析图定义 → 校验 → 落 graph_plans → 广播 graph-plan-updated。
    /// 校验失败按可重试错误返回，错误文本包含全部问题，供模型自修复后重提。
    pub(super) async fn intercept_submit_graph(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        tool_call: &RequestedToolCall,
        already_submitted: bool,
    ) -> Result<SubmitGraphInterception> {
        if already_submitted {
            return Ok(SubmitGraphInterception::Rejected {
                error: "错误：本轮已提交过执行图，每轮最多提交一次；如需调整，请先等待本轮收口后再修改。"
                    .to_string(),
            });
        }

        let definition = match parse_graph_definition(&tool_call.arguments) {
            Ok(definition) => definition,
            Err(error) => return Ok(SubmitGraphInterception::Rejected { error }),
        };

        let Some(app_handle) = &self.app_handle else {
            return Ok(SubmitGraphInterception::Rejected {
                error: "错误：应用运行时未初始化，无法发现 PI Harness".to_string(),
            });
        };
        let dispatcher_state = app_handle.state::<crate::agent::state::DispatcherState>();
        let catalog = match catalog_for_workspace(&dispatcher_state, workspace_id).await {
            Ok(catalog) => catalog,
            Err(error) => {
                return Ok(SubmitGraphInterception::Rejected {
                    error: format!("错误：发现 PI Harness 失败：{error}"),
                })
            }
        };
        if let Err(error) = validate_graph(&definition, &catalog) {
            return Ok(SubmitGraphInterception::Rejected { error });
        }

        let store = GraphStore::new(db);
        let plan = match store.create_plan(workspace_id, &definition) {
            Ok(plan) => plan,
            Err(error) => {
                return Ok(SubmitGraphInterception::Rejected {
                    error: format!("错误：执行图登记失败：{error}"),
                })
            }
        };

        // 全局广播：前端图面板据此加载/刷新待确认计划。
        if let Some(app_handle) = &self.app_handle {
            let _ = app_handle.emit(
                "graph-plan-updated",
                GraphPlanUpdatedPayload {
                    plan_id: plan.id.clone(),
                    workspace_id: workspace_id.to_string(),
                },
            );
        }

        let node_count = definition.nodes.len();
        let display_text = format!(
            "执行图《{}》已生成并登记为待确认计划（plan_id={}，{} 个节点）。\n编排思路：{}",
            plan.title,
            plan.id,
            node_count,
            if plan.summary.trim().is_empty() {
                "（未填写）"
            } else {
                plan.summary.trim()
            }
        );

        Ok(SubmitGraphInterception::Submitted {
            display_text,
            action: LoopProtocolAction::GraphSubmitted {
                title: plan.title,
                node_count,
            },
        })
    }

    /// 根据 execute_tool_calls 的结果决定本轮循环是否结束：
    ///   - 出现可重试工具错误 ⇒ 不收口，让模型再修正一轮。
    ///   - 有协议动作（图已提交）⇒ 输出「图已生成，等待确认」消息并收口。
    ///   - 有 final_message ⇒ 输出最终答复并收口。
    ///   - 以上都不是 ⇒ 返回 None，循环继续。
    pub(super) async fn resolve_loop_outcome(
        &self,
        db: &DispatcherDb,
        workspace_id: &str,
        on_event: &Channel<AgentEvent>,
        outcome: RunLoopToolOutcome,
        usage_tracker: &crate::agent::common::UsageTracker,
    ) -> Result<Option<DispatcherMessageRecord>> {
        if outcome.saw_retryable_tool_error {
            return Ok(None);
        }

        if !outcome.protocol_actions.is_empty() {
            let mut sections = Vec::new();
            for action in &outcome.protocol_actions {
                match action {
                    LoopProtocolAction::GraphSubmitted {
                        title, node_count, ..
                    } => sections.push(format!(
                        "🗺️ 执行图《{title}》已生成并通过校验（{node_count} 个节点）。\n\n请在图面板中检查节点设计与任务指令，确认后开始执行。"
                    )),
                }
            }
            if let Some(message) = outcome
                .final_message
                .as_deref()
                .map(str::trim)
                .filter(|message| !message.is_empty())
            {
                sections.push(format!("补充说明：\n{message}"));
            }

            let usage_stats = usage_tracker.snapshot();
            let closing_msg = db
                .add_visible_message_with_usage_async(
                    workspace_id,
                    "assistant",
                    &sections.join("\n\n"),
                    &usage_stats,
                )
                .await?;
            emit(
                on_event,
                AgentEvent::AssistantMessage {
                    message: closing_msg.clone(),
                },
            );
            return Ok(Some(closing_msg));
        }

        if let Some(final_message) = outcome.final_message {
            let usage_stats = usage_tracker.snapshot();
            let reply = db
                .add_visible_message_with_usage_async(
                    workspace_id,
                    "assistant",
                    &final_message,
                    &usage_stats,
                )
                .await?;
            emit(
                on_event,
                AgentEvent::AssistantMessage {
                    message: reply.clone(),
                },
            );
            return Ok(Some(reply));
        }

        Ok(None)
    }
}

/// 解析 submit_graph 的图定义参数：兼容对象与 JSON 字符串两种形态。
fn parse_graph_definition(arguments: &serde_json::Value) -> Result<GraphDefinition, String> {
    let Some(definition_value) = arguments.get("definition") else {
        return Err("错误：submit_graph 缺少 definition 参数".to_string());
    };

    let parsed = match definition_value {
        serde_json::Value::String(raw) => serde_json::from_str::<GraphDefinition>(raw)
            .map_err(|error| format!("错误：definition 不是合法的图定义 JSON：{error}")),
        value => serde_json::from_value::<GraphDefinition>(value.clone())
            .map_err(|error| format!("错误：definition 结构不合法：{error}")),
    }?;
    Ok(parsed)
}
