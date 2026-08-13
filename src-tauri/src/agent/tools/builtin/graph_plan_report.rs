//! `graph_plan_report`：编排器读取执行图运行报告的协议壳工具。
//!
//! 与 `submit_graph` 同构：真正的动作（读取最新 plan 的最近 run：验收结论、
//! 各节点状态/输出摘要/错误）由 OrchestratorAgent 在 `execute_loop_tool_calls`
//! 中按工具名拦截完成。拦截结果作为普通工具输出返回给模型（不收口本轮），
//! 支撑「失败 → 读报告 → 提交修复图」的反思闭环。工具自身的 `execute` 为
//! fail-closed 兜底：未经拦截直接调用时返回「错误：」而非假成功回执。
//! 只注册进编排器专用注册表，不进通用 `builtin_tools`。
//!
//! 文件名与工具名/构造函数对齐（graph_plan_report），区别于
//! `agents/project/graph_report.rs`（协议拦截实现）。

use async_trait::async_trait;
use serde_json::{json, Value};

use super::super::context::ToolContext;
use super::super::registry::AgentTool;

pub(super) fn graph_plan_report_tool() -> Box<dyn AgentTool> {
    Box::new(GraphPlanReportTool)
}

struct GraphPlanReportTool;

#[async_trait]
impl AgentTool for GraphPlanReportTool {
    fn name(&self) -> &'static str {
        "graph_plan_report"
    }

    fn description(&self) -> &'static str {
        "读取当前会话最近一次执行图的运行报告：验收结论、各节点状态、节点输出摘要与失败原因。上次执行图失败或完成后，先用它了解执行情况，再决定答复用户或提交 inheritsFrom 修复图。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "planId": {
                    "type": "string",
                    "description": "可选：指定图计划 id；缺省取会话最近的图计划"
                }
            }
        })
    }

    async fn execute(&self, _args: &Value, _context: &ToolContext) -> String {
        // fail-closed：真正的报告生成由编排器拦截完成。若未走拦截而直接进入工具
        // execute（如误注册/重构删除拦截），不允许返回假成功回执，立即以「错误：」
        // 暴露误用。
        "错误：graph_plan_report 仅支持在编排器拦截环境下运行，当前上下文不可用。".to_string()
    }
}
