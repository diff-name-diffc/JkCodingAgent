//! 工具调用台账（`dispatcher_tool_runs`）与参数准备。
//!
//! 迁移自旧 `agent/tools/runtime.rs`（`create_and_start_tool_run_with_trace` /
//! `finish_tool_run`）与 `registry.rs` 的参数准备片段（schema 默认值注入 +
//! Draft 2020-12 校验），但不再经 `ToolRegistry`：策略来源改为工具名
//! （`crate::agent::rig_ext::tools::spec::ToolSpec` 策略表），参数校验直接对
//! `PortableDynamicTool` 的 definition 做。

use serde_json::{json, Value};
use tauri::ipc::Channel;

use crate::agent::common::emit;
use crate::agent::db::{DispatcherDb, FinishToolRun, NewToolRun, ToolRunTraceContext};
use crate::agent::rig_ext::events::AgentEvent;
use crate::agent::rig_ext::tools::spec::ToolSpec;

/// 校验错误摘要最多列出的条数（对齐旧 `MAX_SUMMARIZED_ERRORS`）。
const MAX_SUMMARIZED_ERRORS: usize = 8;

/// 参数准备失败：模型可见文本（已带「错误：」前缀）+ 稳定错误码。
#[derive(Debug)]
pub(crate) struct ArgumentError {
    pub message: String,
    pub code: &'static str,
}

/// 参数准备：schema 默认值注入 + Draft 2020-12 校验（对齐旧
/// `ToolRegistry::prepare_input`，但不注入到执行体——工具自身按
/// `unwrap_or(default)` 处理缺省；此处仅用于校验与台账的 effective 参数）。
pub(crate) fn prepare_arguments(
    tool_name: &str,
    schema: &Value,
    args: &Value,
) -> Result<Value, ArgumentError> {
    let mut effective = args.clone();
    apply_schema_defaults(schema, &mut effective);

    let validator = match jsonschema::draft202012::new(schema) {
        Ok(validator) => validator,
        Err(error) => {
            return Err(ArgumentError {
                message: format!("错误：工具 '{tool_name}' 的参数 Schema 无效：{error}"),
                code: "invalid_tool_schema",
            })
        }
    };

    // 先全量收集再截断：total 必须反映真实错误总数（旧实现同口径）。
    let all_errors: Vec<_> = validator.iter_errors(&effective).collect();
    let total = all_errors.len();
    if total == 0 {
        return Ok(effective);
    }
    let mut summaries = all_errors
        .into_iter()
        .take(MAX_SUMMARIZED_ERRORS)
        .map(|error| {
            let instance_path = error.instance_path().to_string();
            let path = if instance_path.is_empty() {
                "/".to_string()
            } else {
                instance_path
            };
            format!("{path}: {}", describe_validation_error(&error))
        })
        .collect::<Vec<_>>();
    if total > MAX_SUMMARIZED_ERRORS {
        summaries.push(format!("…共 {total} 处错误"));
    }
    Err(ArgumentError {
        message: format!(
            "错误：工具 '{tool_name}' 参数不符合 JSON Schema：{}",
            summaries.join("；")
        ),
        code: "invalid_arguments",
    })
}

/// 递归注入 schema 默认值（对齐旧 `apply_schema_defaults`）。
fn apply_schema_defaults(schema: &Value, instance: &mut Value) {
    match instance {
        Value::Object(object) => {
            let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
                return;
            };
            for (name, property_schema) in properties {
                if !object.contains_key(name) {
                    if let Some(default) = property_schema.get("default") {
                        object.insert(name.clone(), default.clone());
                    }
                }
                if let Some(value) = object.get_mut(name) {
                    apply_schema_defaults(property_schema, value);
                }
            }
        }
        Value::Array(items) => {
            if let Some(item_schema) = schema.get("items") {
                for item in items {
                    apply_schema_defaults(item_schema, item);
                }
            }
        }
        _ => {}
    }
}

/// 校验错误的人类/模型可读描述：oneOf/anyOf 分支展开到最贴近的一条
/// （对齐旧 `describe_validation_error`）。
fn describe_validation_error(error: &jsonschema::ValidationError<'_>) -> String {
    let context = match error.kind() {
        jsonschema::error::ValidationErrorKind::OneOfNotValid { context }
        | jsonschema::error::ValidationErrorKind::AnyOf { context } => context,
        _ => return error.to_string(),
    };
    let Some(closest) = context
        .iter()
        .filter(|branch| !branch.is_empty())
        .min_by_key(|branch| branch.len())
    else {
        return error.to_string();
    };
    let detail = closest
        .iter()
        .take(3)
        .map(|sub_error| sub_error.to_string())
        .collect::<Vec<_>>()
        .join("; ");
    if closest.len() > 3 {
        format!("{detail}; …共 {} 处", closest.len())
    } else {
        detail
    }
}

// ─── 台账（dispatcher_tool_runs） ────────────────────────────────────────────

/// 台账写入上下文。
#[derive(Clone, Copy)]
pub(crate) struct RigToolRunContext<'a> {
    pub db: &'a DispatcherDb,
    pub workspace_id: &'a str,
    pub on_event: &'a Channel<AgentEvent>,
}

/// 已开始的工具运行句柄（收尾时消费）。
#[derive(Clone, Debug)]
pub(crate) struct RigToolRun {
    pub run_id: String,
}

/// 台账元数据：`registered=true` 时带策略字段；未注册（模型幻觉）工具名
/// 显式标记 `registered=false`，避免审计误以为该调用经过真实策略评估。
fn run_metadata_json(spec: &ToolSpec, registered: bool) -> String {
    let value = if registered {
        json!({
            "registered": true,
            "safety": spec.safety,
            "access": spec.access,
            "execution": spec.execution,
            "resultPolicy": spec.result_policy,
        })
    } else {
        json!({ "registered": false })
    };
    serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string())
}

/// 创建并标记启动一次工具运行（对齐旧
/// `create_and_start_tool_run_with_trace`）：创建 + 广播 → 标记 started +
/// 广播；标记失败时把记录收敛为 internal_error 终态，避免悬挂中间态。
pub(crate) async fn start_tool_run(
    context: RigToolRunContext<'_>,
    spec: &ToolSpec,
    registered: bool,
    tool_call_id: &str,
    arguments: &Value,
    effective_arguments: &Value,
    trace: ToolRunTraceContext,
) -> anyhow::Result<RigToolRun> {
    let metadata_json = run_metadata_json(spec, registered);
    let run = context
        .db
        .create_tool_run_with_trace_async(
            NewToolRun {
                workspace_id: context.workspace_id.to_string(),
                tool_call_id: tool_call_id.to_string(),
                tool_name: spec.name.clone(),
                provider: spec.provider.clone(),
                category: spec.category.as_str().to_string(),
                arguments_json: serde_json::to_string(arguments)?,
                effective_arguments_json: serde_json::to_string(effective_arguments)?,
                metadata_json,
            },
            trace,
        )
        .await?;
    emit(context.on_event, AgentEvent::ToolRunUpdated { run: run.clone() });

    let started = match context.db.mark_tool_run_started_async(&run.id).await {
        Ok(started) => started,
        Err(error) => {
            if let Ok(finished) = context
                .db
                .finish_tool_run_async(
                    &run.id,
                    FinishToolRun {
                        status: "internal_error".to_string(),
                        result_mode: None,
                        message_id: None,
                        error_kind: Some("internal".to_string()),
                        error_message: Some(format!("标记工具运行启动失败：{error}")),
                        action_kind: None,
                        metadata_json: None,
                    },
                )
                .await
            {
                emit(context.on_event, AgentEvent::ToolRunUpdated { run: finished });
            }
            return Err(error);
        }
    };
    emit(
        context.on_event,
        AgentEvent::ToolRunUpdated {
            run: started.clone(),
        },
    );
    Ok(RigToolRun {
        run_id: started.id,
    })
}

/// 台账收尾更新（对齐旧 `finish_tool_run`）：无 message_id 时广播自身；
/// 有 message_id 时把该消息挂上工具运行树并广播整棵树。
pub(crate) struct RigToolRunFinish<'a> {
    pub status: &'a str,
    pub result_mode: Option<&'a str>,
    pub message_id: Option<&'a str>,
    pub error_kind: Option<&'a str>,
    pub error_message: Option<&'a str>,
    pub action_kind: Option<&'a str>,
    pub metadata_json: Option<&'a str>,
}

pub(crate) async fn finish_tool_run(
    context: RigToolRunContext<'_>,
    run: &RigToolRun,
    update: RigToolRunFinish<'_>,
) -> anyhow::Result<()> {
    let finished = context
        .db
        .finish_tool_run_async(
            &run.run_id,
            FinishToolRun {
                status: update.status.to_string(),
                result_mode: update.result_mode.map(str::to_string),
                message_id: update.message_id.map(str::to_string),
                error_kind: update.error_kind.map(str::to_string),
                error_message: update.error_message.map(str::to_string),
                action_kind: update.action_kind.map(str::to_string),
                metadata_json: update.metadata_json.map(str::to_string),
            },
        )
        .await?;
    if let Some(message_id) = update.message_id {
        let tree = context
            .db
            .attach_tool_run_tree_message_async(&finished.id, message_id)
            .await?;
        for run in tree {
            emit(context.on_event, AgentEvent::ToolRunUpdated { run });
        }
    } else {
        emit(
            context.on_event,
            AgentEvent::ToolRunUpdated { run: finished },
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{apply_schema_defaults, prepare_arguments};

    #[test]
    fn defaults_are_injected_recursively() {
        let schema = json!({
            "type": "object",
            "properties": {
                "flag": { "type": "boolean", "default": false },
                "nested": {
                    "type": "object",
                    "properties": { "limit": { "type": "integer", "default": 10 } }
                }
            }
        });
        let mut args = json!({ "nested": {} });
        apply_schema_defaults(&schema, &mut args);
        assert_eq!(args["flag"], json!(false));
        assert_eq!(args["nested"]["limit"], json!(10));
    }

    #[test]
    fn prepare_arguments_rejects_schema_violations_with_prefix() {
        let schema = json!({
            "type": "object",
            "properties": { "command": { "type": "string" } },
            "required": ["command"]
        });
        let error = prepare_arguments("local_zsh", &schema, &json!({ "command": 7 }))
            .expect_err("schema violation must be rejected");
        assert!(error.message.starts_with("错误："), "{}", error.message);
        assert_eq!(error.code, "invalid_arguments");
    }

    #[test]
    fn prepare_arguments_accepts_valid_and_fills_defaults() {
        let schema = json!({
            "type": "object",
            "properties": {
                "command": { "type": "string" },
                "compress": { "type": "boolean", "default": false }
            },
            "required": ["command"]
        });
        let effective = prepare_arguments("local_zsh", &schema, &json!({ "command": "ls" }))
            .expect("valid arguments");
        assert_eq!(effective["command"], json!("ls"));
        assert_eq!(effective["compress"], json!(false));
    }
}
