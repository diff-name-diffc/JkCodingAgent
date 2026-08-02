//! `submit_graph`：编排器收口工具（协议壳）。
//!
//! 与旧 dispatch 协议工具同构：工具 `execute` 只回显，真正的动作
//! （校验 → 落 graph_plans → 广播 graph-plan-updated → 收口本轮）
//! 由 OrchestratorAgent 在 `execute_loop_tool_calls` 中按工具名拦截完成。
//! 因此该工具只注册进编排器专用注册表（`orchestrator_tools`），
//! 不进 `builtin_tools`，避免出现在普通工具目录与设置页中。

use async_trait::async_trait;
use serde_json::{json, Value};

use super::super::context::ToolContext;
use super::super::registry::AgentTool;

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
                    "description": "执行图定义。边由节点的 dependsOn 派生（隐式 DAG）；节点完成后 state[outputKey]=节点输出，下游节点通过 dependsOn 接收上游输出全文、通过 injectStateKeys 接收指定 state 节选。",
                    "properties": {
                        "title": { "type": "string", "description": "图标题，一句话概括任务目标" },
                        "summary": { "type": "string", "description": "编排思路摘要（为什么这样拆）" },
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
                            "description": "执行节点列表（≤ 20 个）；无依赖关系的节点会并行执行",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string", "description": "节点唯一 id（如 n1、n2）" },
                                    "title": { "type": "string", "description": "节点标题" },
                                    "role": { "type": "string", "description": "该节点 Agent 的角色定位，会注入节点输入" },
                                    "agent": {
                                        "type": "object",
                                        "description": "执行者：subAgent=已启用的子智能体（需 agentId）；claude/codex=本机 CLI Agent",
                                        "properties": {
                                            "kind": { "type": "string", "enum": ["subAgent", "claude", "codex"] },
                                            "agentId": { "type": "string", "description": "kind=subAgent 时必填，必须是当前已启用的子智能体 id" }
                                        },
                                        "required": ["kind"]
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
                                        "description": "需要注入的共享 state key（必须是 stateKeys 声明的键或某个节点的 outputKey）"
                                    },
                                    "outputKey": { "type": "string", "description": "本节点输出写回 state 的 key，全局唯一" }
                                },
                                "required": ["id", "title", "agent", "task", "outputKey"]
                            }
                        }
                    },
                    "required": ["title", "nodes"]
                }
            },
            "required": ["definition"]
        })
    }

    async fn execute(&self, _args: &Value, _context: &ToolContext) -> String {
        // 协议壳：真正的校验/落库/广播由编排器拦截完成，这里只回显。
        "已收到执行图提交，系统正在校验与登记…".to_string()
    }
}
