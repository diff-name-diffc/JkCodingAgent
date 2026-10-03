//! 工具调用的参数准备（schema 默认值注入 + Draft 2020-12 校验）。
//!
//! 台账（`dispatcher_tool_runs`）不再经本模块写入：登记唯一走调度器
//! `enqueue` 的准入批次（`register_tool_task_batch`），终态唯一走 worker 的
//! `settle_tool_completion`——单写路径（P0-3）。本模块此前的
//! `create_and_start_tool_run_with_trace` / `finish_tool_run` 裸路径
//! （无 `ToolInvocationContext` 时由策略层 `before_call` 触发，产出
//! agent_run_id 为 NULL 的孤儿行）已随协议批免台账短路一并退役。
//! 策略来源为工具名（`crate::agent::rig_ext::tools::spec::ToolSpec`
//! 策略表），参数校验直接对 `PortableDynamicTool` 的 definition 做。

use serde_json::Value;

/// 校验错误摘要最多列出的条数（对齐旧实现的同名常量）。
const MAX_SUMMARIZED_ERRORS: usize = 8;

// 回归守护（仅测试）：统计 `prepare_arguments` 的执行次数。整套调度链
// （enqueue 准入 → 台账 → before_call → execute）每调用只允许执行一次。
#[cfg(test)]
thread_local! {
    pub(crate) static PREPARE_ARGUMENTS_CALLS: std::cell::Cell<u64> =
        const { std::cell::Cell::new(0) };
}

/// 参数准备失败：模型可见文本（已带「错误：」前缀）+ 稳定错误码。
#[derive(Debug)]
pub(crate) struct ArgumentError {
    pub message: String,
    pub code: &'static str,
}

/// 参数准备：schema 默认值注入 + Draft 2020-12 校验。每调用只执行一次：
/// 调度器路径在 enqueue 准入时产出 effective 值，随 `ToolInvocationContext`
/// 流入 worker，before_call 门禁与 execute 共用；无上下文的调用由策略层
/// 回退计算。工具闭包收到的即 effective 参数，闭包内的
/// `unwrap_or(default)` 只在 schema 未声明 default 时兜底。
pub(crate) fn prepare_arguments(
    tool_name: &str,
    schema: &Value,
    args: &Value,
) -> Result<Value, ArgumentError> {
    let mut effective = args.clone();
    #[cfg(test)]
    PREPARE_ARGUMENTS_CALLS.with(|count| count.set(count.get() + 1));
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

/// 递归注入 schema 默认值（对齐旧实现的同名助手）。
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
/// （对齐旧实现的同名助手）。
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
