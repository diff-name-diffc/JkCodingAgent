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
use super::super::ToolResult;
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
        submit_graph_parameters_schema()
    }

    async fn execute(&self, _args: &Value, _context: &ToolContext) -> ToolResult {
        // fail-closed：真正的校验/落库/广播由编排器拦截完成。若未走拦截而直接
        // 进入工具 execute（如误注册/重构删除拦截），不允许返回假成功回执，
        // 立即以「错误：」暴露误用。
        ToolResult::recoverable_error(
            "错误：submit_graph 仅支持在编排器拦截环境下运行，当前上下文不可用。",
        )
    }
}

fn bounded_identifier(description: &str) -> Value {
    json!({
        "type": "string",
        "pattern": "^[A-Za-z][A-Za-z0-9_-]{0,63}$",
        "description": description,
    })
}

fn graph_node_schema() -> Value {
    let special_tool = json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "source": { "type": "string", "enum": ["aha"] },
            "name": { "type": "string", "minLength": 1, "maxLength": 256 },
        },
        "required": ["source", "name"],
    });
    let special_tools = json!({
        "type": "array",
        "maxItems": 16,
        "uniqueItems": true,
        "description": "按需启用的 Aha/MCP 宿主工具（统一以 source=\"aha\" 提交并经 CapabilityBroker 执行）",
        "items": special_tool,
    });
    let depends_on = json!({
        "type": "array",
        "maxItems": 20,
        "uniqueItems": true,
        "items": bounded_identifier("上游节点 id"),
        "description": "上游节点 id 列表；多上游 => 接收多个上游输出；必须构成无环图",
    });
    let inject_state_keys = json!({
        "type": "array",
        "maxItems": 64,
        "uniqueItems": true,
        "items": bounded_identifier("共享 state key"),
        "description": "需要注入的共享 state key；生产者必须是本节点的上游",
    });
    let expected_files = json!({
        "type": "array",
        "maxItems": 256,
        "uniqueItems": true,
        "items": { "type": "string", "minLength": 1, "maxLength": 4096 },
        "description": "预期读写的相对工作区路径，供并行写冲突预检",
    });

    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "id": bounded_identifier("节点唯一 id（如 n1、n2）"),
            "title": { "type": "string", "minLength": 1, "maxLength": 200 },
            "role": { "type": "string", "maxLength": 1000 },
            "modelRef": { "type": "string", "minLength": 1, "maxLength": 256 },
            "baseToolGroup": { "type": "string", "enum": ["read_only", "coding"] },
            "specialTools": special_tools,
            "task": { "type": "string", "minLength": 1, "maxLength": 32000 },
            "dependsOn": depends_on,
            "injectStateKeys": inject_state_keys,
            "outputKey": bounded_identifier("本节点输出写回 state 的唯一 key"),
            "expectedFiles": expected_files,
            "exportPolicy": { "type": "string", "enum": ["summary", "full"] },
        },
        "required": ["id", "title", "modelRef", "baseToolGroup", "task", "outputKey"],
    })
}

fn graph_definition_schema() -> Value {
    let inherits_from = json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "planId": { "type": "string", "minLength": 1, "maxLength": 128 },
            "runId": { "type": "string", "minLength": 1, "maxLength": 128 },
        },
        "required": ["planId", "runId"],
    });
    let state_key = json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "key": bounded_identifier("稳定 state key"),
            "description": { "type": "string", "maxLength": 1000 },
        },
        "required": ["key"],
    });

    json!({
        "type": "object",
        "additionalProperties": false,
        "description": "执行图定义；边由节点 dependsOn 派生，节点输出按 outputKey 写回共享 state。",
        "properties": {
            "version": { "type": "integer", "enum": [GRAPH_DEFINITION_VERSION] },
            "title": { "type": "string", "minLength": 1, "maxLength": 200 },
            "summary": { "type": "string", "maxLength": 2000 },
            "inheritsFrom": inherits_from,
            "stateKeys": { "type": "array", "maxItems": 64, "items": state_key },
            "nodes": {
                "type": "array",
                "minItems": 1,
                "maxItems": 20,
                "items": graph_node_schema(),
            },
        },
        "required": ["version", "title", "nodes"],
    })
}

fn submit_graph_parameters_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": { "definition": graph_definition_schema() },
        "required": ["definition"],
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{AgentTool, SubmitGraphTool};
    use crate::agent::graph::types::GRAPH_DEFINITION_VERSION;

    fn minimal_definition() -> serde_json::Value {
        json!({
            "definition": {
                "version": GRAPH_DEFINITION_VERSION,
                "title": "实现运行时工具",
                "nodes": [{
                    "id": "n1",
                    "title": "实现",
                    "modelRef": "model-1",
                    "baseToolGroup": "read_only",
                    "task": "检查实现",
                    "outputKey": "result",
                }],
            },
        })
    }

    #[test]
    fn schema_is_strict_and_rejects_disabled_pi_extensions() {
        let validator = jsonschema::draft202012::new(&SubmitGraphTool.parameters())
            .expect("submit_graph schema");
        assert!(validator.is_valid(&minimal_definition()));

        let mut unknown_field = minimal_definition();
        unknown_field["definition"]["unknown"] = json!(true);
        assert!(!validator.is_valid(&unknown_field));

        let mut pi_extension = minimal_definition();
        pi_extension["definition"]["nodes"][0]["specialTools"] =
            json!([{ "source": "pi_extension", "name": "unsafe" }]);
        assert!(!validator.is_valid(&pi_extension));
    }

    #[test]
    fn schema_enforces_graph_size_fields() {
        let validator = jsonschema::draft202012::new(&SubmitGraphTool.parameters())
            .expect("submit_graph schema");
        let mut oversized = minimal_definition();
        oversized["definition"]["title"] = json!("x".repeat(201));
        oversized["definition"]["nodes"][0]["task"] = json!("x".repeat(32_001));

        assert!(!validator.is_valid(&oversized));
    }
}
