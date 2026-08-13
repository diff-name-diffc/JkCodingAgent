//! `submit_graph`：编排器收口工具（协议壳）。
//!
//! 真正的动作（校验 → 落 graph_plans → 广播 graph-plan-updated → 收口本轮）
//! 由 OrchestratorAgent 在 `execute_loop_tool_calls` 中按工具名拦截完成。
//! 工具自身的 `execute` 为 fail-closed 兜底：未经拦截直接调用时返回「错误：」
//! 而非假成功回执。因此该工具只注册进编排器专用注册表（`orchestrator_tools`），
//! 不进 `builtin_tools`，避免出现在普通工具目录与设置页中。

use async_trait::async_trait;
use serde_json::{json, Value};

use super::super::context::ToolContext;
use super::super::registry::AgentTool;
use crate::agent::graph::types::GRAPH_DEFINITION_VERSION;

pub(super) fn submit_graph_tool() -> Box<dyn AgentTool> {
    Box::new(SubmitGraphTool)
}

struct SubmitGraphTool;

#[async_trait]
impl AgentTool for SubmitGraphTool {
    fn name(&self) -> &'static str {
        "submit_graph"
    }

    fn description(&self) -> &'static str {
        "提交任务执行图（DAG），这是复杂任务的收口方式。调用前必须已完成需求理解与必要的只读探索；提交后系统会校验图定义并登记为待确认计划，等待用户确认后由图运行器执行。每轮最多提交一次。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "definition": {
                    "type": "object",
                    "description": "执行图定义。边由节点的 dependsOn 派生（隐式 DAG）；节点完成后 state[outputKey]=节点产出摘要，下游节点通过 dependsOn 接收上游输出（摘要或全文见 exportPolicy）、通过 injectStateKeys 接收指定 state 值。",
                    "properties": {
                        "version": { "type": "integer", "enum": [GRAPH_DEFINITION_VERSION], "description": "契约版本，固定为当前版本" },
                        "title": { "type": "string", "description": "图标题，一句话概括任务目标" },
                        "summary": { "type": "string", "description": "编排思路摘要（为什么这样拆）" },
                        "inheritsFrom": {
                            "type": "object",
                            "description": "修复图继承来源（可选）：从本会话内既有计划的最近一次运行继承共享 state，避免重做已成功的部分；需与 injectStateKeys 配合引用继承的键",
                            "properties": {
                                "planId": { "type": "string", "description": "被继承的图计划 id" },
                                "runId": { "type": "string", "description": "被继承的运行 id（必须是该计划最近一次且已终态的运行）" }
                            },
                            "required": ["planId", "runId"]
                        },
                        "stateKeys": {
                            "type": "array",
                            "description": "共享 state 的键声明（可选）；节点的 outputKey 会自动成为可用键，无需重复声明",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "key": { "type": "string", "description": "snake_case 键名" },
                                    "description": { "type": "string", "description": "该键承载的内容说明" }
                                },
                                "required": ["key"]
                            }
                        },
                        "nodes": {
                            "type": "array",
                            "minItems": 1,
                            "maxItems": 20,
                            "description": "执行节点列表（1–20 个）；无依赖关系的节点会并行执行",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string", "description": "节点唯一 id（如 n1、n2）" },
                                    "title": { "type": "string", "description": "节点标题" },
                                    "role": { "type": "string", "description": "该节点 Agent 的角色定位，会注入节点输入" },
                                    "modelRef": { "type": "string", "description": "PI Harness 模型目录中的稳定模型 id" },
                                    "baseToolGroup": { "type": "string", "enum": ["read_only", "coding"], "description": "PI 基础工具组" },
                                    "specialTools": {
                                        "type": "array",
                                        "description": "按需启用的 PI 扩展或 Aha/MCP 工具（MCP 工具与 Aha 工具统一以 source=\"aha\" 提交）",
                                        "items": {
                                            "type": "object",
                                            "properties": {
                                                "source": { "type": "string", "enum": ["pi_extension", "aha"] },
                                                "name": { "type": "string" }
                                            },
                                            "required": ["source", "name"]
                                        }
                                    },
                                    "task": { "type": "string", "description": "自包含的子任务说明：目标、背景、相关文件/符号、约束、验证方式、期望产出。节点看不到聊天记录，必须自包含。" },
                                    "dependsOn": {
                                        "type": "array",
                                        "items": { "type": "string" },
                                        "description": "上游节点 id 列表；多上游 => 接收多个上游输出；必须构成无环图"
                                    },
                                    "injectStateKeys": {
                                        "type": "array",
                                        "items": { "type": "string" },
                                        "description": "需要注入的共享 state key（值为对应节点的产出摘要；必须是 stateKeys 声明的键、某个节点的 outputKey 或继承 state 的键，且生产者必须是本节点的上游）"
                                    },
                                    "outputKey": { "type": "string", "description": "本节点输出写回 state 的 key，全局唯一" },
                                    "expectedFiles": {
                                        "type": "array",
                                        "items": { "type": "string" },
                                        "description": "预期读写的文件（相对工作区路径）；供并行写冲突预检，可能并行的两个 coding 节点文件相交会被拒绝。可选，未填不检测"
                                    },
                                    "exportPolicy": { "type": "string", "enum": ["summary", "full"], "description": "输出对下游的导出策略：summary（默认，下游只见产出摘要）/ full（下游可见完整输出）" }
                                },
                                "required": ["id", "title", "modelRef", "baseToolGroup", "task", "outputKey"]
                            }
                        }
                    },
                    "required": ["version", "title", "nodes"]
                }
            },
            "required": ["definition"]
        })
    }

    async fn execute(&self, _args: &Value, _context: &ToolContext) -> String {
        // fail-closed：真正的校验/落库/广播由编排器拦截完成。若未走拦截而直接
        // 进入工具 execute（如误注册/重构删除拦截），不允许返回假成功回执，
        // 立即以「错误：」暴露误用。
        "错误：submit_graph 仅支持在编排器拦截环境下运行，当前上下文不可用。".to_string()
    }
}
