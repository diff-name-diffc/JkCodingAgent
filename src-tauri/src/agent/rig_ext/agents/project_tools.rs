//! 编排器协议工具（rig 壳工具）：`submit_graph` / `graph_plan_report` / `message`。
//!
//! 三者的定义（name/description/parameters）逐字迁移自旧
//! `tools/builtin/{submit_graph,graph_plan_report,shell}.rs`；执行回调为
//! fail-closed 兜底——真正的动作由 `RigOrchestratorProtocol`（宿主拦截）完成，
//! 若因接线错误走到回调，以「错误：」暴露误用而不是返回假成功回执。

use rig::tool::{PortableDynamicTool, ToolExecutionError};
use serde_json::{json, Value};

use crate::agent::graph::types::GRAPH_DEFINITION_VERSION;

/// `submit_graph`：提交执行图（编排器收口工具）。
pub(crate) fn submit_graph_shell() -> PortableDynamicTool {
    PortableDynamicTool::new(
        "submit_graph",
        "提交任务执行图（DAG），这是复杂任务的收口方式。调用前必须已完成需求理解与必要的只读探索；提交后系统会校验图定义并登记为待确认计划，等待用户确认后由图运行器执行。每轮最多提交一次。",
        submit_graph_parameters_schema(),
        |_args| {
            Box::pin(async move {
                Err(ToolExecutionError::refused(
                    "错误：submit_graph 仅支持在编排器拦截环境下运行，当前上下文不可用。",
                ))
            })
        },
    )
}

/// `graph_plan_report`：读取最近一次执行图运行报告（反思闭环，不收口）。
pub(crate) fn graph_plan_report_shell() -> PortableDynamicTool {
    PortableDynamicTool::new(
        "graph_plan_report",
        "读取当前会话最近一次执行图的运行报告：验收结论、各节点状态、节点输出摘要与失败原因。上次执行图失败或完成后，先用它了解执行情况，再决定答复用户或提交 inheritsFrom 修复图。",
        json!({
            "type": "object",
            "properties": {
                "planId": {
                    "type": "string",
                    "description": "可选：指定图计划 id；缺省取会话最近的图计划"
                }
            }
        }),
        |_args| {
            Box::pin(async move {
                Err(ToolExecutionError::refused(
                    "错误：graph_plan_report 仅支持在编排器拦截环境下运行，当前上下文不可用。",
                ))
            })
        },
    )
}

/// `message`：给用户的最终答复（编排器收口工具）。
pub(crate) fn message_shell() -> PortableDynamicTool {
    PortableDynamicTool::new(
        "message",
        "向用户发送本轮任务的最终答复。当任务不需要执行图（简单问答、信息查询、澄清说明）时，用它收口；需要执行图时用 submit_graph。",
        json!({
            "type": "object",
            "properties": {
                "content": { "type": "string", "description": "要发送给用户的内容" }
            },
            "required": ["content"]
        }),
        |_args| {
            Box::pin(async move {
                Err(ToolExecutionError::refused(
                    "错误：message 仅支持在编排器拦截环境下运行，当前上下文不可用。",
                ))
            })
        },
    )
}

/// 编排器可见的工具名（固定集合，模型只看这四个入口）。
pub(crate) const ORCHESTRATOR_PROTOCOL_TOOL_NAMES: [&str; 4] =
    ["run_tool_program", "message", "submit_graph", "graph_plan_report"];

fn bounded_identifier(description: &str) -> Value {
    json!({
        "type": "string",
        "pattern": "^[A-Za-z][A-Za-z0-9_-]{0,63}$",
        "description": description,
    })
}

fn graph_node_schema() -> Value {
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
    use super::*;

    fn minimal_definition() -> Value {
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
    fn schema_is_strict_and_rejects_removed_or_unknown_fields() {
        let validator =
            jsonschema::draft202012::new(&submit_graph_parameters_schema()).expect("schema");
        assert!(validator.is_valid(&minimal_definition()));

        let mut unknown_field = minimal_definition();
        unknown_field["definition"]["unknown"] = json!(true);
        assert!(!validator.is_valid(&unknown_field));

        // v4 已删除 specialTools：作为未知字段必须被 additionalProperties=false 拒绝。
        let mut legacy = minimal_definition();
        legacy["definition"]["nodes"][0]["specialTools"] =
            json!([{ "source": "aha", "name": "exec" }]);
        assert!(!validator.is_valid(&legacy));
    }

    #[test]
    fn schema_enforces_graph_size_fields() {
        let validator =
            jsonschema::draft202012::new(&submit_graph_parameters_schema()).expect("schema");
        let mut oversized = minimal_definition();
        oversized["definition"]["title"] = json!("x".repeat(201));
        oversized["definition"]["nodes"][0]["task"] = json!("x".repeat(32_001));
        assert!(!validator.is_valid(&oversized));
    }

    #[tokio::test]
    async fn shells_are_fail_closed_without_host_interception() {
        for tool in [submit_graph_shell(), graph_plan_report_shell(), message_shell()] {
            let error = tool
                .execute(json!({}))
                .await
                .expect_err("壳工具不得在无拦截时返回成功");
            assert!(
                error.model_feedback().unwrap_or_default().contains("错误："),
                "壳工具错误必须带「错误：」前缀"
            );
        }
    }

    #[test]
    fn orchestrator_tool_names_cover_all_shells() {
        let names = ORCHESTRATOR_PROTOCOL_TOOL_NAMES;
        assert!(names.contains(&"submit_graph"));
        assert!(names.contains(&"graph_plan_report"));
        assert!(names.contains(&"message"));
        assert!(names.contains(&"run_tool_program"));
    }

    #[test]
    fn shell_descriptions_match_model_contract() {
        assert!(message_shell().definition().description.contains("最终答复"));
        assert!(submit_graph_shell()
            .definition()
            .description
            .contains("每轮最多提交一次"));
    }
}
