//! 编排器协议工具（rig 壳工具）：`submit_workflow` / `workflow_plan_report` / `message`。
//!
//! 三者的定义（name/description/parameters）逐字迁移自旧
//! 旧自实现工具层（已随迁移删除）的 submit_workflow/workflow_plan_report/shell；执行回调为
//! fail-closed 兜底——真正的动作由 `RigOrchestratorProtocol`（宿主拦截）完成，
//! 若因接线错误走到回调，以「错误：」暴露误用而不是返回假成功回执。

use rig::tool::{PortableDynamicTool, ToolExecutionError};
use serde_json::{json, Value};

use crate::agent::workflow::types::WORKFLOW_DEFINITION_VERSION;

/// `submit_workflow`：提交工作流（编排器收口工具）。
pub(crate) fn submit_workflow_shell() -> PortableDynamicTool {
    PortableDynamicTool::new(
        "submit_workflow",
        "提交任务工作流（DAG），这是复杂任务的收口方式。调用前必须已完成需求理解与必要的只读探索；提交后系统会校验工作流定义并登记为待确认计划，等待用户确认后由工作流运行器执行。每轮最多提交一次。",
        submit_workflow_parameters_schema(),
        |_args| {
            Box::pin(async move {
                Err(ToolExecutionError::refused(
                    "错误：submit_workflow 仅支持在编排器拦截环境下运行，当前上下文不可用。",
                ))
            })
        },
    )
}

/// `workflow_plan_report`：读取最近一次工作流运行报告（反思闭环，不收口）。
pub(crate) fn workflow_plan_report_shell() -> PortableDynamicTool {
    PortableDynamicTool::new(
        "workflow_plan_report",
        "读取当前会话最近一次工作流的运行报告：验收结论、各节点状态、节点输出摘要与失败原因。上次工作流失败或完成后，先用它了解执行情况，再决定答复用户或提交 inheritsFrom 修复工作流。",
        json!({
            "type": "object",
            "properties": {
                "planId": {
                    "type": "string",
                    "description": "可选：指定工作流计划 id；缺省取会话最近的工作流计划"
                }
            }
        }),
        |_args| {
            Box::pin(async move {
                Err(ToolExecutionError::refused(
                    "错误：workflow_plan_report 仅支持在编排器拦截环境下运行，当前上下文不可用。",
                ))
            })
        },
    )
}

/// `message`：给用户的最终答复（编排器收口工具）。
pub(crate) fn message_shell() -> PortableDynamicTool {
    PortableDynamicTool::new(
        "message",
        "向用户发送本轮任务的最终答复。当任务不需要工作流（简单问答、信息查询、澄清说明）时，用它收口；需要工作流时用 submit_workflow。",
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

/// `workflow_result_read`：读取工作流执行结果（结构化结果的感知工具，不收口）。
pub(crate) fn workflow_result_read_shell() -> PortableDynamicTool {
    PortableDynamicTool::new(
        "workflow_result_read",
        "读取当前会话最近一次工作流的执行结果：结果类型（审查报告/执行结果）、验收结论、完整结论文本（审查类=问题清单，编辑类=执行总结）与修改文件清单。审查完成后要用结论规划修复/后续工作流，或需要向用户复述结果详情时，先用它拿到完整结论，再决定下一步。",
        json!({
            "type": "object",
            "properties": {
                "planId": {
                    "type": "string",
                    "description": "可选：指定工作流计划 id；缺省取会话最近的工作流计划"
                }
            }
        }),
        |_args| {
            Box::pin(async move {
                Err(ToolExecutionError::refused(
                    "错误：workflow_result_read 仅支持在编排器拦截环境下运行，当前上下文不可用。",
                ))
            })
        },
    )
}

/// `workflow_get`：读取工作流定义与状态（感知工具，不收口）。
pub(crate) fn workflow_get_shell() -> PortableDynamicTool {
    PortableDynamicTool::new(
        "workflow_get",
        "读取当前会话工作流的完整定义与状态：节点任务、依赖、共享 state 键、最近运行摘要。上下文过长或被压缩后工作流细节可能丢失，需要时用它重新感知最新工作流信息，再决定答复、读报告或修复。",
        json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "planId": {
                    "type": "string",
                    "description": "可选：指定工作流计划 id；缺省取会话最近的工作流计划"
                }
            }
        }),
        |_args| {
            Box::pin(async move {
                Err(ToolExecutionError::refused(
                    "错误：workflow_get 仅支持在编排器拦截环境下运行，当前上下文不可用。",
                ))
            })
        },
    )
}

/// `workflow_node_update`：定点修改待确认工作流的单个节点（局部更新，不收口）。
pub(crate) fn workflow_node_update_shell() -> PortableDynamicTool {
    PortableDynamicTool::new(
        "workflow_node_update",
        "定点修正待确认（draft）工作流中的单个节点：patch 里提供哪个字段就替换哪个，未提供的字段保持不变（含 dependsOn，即改边）。仅 draft 态可用；工作流已开始执行后的修复请用 submit_workflow + inheritsFrom 提交修复工作流。",
        workflow_node_update_parameters_schema(),
        |_args| {
            Box::pin(async move {
                Err(ToolExecutionError::refused(
                    "错误：workflow_node_update 仅支持在编排器拦截环境下运行，当前上下文不可用。",
                ))
            })
        },
    )
}

/// `workflow_node_add`：向待确认工作流新增节点（可原子插入执行边中间）。
pub(crate) fn workflow_node_add_shell() -> PortableDynamicTool {
    PortableDynamicTool::new(
        "workflow_node_add",
        "向待确认（draft）工作流新增一个节点（须给完整节点定义）。insertBefore 可选：把指定节点的、指向本节点上游的依赖边改写为本节点，实现 A→B 中间插入为 A→新节点→B；不传则按 dependsOn 并行/尾部追加。仅 draft 态可用。",
        workflow_node_add_parameters_schema(),
        |_args| {
            Box::pin(async move {
                Err(ToolExecutionError::refused(
                    "错误：workflow_node_add 仅支持在编排器拦截环境下运行，当前上下文不可用。",
                ))
            })
        },
    )
}

/// `workflow_node_delete`：从待确认工作流删除节点（可选级联删除下游）。
pub(crate) fn workflow_node_delete_shell() -> PortableDynamicTool {
    PortableDynamicTool::new(
        "workflow_node_delete",
        "从待确认（draft）工作流删除节点。节点被下游依赖时默认拒绝并列出全部传递下游；确认需要连带清理时带 force=true，将级联删除依赖它的全部下游节点，保证工作流无悬空依赖。仅 draft 态可用。",
        workflow_node_delete_parameters_schema(),
        |_args| {
            Box::pin(async move {
                Err(ToolExecutionError::refused(
                    "错误：workflow_node_delete 仅支持在编排器拦截环境下运行，当前上下文不可用。",
                ))
            })
        },
    )
}

/// 编排器可见的工具名（固定集合，模型只看这九个入口）。
pub(crate) const ORCHESTRATOR_PROTOCOL_TOOL_NAMES: [&str; 9] = [
    "run_tool_program",
    "message",
    "submit_workflow",
    "workflow_plan_report",
    "workflow_result_read",
    "workflow_get",
    "workflow_node_update",
    "workflow_node_add",
    "workflow_node_delete",
];

fn bounded_identifier(description: &str) -> Value {
    json!({
        "type": "string",
        "pattern": "^[A-Za-z][A-Za-z0-9_-]{0,63}$",
        "description": description,
    })
}

fn workflow_node_schema() -> Value {
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
            "usePlanMode": {
                "type": "boolean",
                "description": "true = 以 plan 模式启动（先计划后执行，计划完成后自动批准并切回 bypassPermissions）；默认 false（bypassPermissions 全权限）",
            },
        },
        "required": ["id", "title", "modelRef", "baseToolGroup", "task", "outputKey"],
    })
}

fn workflow_definition_schema() -> Value {
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
        "description": "工作流定义；边由节点 dependsOn 派生，节点输出按 outputKey 写回共享 state。",
        "properties": {
            "version": { "type": "integer", "enum": [WORKFLOW_DEFINITION_VERSION] },
            "title": { "type": "string", "minLength": 1, "maxLength": 200 },
            "summary": { "type": "string", "maxLength": 2000 },
            "inheritsFrom": inherits_from,
            "stateKeys": { "type": "array", "maxItems": 64, "items": state_key },
            "nodes": {
                "type": "array",
                "minItems": 1,
                "maxItems": 20,
                "items": workflow_node_schema(),
            },
        },
        "required": ["version", "title", "nodes"],
    })
}

fn submit_workflow_parameters_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": { "definition": workflow_definition_schema() },
        "required": ["definition"],
    })
}

/// workflow_node_update 的 patch 子 schema：复用节点 schema 的字段约束改为全
/// 可选（minProperties=1 拒绝空 patch）；节点 id 不在 patch 面——id 是依赖
/// 引用锚点，改 id 等同改工作流拓扑，应重提整个工作流。
fn workflow_node_patch_schema() -> Value {
    let mut node = workflow_node_schema();
    let object = node
        .as_object_mut()
        .expect("workflow_node_schema 恒为 object schema");
    // 节点 id 从 properties 中移除（additionalProperties=false 随即拒绝 patch
    // 携带 id）；required 清空 + minProperties=1 拒绝空 patch。
    if let Some(properties) = object.get_mut("properties").and_then(Value::as_object_mut) {
        properties.remove("id");
    }
    object.insert("minProperties".into(), json!(1));
    object.insert("required".into(), json!([]));
    node
}

fn workflow_node_update_parameters_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "planId": {
                "type": "string",
                "description": "可选：指定工作流计划 id；缺省取会话最近的工作流计划"
            },
            "nodeId": bounded_identifier("要更新的节点 id"),
            "patch": workflow_node_patch_schema(),
        },
        "required": ["nodeId", "patch"],
    })
}

fn workflow_node_add_parameters_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "planId": {
                "type": "string",
                "description": "可选：指定工作流计划 id；缺省取会话最近的工作流计划"
            },
            "node": workflow_node_schema(),
            "insertBefore": {
                "type": "array",
                "maxItems": 20,
                "uniqueItems": true,
                "items": bounded_identifier("要改写依赖边的下游节点 id"),
                "description": "可选：把这些节点对本节点上游的依赖边接管为本节点（中间插入）；不传则按 node.dependsOn 直接挂入工作流",
            },
        },
        "required": ["node"],
    })
}

fn workflow_node_delete_parameters_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "planId": {
                "type": "string",
                "description": "可选：指定工作流计划 id；缺省取会话最近的工作流计划"
            },
            "nodeId": bounded_identifier("要删除的节点 id"),
            "force": {
                "type": "boolean",
                "description": "true = 节点被依赖时级联删除全部传递下游；缺省 false（被依赖则拒绝并列出下游）",
            },
        },
        "required": ["nodeId"],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_definition() -> Value {
        json!({
            "definition": {
                "version": WORKFLOW_DEFINITION_VERSION,
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
            jsonschema::draft202012::new(&submit_workflow_parameters_schema()).expect("schema");
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
    fn schema_accepts_optional_use_plan_mode_boolean() {
        let validator =
            jsonschema::draft202012::new(&submit_workflow_parameters_schema()).expect("schema");
        let mut planned = minimal_definition();
        planned["definition"]["nodes"][0]["usePlanMode"] = json!(true);
        assert!(validator.is_valid(&planned));
        // 非布尔值拒绝。
        planned["definition"]["nodes"][0]["usePlanMode"] = json!("yes");
        assert!(!validator.is_valid(&planned));
    }

    #[test]
    fn schema_enforces_workflow_size_fields() {
        let validator =
            jsonschema::draft202012::new(&submit_workflow_parameters_schema()).expect("schema");
        let mut oversized = minimal_definition();
        oversized["definition"]["title"] = json!("x".repeat(201));
        oversized["definition"]["nodes"][0]["task"] = json!("x".repeat(32_001));
        assert!(!validator.is_valid(&oversized));
    }

    #[tokio::test]
    async fn shells_are_fail_closed_without_host_interception() {
        for tool in [
            submit_workflow_shell(),
            workflow_plan_report_shell(),
            workflow_result_read_shell(),
            message_shell(),
            workflow_get_shell(),
            workflow_node_update_shell(),
            workflow_node_add_shell(),
            workflow_node_delete_shell(),
        ] {
            let error = tool
                .execute(json!({}))
                .await
                .expect_err("壳工具不得在无拦截时返回成功");
            assert!(
                error
                    .model_feedback()
                    .unwrap_or_default()
                    .contains("错误："),
                "壳工具错误必须带「错误：」前缀"
            );
        }
    }

    #[test]
    fn orchestrator_tool_names_cover_all_shells() {
        let names = ORCHESTRATOR_PROTOCOL_TOOL_NAMES;
        assert!(names.contains(&"submit_workflow"));
        assert!(names.contains(&"workflow_plan_report"));
        assert!(names.contains(&"workflow_result_read"));
        assert!(names.contains(&"message"));
        assert!(names.contains(&"run_tool_program"));
        assert!(names.contains(&"workflow_get"));
        assert!(names.contains(&"workflow_node_update"));
        assert!(names.contains(&"workflow_node_add"));
        assert!(names.contains(&"workflow_node_delete"));
    }

    #[test]
    fn node_add_and_delete_schemas_are_strict() {
        let add_validator =
            jsonschema::draft202012::new(&workflow_node_add_parameters_schema()).expect("schema");
        // 完整节点定义 + insertBefore 数组合法。
        let valid = json!({
            "node": {
                "id": "n9", "title": "新增", "modelRef": "sonnet",
                "baseToolGroup": "read_only", "task": "任务", "outputKey": "out",
                "dependsOn": ["n1"]
            },
            "insertBefore": ["n2"],
        });
        assert!(add_validator.is_valid(&valid));
        // node 缺必填字段（task）拒绝；顶层未知字段拒绝。
        let incomplete = json!({
            "node": {
                "id": "n9", "title": "新增", "modelRef": "sonnet",
                "baseToolGroup": "read_only", "outputKey": "out"
            },
        });
        assert!(!add_validator.is_valid(&incomplete));
        let extra = json!({ "node": valid["node"].clone(), "extra": true });
        assert!(!add_validator.is_valid(&extra));

        let delete_validator =
            jsonschema::draft202012::new(&workflow_node_delete_parameters_schema())
                .expect("schema");
        assert!(delete_validator.is_valid(&json!({ "nodeId": "n1", "force": true })));
        assert!(delete_validator.is_valid(&json!({ "nodeId": "n1" })));
        // 缺 nodeId / force 非布尔拒绝。
        assert!(!delete_validator.is_valid(&json!({ "force": true })));
        assert!(!delete_validator.is_valid(&json!({ "nodeId": "n1", "force": "yes" })));
    }

    #[test]
    fn node_update_patch_schema_is_strict_and_optional() {
        let validator = jsonschema::draft202012::new(&workflow_node_update_parameters_schema())
            .expect("schema");
        let valid = json!({
            "nodeId": "n1",
            "patch": { "task": "新任务", "usePlanMode": true },
        });
        assert!(validator.is_valid(&valid));

        // 空 patch、未知字段、节点 id 修改一律拒绝。
        let empty_patch = json!({ "nodeId": "n1", "patch": {} });
        assert!(!validator.is_valid(&empty_patch));
        let unknown = json!({ "nodeId": "n1", "patch": { "unknown": 1 } });
        assert!(!validator.is_valid(&unknown));
        let id_patch = json!({ "nodeId": "n1", "patch": { "id": "n2" } });
        assert!(!validator.is_valid(&id_patch));

        // 顶层缺 nodeId 拒绝；多余顶层字段拒绝。
        let missing_node = json!({ "patch": { "task": "x" } });
        assert!(!validator.is_valid(&missing_node));
        let extra_top = json!({ "nodeId": "n1", "patch": { "task": "x" }, "extra": true });
        assert!(!validator.is_valid(&extra_top));
    }

    #[test]
    fn shell_descriptions_match_model_contract() {
        assert!(message_shell()
            .definition()
            .description
            .contains("最终答复"));
        assert!(submit_workflow_shell()
            .definition()
            .description
            .contains("每轮最多提交一次"));
    }
}
