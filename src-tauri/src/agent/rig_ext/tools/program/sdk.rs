//! 从本轮数据面工具的 JSON Schema 生成模型可见的 SDK 文本。
//! 字段名只出现在这里和工具自己的 schema 里，不在编排提示词里再抄一份。

use serde_json::Value;

use super::limits::MODEL_VISIBLE_CHARS;
use super::ToolContract;

pub(super) fn render_section(contracts: &[ToolContract]) -> String {
    let mut text = String::from(
        "# 数据面 SDK\n\n这段声明是本轮程序可调用工具的唯一字段来源。类型标注只供阅读，运行时按各工具的 JSON Schema 校验。下面的花括号是类型声明，不是模板变量。\n\n",
    );
    if contracts.is_empty() {
        text.push_str("当前没有授权任何数据面工具。`tools` 上的任何调用都会抛出 ToolCallError。\n");
        return text;
    }

    text.push_str("```javascript\nclass ToolCallError extends Error {\n  constructor(tool, message) {\n    super(message);\n    this.name = \"ToolCallError\";\n    this.tool = tool;\n  }\n}\n\ndeclare const tools: {\n");
    for entry in contracts {
        text.push_str(&render_method(entry));
    }
    text.push_str("};\n```\n\n");
    text.push_str("调用 `await tools.name({ ... })` 或 `await tools[\"name\"]({ ... })`。参数必须是对象。互不依赖、且注释标了可并行的只读工具可以放进 `Promise.all`；其余调用即使写进 `Promise.all` 也会按提交顺序单独执行。失败抛出 `ToolCallError`，可以 `try/catch` 后继续。只有 `return` 的值和 `console.log` / `console.error` 回到上下文。");
    text.push_str(&format!(
        "交回模型的文本上限是 {MODEL_VISIBLE_CHARS} 字符。绑定返回的长文本可能已经按该工具的内联上限截断。程序不能访问文件、网络或进程。\n"
    ));
    text
}

fn render_method(entry: &ToolContract) -> String {
    let parallel = if entry.supports_parallel_readonly {
        "可与其它只读调用重叠"
    } else {
        "提交后单独执行"
    };
    let mut doc = vec![format!("  /** {parallel}。")];
    let purpose = purpose_before_compress(&entry.description);
    if !purpose.is_empty() {
        doc.push(format!("   * {purpose}"));
    }
    for line in field_docs(&entry.parameters) {
        doc.push(format!("   * {line}"));
    }
    if entry.parameters.get("additionalProperties") == Some(&Value::Bool(false)) {
        doc.push("   * 不要添加未列出的字段。".to_string());
    }
    if let Some(note) = any_of_note(&entry.parameters) {
        doc.push(format!("   * {note}"));
    }
    doc.push("   */".to_string());
    let fields = render_fields(&entry.parameters);
    doc.push(format!(
        "  {}(args: {{ {fields} }}): Promise<unknown>;\n",
        entry.name
    ));
    doc.join("\n")
}

fn field_docs(schema: &Value) -> Vec<String> {
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return Vec::new();
    };
    let required = required_names(schema);
    let mut names = properties.keys().cloned().collect::<Vec<_>>();
    names.sort();
    names
        .into_iter()
        .filter(|name| !is_hidden_field(name))
        .map(|name| {
            let property = &properties[&name];
            let flag = if required.contains(&name) {
                "必填"
            } else {
                "可选"
            };
            let mut line = format!("- {name}（{flag}，{}）", render_type(property));
            if let Some(description) = property.get("description").and_then(Value::as_str) {
                let sentence = first_sentence(description);
                if !sentence.is_empty() && !sentence.contains("compress") {
                    line.push('：');
                    line.push_str(sentence);
                }
            }
            line
        })
        .collect()
}

fn render_fields(schema: &Value) -> String {
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return String::new();
    };
    let required = required_names(schema);
    let mut names = properties.keys().cloned().collect::<Vec<_>>();
    names.sort();
    names
        .into_iter()
        .filter(|name| !is_hidden_field(name))
        .map(|name| {
            let optional = if required.contains(&name) { "" } else { "?" };
            format!("{name}{optional}: {}", render_type(&properties[&name]))
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn is_hidden_field(name: &str) -> bool {
    name == "compress" || name == "compress_intent"
}

fn required_names(schema: &Value) -> std::collections::BTreeSet<String> {
    schema
        .get("required")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
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
                .filter(|name| !is_hidden_field(name))
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

fn render_type(schema: &Value) -> String {
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        let labels = values
            .iter()
            .filter_map(|value| serde_json::to_string(value).ok())
            .collect::<Vec<_>>()
            .join(" | ");
        if !labels.is_empty() {
            return labels;
        }
    }
    match schema.get("type").and_then(Value::as_str) {
        Some("array") => {
            let item = schema
                .get("items")
                .map(render_type)
                .unwrap_or_else(|| "unknown".to_string());
            format!("{item}[]")
        }
        Some("string") => "string".to_string(),
        Some("integer" | "number") => "number".to_string(),
        Some("boolean") => "boolean".to_string(),
        Some("object") => "object".to_string(),
        Some("null") => "null".to_string(),
        _ => "unknown".to_string(),
    }
}

fn purpose_before_compress(description: &str) -> String {
    let cut = description.find("compress").unwrap_or(description.len());
    description[..cut]
        .trim()
        .trim_end_matches(['。', '，', '；', '.', ',', ';', ' '])
        .to_string()
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

    use super::super::DataPlane;
    use super::render_section;
    use rig::tool::PortableDynamicTool;

    fn read_file() -> PortableDynamicTool {
        PortableDynamicTool::new(
            "read_file",
            "读取文本文件。paths 支持 path:start-end。compress=false 时不摘要。",
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
                    "offset": { "type": "integer", "description": "起始行号。" },
                    "compress": { "type": "boolean", "description": "不要出现" },
                    "compress_intent": { "type": "string" }
                }
            }),
            |_| Box::pin(async { Ok(rig::tool::ToolOutput::text("")) }),
        )
    }

    #[test]
    fn sdk_lists_live_fields_and_hides_compress() {
        let plane = DataPlane::new(vec![read_file()]);
        let sdk = render_section(&plane.contracts());
        assert!(sdk.contains("paths: string[]"));
        assert!(sdk.contains("offset?: number"));
        assert!(sdk.contains("read_file(args:"));
        assert!(!sdk.contains("path?:"));
        assert!(!sdk.contains("path: string"));
        assert!(!sdk.contains("compress"));
        assert!(sdk.contains("不要添加未列出的字段"));
        assert!(sdk.contains("32000"));
        assert!(!sdk.contains("$ref"));
    }

    #[test]
    fn empty_plane_states_that_every_call_fails() {
        let sdk = render_section(&[]);
        assert!(sdk.contains("没有授权任何数据面工具"));
        assert!(sdk.contains("ToolCallError"));
    }
}
