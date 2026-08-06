//! `submit_graph` 协议拦截与本轮收口。
//!
//! 壳工具只回显；这里完成真正的动作：解析图定义 → 解析 inheritsFrom（修复图
//! 继承来源 plan 最近一次运行结束时的共享 state 并种入新 plan）→ `graph/validate`
//! 结构+语义校验 → 落 `graph_plans`（携带需求快照）→ 广播 `graph-plan-updated`。
//! 校验失败按可重试工具错误交回模型自修复；成功则由 `resolve_loop_outcome`
//! 以「图已生成，等待确认」收口。

use std::collections::HashSet;

use anyhow::Result;
use serde_json::{Map, Value};
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

/// inheritsFrom 解析结果：继承的 state 快照（种入新 plan）与其键集合（校验用）。
struct InheritedState {
    state_json: String,
    seeded_keys: HashSet<String>,
}

impl OrchestratorAgent {
    /// submit_graph 协议拦截：解析图定义 → 解析继承 → 校验 → 落 graph_plans → 广播。
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

        let store = GraphStore::new(db);

        // 修复图继承：校验引用合法性，并把被继承 run 的 state 快照种入新 plan。
        let inherited = match resolve_inheritance(&store, workspace_id, &definition).await {
            Ok(inherited) => inherited,
            Err(error) => return Ok(SubmitGraphInterception::Rejected { error }),
        };
        // 按值解构转移所有权，避免为保住 is_some 判断而克隆整个 HashSet
        // 与 state 字符串。
        let (initial_state_json, seeded_keys, has_inherited) = match inherited {
            Some(inherited) => (inherited.state_json, inherited.seeded_keys, true),
            None => ("{}".to_string(), HashSet::new(), false),
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
        if let Err(error) = validate_graph(&definition, &catalog, &seeded_keys) {
            return Ok(SubmitGraphInterception::Rejected { error });
        }

        // 需求快照：以提交时刻的最新用户消息为准，运行与验收都以此为目标。
        // 读取失败或读不到可见用户消息（None/空白）都不能静默退化为空串：
        // 空快照会让运行与验收失去锚点，交回模型/用户明确的失败原因优于带病登记。
        let requirement = match db.get_latest_user_message_content_async(workspace_id).await {
            Ok(Some(content)) if !content.trim().is_empty() => content,
            Ok(_) => {
                return Ok(SubmitGraphInterception::Rejected {
                    error: "错误：会话中没有可读的用户需求消息，运行与验收将失去目标，无法登记执行图；请先向会话发送需求描述后再提交。".to_string(),
                })
            }
            Err(error) => {
                return Ok(SubmitGraphInterception::Rejected {
                    // {error:#} 保留完整错误链（{error} 只打印最外层 context）。
                    error: format!("错误：读取用户需求快照失败，无法登记执行图：{error:#}"),
                })
            }
        };
        let plan = match store
            .create_plan_async(
                workspace_id,
                &definition,
                &requirement,
                &initial_state_json,
            )
            .await
        {
            Ok(plan) => plan,
            Err(error) => {
                return Ok(SubmitGraphInterception::Rejected {
                    error: format!("错误：执行图登记失败：{error:#}"),
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
        let inherited_note = if has_inherited {
            "（修复图：已继承上次运行的共享 state）"
        } else {
            ""
        };
        let display_text = format!(
            "执行图《{}》已生成并登记为待确认计划（plan_id={}，{} 个节点）{}。\n编排思路：{}",
            plan.title,
            plan.id,
            node_count,
            inherited_note,
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
                    LoopProtocolAction::GraphSubmitted { title, node_count } => sections.push(format!(
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

/// 解析并校验 inheritsFrom：被继承的 plan/run 必须存在、同会话、run 已终态，
/// 且 run 必须是该 plan 的最近一次运行。成功时返回种入新 plan 的 state 快照
/// （提交时该 plan 的共享 state）与键集合（供校验 injectStateKeys）。
///
/// 并发说明：此处 get_plan 与后续 create_plan 不在同一事务，其间源 plan 若
/// 启动新运行，latest_run_id/state_json 会变化。该竞态无害，不必事务化收敛：
/// latest_run_id 与 state_json 位于同一行、读取原子，拿到的快照恒对应读取
/// 时刻被校验的运行（state_json 只在新运行 create_run 时被重置）；最坏情况
/// 是新 plan 登记「读取时最近运行」的结束态，与 inheritsFrom 的语义一致。
async fn resolve_inheritance(
    store: &GraphStore,
    workspace_id: &str,
    definition: &GraphDefinition,
) -> Result<Option<InheritedState>, String> {
    let Some(inherits) = &definition.inherits_from else {
        return Ok(None);
    };
    let source_plan = store
        .get_plan_async(&inherits.plan_id)
        .await
        // {error:#} 保留完整错误链（{error} 只打印最外层 context，丢失根因）。
        .map_err(|error| format!("错误：读取被继承的图计划失败：{error:#}"))?
        .ok_or_else(|| {
            format!(
                "错误：inheritsFrom 引用的图计划 '{}' 不存在",
                inherits.plan_id
            )
        })?;
    if source_plan.workspace_id != workspace_id {
        return Err("错误：inheritsFrom 只能继承当前会话内的图计划".to_string());
    }
    let source_run = source_plan
        .runs
        .iter()
        .find(|run| run.id == inherits.run_id)
        .ok_or_else(|| {
            format!(
                "错误：inheritsFrom 引用的运行 '{}' 不属于图计划 '{}'",
                inherits.run_id, inherits.plan_id
            )
        })?;
    if matches!(source_run.status.as_str(), "running") {
        return Err("错误：不能继承仍在运行中的图运行，请等待其结束".to_string());
    }
    // plan.state_json 是跨 run 持续累积的计划级 state，并非某次 run 的快照：
    // 只允许继承最近一次运行，确保种入新图的是该 plan 最新运行结束时的 state，
    // 避免引用旧 run 时被更晚 run 写入的 state 污染。
    if source_plan.latest_run_id.as_deref() != Some(inherits.run_id.as_str()) {
        return Err(format!(
            "错误：inheritsFrom 只能继承图计划 '{}' 的最近一次运行，'{}' 不是最近一次运行",
            inherits.plan_id, inherits.run_id
        ));
    }
    // 解析失败必须拒绝提交：损坏的 state_json 若被静默当成空 Map，
    // 校验会误拒合法的 injectStateKeys，且损坏内容还会原样种入新 plan。
    let state: Map<String, Value> = serde_json::from_str(&source_plan.state_json).map_err(
        |error| {
            format!(
                "错误：图计划 '{}' 的共享 state 已损坏（JSON 解析失败：{error}），无法继承",
                inherits.plan_id
            )
        },
    )?;
    let seeded_keys = state.keys().cloned().collect::<HashSet<_>>();
    Ok(Some(InheritedState {
        state_json: Value::Object(state).to_string(),
        seeded_keys,
    }))
}

/// 解析 submit_graph 的图定义参数：兼容对象与 JSON 字符串两种形态。
/// 解析成功后统一 trim 节点 id / 依赖引用等标识符，保证落库的定义
/// 与运行期（调度器/持久化均以 trim 后 id 为准）完全一致。
fn parse_graph_definition(arguments: &serde_json::Value) -> Result<GraphDefinition, String> {
    let Some(definition_value) = arguments.get("definition") else {
        return Err("错误：submit_graph 缺少 definition 参数".to_string());
    };

    let mut parsed = match definition_value {
        serde_json::Value::String(raw) => serde_json::from_str::<GraphDefinition>(raw)
            .map_err(|error| format!("错误：definition 不是合法的图定义 JSON：{error}")),
        value => serde_json::from_value::<GraphDefinition>(value.clone())
            .map_err(|error| format!("错误：definition 结构不合法：{error}")),
    }?;
    parsed.normalize_ids();
    Ok(parsed)
}
