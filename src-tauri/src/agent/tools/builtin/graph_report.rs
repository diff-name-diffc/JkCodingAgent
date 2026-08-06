//! `graph_plan_report`：编排器读取执行图运行报告的协议壳工具。
//!
//! 与 `submit_graph` 同构：工具 `execute` 只回显，真正的动作（读取最新 plan 的
//! 最近 run：验收结论、各节点状态/输出摘要/错误）由 OrchestratorAgent 在
//! `execute_loop_tool_calls` 中按工具名拦截完成。拦截结果作为普通工具输出
//! 返回给模型（不收口本轮），支撑「失败 → 读报告 → 提交修复图」的反思闭环。
//! 只注册进编排器专用注册表，不进通用 `builtin_tools`。

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
        // 协议壳：真正的报告生成由编排器拦截完成，这里只回显。
        "已收到执行图报告请求，系统正在读取运行记录…".to_string()
    }
}
