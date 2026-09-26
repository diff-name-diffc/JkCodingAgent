//! ToolProgram AST 与模型参数 schema（逐字迁移自旧自实现工具层（已随迁移删除）的工具程序 ast 模块）。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const TOOL_PROGRAM_VERSION: u8 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolProgram {
    pub version: u8,
    pub root: ProgramNode,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProgramNode {
    Call {
        id: String,
        tool: String,
        #[serde(default = "empty_object")]
        arguments: Value,
    },
    Sequence {
        steps: Vec<ProgramNode>,
    },
    Parallel {
        branches: Vec<ProgramNode>,
    },
    Return {
        value: Value,
    },
}

fn empty_object() -> Value {
    json!({})
}

/// `run_tool_program` 的模型参数 schema。
///
/// Rust 反序列化与 `validate_program` 才是权威校验；该 schema 用于尽早约束
/// 模型输出。递归节点通过 `$defs/node` 表达，四种 `op` 之外没有扩展口。
pub fn tool_program_parameters_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["version", "root"],
        "properties": {
            "version": {
                "const": TOOL_PROGRAM_VERSION,
                "description": "固定为 1。"
            },
            "root": {
                "description": "必须是 op=sequence。steps 最后一项必须是全程序唯一的 return，否则整次拒绝、不执行。",
                "type": "object",
                "additionalProperties": false,
                "required": ["op", "steps"],
                "properties": {
                    "op": { "const": "sequence" },
                    "steps": {
                        "type": "array",
                        "minItems": 1,
                        "items": { "$ref": "#/$defs/node" }
                    }
                }
            }
        },
        "$defs": {
            "node": {
                "oneOf": [
                    {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["op", "id", "tool"],
                        "properties": {
                            "op": { "const": "call" },
                            "id": {
                                "type": "string",
                                "description": "全程序唯一，匹配 [A-Za-z][A-Za-z0-9_-]{0,63}。"
                            },
                            "tool": {
                                "type": "string",
                                "description": "只能是工具描述里「当前可调用的数据面工具」列出的名字。"
                            },
                            "arguments": {
                                "type": "object",
                                "default": {},
                                "description": "该工具的参数对象。路径字段名是 paths（字符串数组），不是 path。paths、pattern、patterns 写字面量，不要填 $ref。"
                            }
                        }
                    },
                    {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["op", "steps"],
                        "properties": {
                            "op": { "const": "sequence" },
                            "steps": {
                                "type": "array",
                                "items": { "$ref": "#/$defs/node" }
                            }
                        }
                    },
                    {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["op", "branches"],
                        "properties": {
                            "op": { "const": "parallel" },
                            "branches": {
                                "type": "array",
                                "items": { "$ref": "#/$defs/node" }
                            }
                        }
                    },
                    {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["op", "value"],
                        "properties": {
                            "op": { "const": "return" },
                            "value": true
                        }
                    }
                ]
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{tool_program_parameters_schema, ProgramNode, ToolProgram, TOOL_PROGRAM_VERSION};

    #[test]
    fn serde_round_trip_preserves_all_four_operations() {
        let program = ToolProgram {
            version: TOOL_PROGRAM_VERSION,
            root: ProgramNode::Sequence {
                steps: vec![
                    ProgramNode::Call {
                        id: "first".to_string(),
                        tool: "grep".to_string(),
                        arguments: json!({ "pattern": "ToolRuntime" }),
                    },
                    ProgramNode::Parallel {
                        branches: vec![
                            ProgramNode::Call {
                                id: "left".to_string(),
                                tool: "read_file".to_string(),
                                arguments: json!({ "path": "a.rs" }),
                            },
                            ProgramNode::Sequence {
                                steps: vec![ProgramNode::Call {
                                    id: "right".to_string(),
                                    tool: "read_file".to_string(),
                                    arguments: json!({ "path": "b.rs" }),
                                }],
                            },
                        ],
                    },
                    ProgramNode::Return {
                        value: json!({ "$ref": { "step": "first", "pointer": "/output" } }),
                    },
                ],
            },
        };

        let encoded = serde_json::to_value(&program).expect("serialize program");
        let decoded: ToolProgram = serde_json::from_value(encoded).expect("deserialize program");

        assert_eq!(decoded, program);
    }

    #[test]
    fn serde_rejects_unknown_fields_and_operations() {
        let unknown_field = json!({
            "version": 1,
            "root": { "op": "return", "value": null, "unexpected": true }
        });
        assert!(serde_json::from_value::<ToolProgram>(unknown_field).is_err());

        let unknown_operation = json!({
            "version": 1,
            "root": { "op": "eval", "code": "danger()" }
        });
        assert!(serde_json::from_value::<ToolProgram>(unknown_operation).is_err());
    }

    #[test]
    fn call_arguments_default_to_empty_object() {
        let value = json!({
            "version": 1,
            "root": { "op": "call", "id": "read", "tool": "read_file" }
        });
        let program: ToolProgram = serde_json::from_value(value).expect("deserialize program");

        let ProgramNode::Call { arguments, .. } = program.root else {
            panic!("expected call");
        };
        assert_eq!(arguments, json!({}));
    }

    #[test]
    fn schema_is_closed_and_recursive() {
        let schema = tool_program_parameters_schema();

        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["properties"]["version"]["const"], 1);
        assert_eq!(
            schema["properties"]["root"]["properties"]["op"]["const"],
            "sequence"
        );
        assert_eq!(
            schema["properties"]["root"]["properties"]["steps"]["items"]["$ref"],
            "#/$defs/node"
        );
        assert_eq!(
            schema["$defs"]["node"]["oneOf"].as_array().unwrap().len(),
            4
        );
    }
}
