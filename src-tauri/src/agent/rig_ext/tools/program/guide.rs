//! 模型可见的程序契约，以及调用前的参数预检。
//!
//! 编排器看不到数据面工具自己的 schema，只能看到 `run_tool_program` 的描述。
//! 描述必须写清程序形状、引用限制，并带上本轮授权工具的字段名；否则模型会用
//! `path`、`/data/files` 这类不存在的写法反复试错。

use serde_json::Value;

use super::ast::ProgramNode;
use super::error::{ProgramError, ProgramErrorKind};
use super::validate::ValidatedProgram;
use super::value::contains_reference;
use super::DataPlane;
use crate::agent::rig_ext::tools::run_record::prepare_arguments;

const RULES: &str = r#"在受限运行时里把多次只读工具调用写成一个程序，执行完后只把 return 的值交回。程序不执行 Python、JavaScript 或 Shell，工具名必须是字面量。

程序形状（不符合则整次拒绝，任何步骤都不会执行）：
- 参数只有 version 与 root。version 固定为 1。
- root 必须是 {"op":"sequence","steps":[...]}。steps 至少一步，最后一步必须是 {"op":"return","value":...}，且这是全程序唯一的 return。
- call 的形状是 {"op":"call","id":"步骤ID","tool":"工具名","arguments":{...}}。id 匹配 [A-Za-z][A-Za-z0-9_-]{0,63}，全程序唯一。
- sequence 的 steps 不能为空。parallel 的 branches 为 2 到 8 个。嵌套深度不超过 6，节点不超过 64，调用不超过 32。
- parallel 的每个分支只能引用进入该 parallel 之前已经完成的步骤，不能引用兄弟分支。parallel 结束后，后面的步骤才能引用各分支结果。只有清单里标了「可并行」的工具能出现在 parallel 中。

引用规则：
- 唯一写法是 {"$ref":{"step":"已完成的步骤ID","pointer":"/data"}}。pointer 也可以是 "/output"，与 /data 相同，都是该步全文。
- 引用替换整个 JSON 值，不能嵌进字符串，也不能取子路径。清单里标了「返回文本」的工具没有 /data/files、/data/entries、/data/0 这类字段，写了会在执行前被拒绝。
- paths、pattern、patterns 必须写成字面量。不要把上一步的全文引用填进这些字段。上一步的文本只放进 return.value；下一轮由你读完后再把路径写成字面量。

结果：
- 清单里标了「返回文本」的工具，单步超过内联上限时截断并带标记（默认 10000 字符；read_file 传了 offset 或 limit 时 20000）。被截掉的部分不会保存，程序内也不调用摘要模型。不要传 compress 或 compress_intent。
- return 进入后续上下文的上限约 32000 字符。只组装这次调查需要的步骤。
- 读大文件用 paths 里的 path:start-end（如 "src/main.rs:1-80"）或 offset/limit。list_dir 最多两层，文件名后的 (:N行) 是总行数。

示例（工具名必须出现在下面的授权清单里，否则不要照抄）：
{"version":1,"root":{"op":"sequence","steps":[{"op":"parallel","branches":[{"op":"call","id":"listing","tool":"list_dir","arguments":{"paths":["."],"recursive":true}},{"op":"call","id":"hits","tool":"grep","arguments":{"pattern":"submit_graph","paths":["src"],"include":["**/*.rs"],"max_files":10}}]},{"op":"return","value":{"listing":{"$ref":{"step":"listing","pointer":"/data"}},"hits":{"$ref":{"step":"hits","pointer":"/data"}}}}]}}"#;

pub(super) fn render_description(plane: &DataPlane) -> String {
    let mut text = String::from(RULES);
    text.push_str("\n\n");
    text.push_str(&render_data_plane(plane));
    text
}

/// 不含 `$ref` 的 call 在执行前按目标工具 schema 校验。带引用的参数要等解析后
/// 再查（见 `check_resolved_arguments`）。
pub(super) fn reject_invalid_literal_arguments(
    program: &ValidatedProgram,
    plane: &DataPlane,
) -> Result<(), ProgramError> {
    check_node(&program.program().root, plane)
}

pub(super) fn check_resolved_arguments(
    id: &str,
    tool: &str,
    arguments: &Value,
    plane: &DataPlane,
) -> Result<(), ProgramError> {
    let Some(tool_impl) = plane.get(tool) else {
        return Err(ProgramError::new(
            ProgramErrorKind::Internal,
            format!("步骤 '{id}' 的数据面工具 '{tool}' 在校验参数时缺失"),
        )
        .for_step(id, tool));
    };
    let schema = tool_impl.definition().parameters;
    match prepare_arguments(tool, &schema, arguments) {
        Ok(_) => Ok(()),
        Err(error) => {
            let detail = error.message.trim_start_matches("错误：");
            Err(ProgramError::new(
                ProgramErrorKind::Validation,
                format!(
                    "步骤 '{id}' 调用 '{tool}' 的参数不符合该工具 schema：{detail}。字段名以 run_tool_program 描述里的「{tool}」清单为准，不要把 paths 写成 path，也不要把单个路径写成字符串"
                ),
            )
            .for_step(id, tool))
        }
    }
}

fn check_node(node: &ProgramNode, plane: &DataPlane) -> Result<(), ProgramError> {
    match node {
        ProgramNode::Call {
            id,
            tool,
            arguments,
        } => {
            if contains_reference(arguments) {
                return Ok(());
            }
            check_resolved_arguments(id, tool, arguments, plane)
        }
        ProgramNode::Sequence { steps } => {
            for step in steps {
                check_node(step, plane)?;
            }
            Ok(())
        }
        ProgramNode::Parallel { branches } => {
            for branch in branches {
                check_node(branch, plane)?;
            }
            Ok(())
        }
        ProgramNode::Return { .. } => Ok(()),
    }
}

fn render_data_plane(plane: &DataPlane) -> String {
    let entries = plane.contracts();
    if entries.is_empty() {
        return "当前可调用的数据面工具：无。当前设置没有授权任何数据面能力，程序里的 call 都会被拒绝。".to_string();
    }
    let mut lines = vec![
        "当前可调用的数据面工具（只能使用这些名字和字段；未列出的工具或字段会被拒绝）："
            .to_string(),
    ];
    for entry in entries {
        let parallel = if entry.supports_parallel_readonly {
            "可并行"
        } else {
            "仅 sequence"
        };
        let result = if entry.returns_text {
            "返回文本"
        } else {
            "返回 JSON"
        };
        lines.push(format!(
            "\n### `{name}`（{parallel}，{result}）",
            name = entry.name
        ));
        let purpose = purpose_before_compress(&entry.description);
        if !purpose.is_empty() {
            lines.push(purpose);
        }
        lines.push(digest_parameters(&entry.parameters));
    }
    lines.join("\n")
}

/// 数据面工具的原文描述里常有「compress 后全文在工具产物」的句子。程序内这两句
/// 不成立，从第一次出现 compress 处截断，避免模型按那套规则写程序。
fn purpose_before_compress(description: &str) -> String {
    let cut = description.find("compress").unwrap_or(description.len());
    description[..cut]
        .trim()
        .trim_end_matches(['。', '，', '；', '.', ',', ';', ' '])
        .to_string()
}

fn digest_parameters(schema: &Value) -> String {
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return "参数：对象。字段以该工具实际接受的为准。".to_string();
    };
    let required = required_names(schema);
    let mut lines = Vec::new();
    if let Some(note) = any_of_note(schema) {
        lines.push(note);
    }
    for (name, property) in properties {
        if name == "compress" || name == "compress_intent" {
            continue;
        }
        let kind = type_name(property);
        let flag = if required.contains(name.as_str()) {
            "必填"
        } else {
            "可选"
        };
        let mut line = format!("- {name}（{flag}，{kind}）");
        if let Some(default) = property.get("default") {
            if !default.is_null() {
                line.push_str(&format!("，默认 {default}"));
            }
        }
        if let Some(description) = property.get("description").and_then(Value::as_str) {
            let sentence = first_sentence(description);
            if !sentence.is_empty() {
                line.push('：');
                line.push_str(sentence);
            }
        }
        lines.push(line);
    }
    if lines.is_empty() {
        return "参数：对象，无额外字段。".to_string();
    }
    lines.join("\n")
}

fn required_names(schema: &Value) -> std::collections::BTreeSet<&str> {
    schema
        .get("required")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

fn any_of_note(schema: &Value) -> Option<String> {
    let groups = schema.get("anyOf")?.as_array()?;
    let parts = groups
        .iter()
        .filter_map(|group| {
            let names = group
                .get("required")?
                .as_array()?
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>();
            if names.is_empty() {
                None
            } else {
                Some(names.join("、"))
            }
        })
        .collect::<Vec<_>>();
    if parts.is_empty() {
        None
    } else {
        Some(format!("至少提供其中一组：{}。", parts.join("，或 ")))
    }
}

fn type_name(schema: &Value) -> String {
    let Some(kind) = schema.get("type").and_then(Value::as_str) else {
        return "any".to_string();
    };
    if kind == "array" {
        let item = schema
            .get("items")
            .and_then(|value| value.get("type"))
            .and_then(Value::as_str)
            .unwrap_or("any");
        return format!("{item}[]");
    }
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        let labels = values
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join("|");
        if !labels.is_empty() {
            return format!("{kind}（{labels}）");
        }
    }
    kind.to_string()
}

fn first_sentence(text: &str) -> &str {
    let trimmed = text.trim();
    if let Some(index) = trimmed.find('。') {
        let end = index + '。'.len_utf8();
        if index > 0 && end <= 240 {
            return &trimmed[..end];
        }
    }
    let end = trimmed.chars().take(160).map(char::len_utf8).sum();
    &trimmed[..end]
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::validate::{validate_program_value, CapabilityPolicy, ProgramLimits};
    use super::super::{build_program_tool, DataPlane};
    use super::reject_invalid_literal_arguments;
    use rig::tool::{PortableDynamicTool, ToolErrorKind, ToolOutput};

    fn read_tool() -> PortableDynamicTool {
        PortableDynamicTool::new(
            "read_file",
            "读取文本文件，输出格式为 行号|内容。paths 支持 path:start-end。compress=false 时不摘要。",
            json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["paths"],
                "properties": {
                    "paths": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "要读取的文件路径列表。即使只读一个文件也必须传单元素数组。"
                    },
                    "compress": { "type": "boolean", "description": "不要出现在摘要里" }
                }
            }),
            |_| Box::pin(async { Ok(ToolOutput::text("body")) }),
        )
    }

    fn catalog(name: &str) -> Option<CapabilityPolicy> {
        (name == "read_file").then_some(CapabilityPolicy::parallel_readonly().with_text_result())
    }

    #[test]
    fn description_lists_live_fields_and_hides_compress_guidance() {
        let tool = build_program_tool(None, DataPlane::new(vec![read_tool()]));
        let description = tool.definition().description;
        assert!(description.contains("当前可调用的数据面工具"));
        assert!(description.contains("`read_file`"));
        assert!(description.contains("可并行"));
        assert!(description.contains("返回文本"));
        assert!(description.contains("paths（必填，string[]）"));
        assert!(description.contains("path:start-end"));
        assert!(!description.contains("不要出现在摘要里"));
        assert!(description.contains("/data/files"));
        assert!(description.contains("唯一的 return"));
    }

    #[test]
    fn literal_arguments_are_rejected_against_the_tool_schema() {
        let plane = DataPlane::new(vec![read_tool()]);
        let program = json!({
            "version": 1,
            "root": { "op": "sequence", "steps": [
                { "op": "call", "id": "read", "tool": "read_file", "arguments": { "path": "a.rs" } },
                { "op": "return", "value": { "$ref": { "step": "read", "pointer": "/data" } } }
            ] }
        });
        let validated = validate_program_value(&program, &catalog, &ProgramLimits::default())
            .expect("shape is valid");
        let error = reject_invalid_literal_arguments(&validated, &plane).unwrap_err();
        assert_eq!(
            error.kind,
            super::super::error::ProgramErrorKind::Validation
        );
        assert!(error.message.contains("paths") || error.message.contains("path"));
    }

    #[tokio::test]
    async fn wrong_argument_name_fails_before_the_tool_runs() {
        let tool = build_program_tool(None, DataPlane::new(vec![read_tool()]));
        let error = tool
            .execute(json!({
                "version": 1,
                "root": { "op": "sequence", "steps": [
                    { "op": "call", "id": "read", "tool": "read_file", "arguments": { "path": "a.rs" } },
                    { "op": "return", "value": null }
                ] }
            }))
            .await
            .expect_err("schema rejection");
        assert_eq!(error.kind(), ToolErrorKind::InvalidArgs);
        assert_eq!(error.retryable(), Some(true));
        assert!(error.message().contains("paths") || error.message().contains("path"));
    }
}
