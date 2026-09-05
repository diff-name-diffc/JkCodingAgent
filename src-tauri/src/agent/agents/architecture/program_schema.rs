//! `architecture_run` 的模型参数 JSON Schema（尽早约束模型输出）。
//!
//! 与程序模块的关系：**权威校验仍是反序列化 + `validate_program`**（AST 在
//! `program_ast.rs`，校验在 `program_validate.rs`，入口与契约常量在 `program.rs`），
//! 本文件的 schema 只是下发给模型的前置契约。两处的字段/枚举/数值上限
//! 必须逐项一致——数值上限直接复用 `program.rs` 的常量，避免双份漂移。
//!
//! 指令层使用 `_type` 判别式 + `oneOf` 分支。oneOf 的失败消息本身较笼统，
//! 注册表校验层（`tools/registry.rs`）会把 oneOf 错误展开为「最接近分支」
//! 的子错误（如「Additional properties are not allowed ('labelPosition'…)」），
//! 让模型一轮就能定位到具体字段。

use serde_json::{json, Value};

use super::program::{
    ARCH_MAX_INSTRUCTIONS, ARCH_PROGRAM_VERSION, COORD_LIMIT, GAP_LIMIT, MAX_LABEL_LEN,
    MAX_LAYOUT_TARGETS, MAX_REPARENT_TARGETS, MAX_SELECT_TARGETS, MAX_TARGETS_PER_DELETE,
    MAX_TEXT_LEN, NUDGE_LIMIT, SIZE_LIMIT,
};

fn enum_schema(values: &[&str]) -> Value {
    json!({ "type": "string", "enum": values })
}

fn number_schema(min: f64, max: f64) -> Value {
    json!({ "type": "number", "minimum": min, "maximum": max })
}

const COLOR_VALUES: &[&str] = &[
    "black",
    "grey",
    "light-violet",
    "violet",
    "blue",
    "light-blue",
    "yellow",
    "orange",
    "green",
    "light-green",
    "light-red",
    "red",
    "white",
];
const FILL_VALUES: &[&str] = &["none", "semi", "solid", "pattern", "fill", "lined-fill"];
const SIZE_VALUES: &[&str] = &["s", "m", "l", "xl"];
const DASH_VALUES: &[&str] = &["draw", "solid", "dashed", "dotted", "none"];
const FONT_VALUES: &[&str] = &["draw", "sans", "serif", "mono"];
const ALIGN_VALUES: &[&str] = &["start", "middle", "end"];
const GEO_VALUES: &[&str] = &[
    "rectangle",
    "ellipse",
    "triangle",
    "diamond",
    "pentagon",
    "hexagon",
    "octagon",
    "star",
    "rhombus",
    "rhombus-2",
    "oval",
    "cloud",
    "trapezoid",
    "arrow-right",
    "arrow-left",
    "arrow-up",
    "arrow-down",
    "x-box",
    "check-box",
    "heart",
];
const ARROWHEAD_VALUES: &[&str] = &[
    "arrow", "triangle", "square", "dot", "pipe", "diamond", "inverted", "bar", "none",
];

fn style_properties() -> Value {
    json!({
        "color": enum_schema(COLOR_VALUES),
        "labelColor": enum_schema(COLOR_VALUES),
        "fill": enum_schema(FILL_VALUES),
        "size": enum_schema(SIZE_VALUES),
        "dash": enum_schema(DASH_VALUES),
        "font": enum_schema(FONT_VALUES),
        "align": enum_schema(ALIGN_VALUES),
    })
}

/// 箭头支持的样式子集（tldraw 箭头 props 只有 color/labelColor/size/dash，
/// 没有 fill/font/align——与前端 `styleProps` 实际落到箭头的字段一致）。
fn arrow_style_properties() -> Value {
    json!({
        "color": enum_schema(COLOR_VALUES),
        "labelColor": enum_schema(COLOR_VALUES),
        "size": enum_schema(SIZE_VALUES),
        "dash": enum_schema(DASH_VALUES),
    })
}

/// `architecture_run` 的模型参数 schema：`{program: {version, instructions}}`。
pub fn architecture_run_parameters_schema() -> Value {
    let create_shape = json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["_type", "ref", "shape"],
        "properties": {
            "_type": { "const": "create_shape" },
            "ref": { "type": "string", "pattern": "^[A-Za-z][A-Za-z0-9_-]{0,31}$" },
            "shape": enum_schema(&["geo", "note", "text", "frame"]),
            "geo": enum_schema(GEO_VALUES),
            "text": { "type": "string", "maxLength": MAX_TEXT_LEN },
            "x": number_schema(-COORD_LIMIT, COORD_LIMIT),
            "y": number_schema(-COORD_LIMIT, COORD_LIMIT),
            "w": number_schema(1.0, SIZE_LIMIT),
            "h": number_schema(1.0, SIZE_LIMIT),
            "into": { "type": "string", "minLength": 1, "maxLength": 64 },
        },
    });
    let create_arrow = json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["_type", "from", "to"],
        "properties": {
            "_type": { "const": "create_arrow" },
            "ref": { "type": "string", "pattern": "^[A-Za-z][A-Za-z0-9_-]{0,31}$" },
            "from": { "type": "string", "minLength": 1, "maxLength": 64 },
            "to": { "type": "string", "minLength": 1, "maxLength": 64 },
            "label": { "type": "string", "maxLength": MAX_LABEL_LEN },
            "labelPosition": number_schema(0.0, 1.0),
            "kind": enum_schema(&["arc", "elbow"]),
            "arrowheadStart": enum_schema(ARROWHEAD_VALUES),
            "arrowheadEnd": enum_schema(ARROWHEAD_VALUES),
        },
    });
    let update_shape = json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["_type", "target"],
        "properties": {
            "_type": { "const": "update_shape" },
            "target": { "type": "string", "minLength": 1, "maxLength": 64 },
            "text": { "type": "string", "maxLength": MAX_TEXT_LEN },
            "x": number_schema(-COORD_LIMIT, COORD_LIMIT),
            "y": number_schema(-COORD_LIMIT, COORD_LIMIT),
            "w": number_schema(1.0, SIZE_LIMIT),
            "h": number_schema(1.0, SIZE_LIMIT),
        },
    });
    // 箭头属性只能在 update_arrow 中修改；update_shape 收到箭头字段时
    // additionalProperties 会拒绝，注册表层展开为可读的字段级错误。
    let update_arrow = json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["_type", "target"],
        "properties": {
            "_type": { "const": "update_arrow" },
            "target": { "type": "string", "minLength": 1, "maxLength": 64 },
            "label": { "type": "string", "maxLength": MAX_LABEL_LEN },
            "labelPosition": number_schema(0.0, 1.0),
            "kind": enum_schema(&["arc", "elbow"]),
            "arrowheadStart": enum_schema(ARROWHEAD_VALUES),
            "arrowheadEnd": enum_schema(ARROWHEAD_VALUES),
        },
    });
    // x/y 与 dx/dy 均可单轴给出（未给出的轴保持不变），两族互斥由
    // 权威语义校验保证——schema 层不用 oneOf 表达，避免模糊错误消息。
    let move_shape = json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["_type", "target"],
        "properties": {
            "_type": { "const": "move_shape" },
            "target": { "type": "string", "minLength": 1, "maxLength": 64 },
            "x": number_schema(-COORD_LIMIT, COORD_LIMIT),
            "y": number_schema(-COORD_LIMIT, COORD_LIMIT),
            "dx": number_schema(-NUDGE_LIMIT, NUDGE_LIMIT),
            "dy": number_schema(-NUDGE_LIMIT, NUDGE_LIMIT),
        },
    });
    let delete_shape = json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["_type", "targets"],
        "properties": {
            "_type": { "const": "delete_shape" },
            "targets": {
                "type": "array",
                "minItems": 1,
                "maxItems": MAX_TARGETS_PER_DELETE,
                "items": { "type": "string", "minLength": 1, "maxLength": 64 },
            },
        },
    });
    let layout = json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["_type", "mode", "targets"],
        "properties": {
            "_type": { "const": "layout" },
            "mode": enum_schema(&["grid", "row", "column"]),
            "targets": {
                "type": "array",
                "minItems": 2,
                "maxItems": MAX_LAYOUT_TARGETS,
                "items": { "type": "string", "minLength": 1, "maxLength": 64 },
            },
            "gap": number_schema(0.0, GAP_LIMIT),
            "columns": { "type": "integer", "minimum": 1, "maximum": 8 },
            "align": enum_schema(&["start", "center", "end"]),
            "origin": {
                "type": "object",
                "additionalProperties": false,
                "required": ["x", "y"],
                "properties": {
                    "x": number_schema(-COORD_LIMIT, COORD_LIMIT),
                    "y": number_schema(-COORD_LIMIT, COORD_LIMIT),
                },
            },
        },
    });
    // parent 为必填：frame 的 ref/shapeId，或字面量 "page"（移回页面根）。
    let reparent = json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["_type", "targets", "parent"],
        "properties": {
            "_type": { "const": "reparent" },
            "targets": {
                "type": "array",
                "minItems": 1,
                "maxItems": MAX_REPARENT_TARGETS,
                "items": { "type": "string", "minLength": 1, "maxLength": 64 },
            },
            "parent": { "type": "string", "minLength": 1, "maxLength": 64 },
        },
    });
    let select_shapes = json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["_type", "targets"],
        "properties": {
            "_type": { "const": "select_shapes" },
            "targets": {
                "type": "array",
                "minItems": 1,
                "maxItems": MAX_SELECT_TARGETS,
                "items": { "type": "string", "minLength": 1, "maxLength": 64 },
            },
            "zoom": { "type": "boolean" },
        },
    });
    // mode=fit 不携带 point；mode=point 必须携带——由权威语义校验保证，
    // schema 层不用 oneOf 表达，避免模糊错误消息（同 move_shape 先例）。
    let camera = json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["_type", "mode"],
        "properties": {
            "_type": { "const": "camera" },
            "mode": enum_schema(&["fit", "point"]),
            "point": {
                "type": "object",
                "additionalProperties": false,
                "required": ["x", "y"],
                "properties": {
                    "x": number_schema(-COORD_LIMIT, COORD_LIMIT),
                    "y": number_schema(-COORD_LIMIT, COORD_LIMIT),
                },
            },
        },
    });

    // 各分支的样式属性经 style_properties()/arrow_style_properties() 合并，
    // 保持单一定义源。箭头只吃样式子集（tldraw 箭头无 fill/font/align）。
    let with_style = |mut branch: Value| {
        if let Some(properties) = branch.get_mut("properties").and_then(|p| p.as_object_mut()) {
            if let Some(style) = style_properties().as_object() {
                for (key, value) in style {
                    properties
                        .entry(key.clone())
                        .or_insert_with(|| value.clone());
                }
            }
        }
        branch
    };
    let with_arrow_style = |mut branch: Value| {
        if let Some(properties) = branch.get_mut("properties").and_then(|p| p.as_object_mut()) {
            if let Some(style) = arrow_style_properties().as_object() {
                for (key, value) in style {
                    properties
                        .entry(key.clone())
                        .or_insert_with(|| value.clone());
                }
            }
        }
        branch
    };

    let instruction = json!({
        "oneOf": [
            with_style(create_shape),
            with_arrow_style(create_arrow),
            with_style(update_shape),
            with_arrow_style(update_arrow),
            move_shape,
            delete_shape,
            layout,
            reparent,
            select_shapes,
            camera,
        ],
    });

    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["program"],
        "properties": {
            "program": {
                "type": "object",
                "additionalProperties": false,
                "required": ["version", "instructions"],
                "properties": {
                    "version": { "const": ARCH_PROGRAM_VERSION },
                    "instructions": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": ARCH_MAX_INSTRUCTIONS,
                        "items": { "$ref": "#/$defs/instruction" },
                    },
                },
            },
        },
        "$defs": { "instruction": instruction },
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::architecture_run_parameters_schema;
    use crate::agent::agents::architecture::program::ARCH_PROGRAM_VERSION;

    fn compile() -> jsonschema::Validator {
        let schema = architecture_run_parameters_schema();
        jsonschema::draft202012::new(&schema).expect("schema 应可编译")
    }

    #[test]
    fn schema_is_valid_jsonschema_and_rejects_unknown_instruction() {
        let validator = compile();
        let good = json!({
            "program": {
                "version": ARCH_PROGRAM_VERSION,
                "instructions": [
                    { "_type": "create_shape", "ref": "a", "shape": "note", "text": "服务" },
                ],
            },
        });
        assert!(validator.is_valid(&good));

        let unknown_instruction = json!({
            "program": {
                "version": ARCH_PROGRAM_VERSION,
                "instructions": [{ "_type": "explode_shape", "ref": "a" }],
            },
        });
        assert!(!validator.is_valid(&unknown_instruction));

        let unknown_field = json!({
            "program": {
                "version": ARCH_PROGRAM_VERSION,
                "instructions": [
                    { "_type": "create_shape", "ref": "a", "shape": "note", "surprise": 1 },
                ],
            },
        });
        assert!(!validator.is_valid(&unknown_field));
    }

    #[test]
    fn schema_accepts_update_arrow_and_partial_move() {
        let validator = compile();
        let good = json!({
            "program": {
                "version": ARCH_PROGRAM_VERSION,
                "instructions": [
                    { "_type": "update_arrow", "target": "shape:abc", "labelPosition": 0.3 },
                    { "_type": "update_arrow", "target": "shape:abc", "label": "", "dash": "dashed" },
                    { "_type": "move_shape", "target": "shape:abc", "x": 12.0 },
                    { "_type": "move_shape", "target": "shape:abc", "dy": -8.0 },
                ],
            },
        });
        assert!(validator.is_valid(&good));
    }

    #[test]
    fn schema_rejects_arrow_only_fields_on_update_shape() {
        // 模型最常见的错位：拿 update_shape 改箭头字段。schema 必须拒绝，
        // 且 oneOf 的失败子错误能精确指向多出/缺失的字段（注册表层展开）。
        let validator = compile();
        let bad = json!({
            "program": {
                "version": ARCH_PROGRAM_VERSION,
                "instructions": [
                    { "_type": "update_shape", "labelPosition": 0.3, "target": "shape:abc" },
                ],
            },
        });
        let errors: Vec<_> = validator.iter_errors(&bad).collect();
        assert_eq!(errors.len(), 1);
        let message = errors[0].to_string();
        assert!(message.contains("oneOf"), "{message}");
        // 最接近分支（update_shape）的子错误点出 labelPosition 不合法。
        if let jsonschema::error::ValidationErrorKind::OneOfNotValid { context } = errors[0].kind()
        {
            let closest = context
                .iter()
                .filter(|c| !c.is_empty())
                .min_by_key(|c| c.len());
            let detail = closest
                .map(|branch| {
                    branch
                        .iter()
                        .map(|e| e.to_string())
                        .collect::<Vec<_>>()
                        .join("; ")
                })
                .unwrap_or_default();
            assert!(detail.contains("labelPosition"), "{detail}");
        } else {
            panic!("期望 OneOfNotValid 错误");
        }
    }

    #[test]
    fn schema_accepts_reparent_select_and_camera() {
        let validator = compile();
        let good = json!({
            "program": {
                "version": ARCH_PROGRAM_VERSION,
                "instructions": [
                    { "_type": "create_shape", "ref": "svc", "shape": "note", "into": "shape:frame1" },
                    { "_type": "reparent", "targets": ["shape:abc"], "parent": "shape:frame1" },
                    { "_type": "reparent", "targets": ["shape:abc", "shape:def"], "parent": "page" },
                    { "_type": "select_shapes", "targets": ["shape:abc"], "zoom": true },
                    { "_type": "camera", "mode": "fit" },
                    { "_type": "camera", "mode": "point", "point": { "x": 10.0, "y": 20.0 } },
                ],
            },
        });
        assert!(validator.is_valid(&good));

        // parent 为必填字段（缺失即拒绝）。
        let missing_parent = json!({
            "program": {
                "version": ARCH_PROGRAM_VERSION,
                "instructions": [
                    { "_type": "reparent", "targets": ["shape:abc"] },
                ],
            },
        });
        assert!(!validator.is_valid(&missing_parent));
    }

    #[test]
    fn schema_rejects_out_of_range_values() {
        let validator = compile();
        let bad = json!({
            "program": {
                "version": ARCH_PROGRAM_VERSION,
                "instructions": [
                    { "_type": "move_shape", "target": "shape:abc", "dx": 99999.0 },
                ],
            },
        });
        assert!(!validator.is_valid(&bad));
    }
}
