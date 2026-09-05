//! 架构画布程序（architecture_run 工具载荷）：模块入口与契约常量。
//!
//! 与 `tools/program/ast.rs` 同一模式：**Rust 反序列化 + `validate_program`
//! 才是权威校验**；随工具定义下发的 JSON Schema（见 `program_schema.rs`）
//! 用于尽早约束模型输出。语义层校验（别名解析、形状存在性、几何计算）
//! 由前端画布解释器完成。
//!
//! 拆分布局（原单文件超 500 行红线后按「变化原因」拆分）：
//! - `program_ast.rs` —— AST 类型（程序信封、指令枚举、样式枚举、指令结构体）；
//! - `program_validate.rs` —— Rust 侧权威语义校验（`validate_program`）；
//! - 本文件保留 schema 与校验共用的契约常量与全部测试，并统一再导出，
//!   外部调用方（`architecture_run` 工具、`program_schema.rs`）的导入路径不变。

// AST 类型经 glob 全量再导出，保持 `program::` 为单一入口
// （外部调用方无需感知 `program_ast` / `program_validate` 的拆分）。
pub use super::program_ast::*;
pub use super::program_validate::validate_program;

pub const ARCH_PROGRAM_VERSION: u8 = 1;
/// 单程序指令数上限（与系统提示词约定一致）。
pub const ARCH_MAX_INSTRUCTIONS: usize = 40;

/// 几何/数值上限：`program_schema.rs` 的 JSON Schema 与 `program_validate.rs`
/// 的语义校验共用同一组常量，保持模型契约与权威校验一致。
pub(crate) const COORD_LIMIT: f64 = 10_000.0;
pub(crate) const NUDGE_LIMIT: f64 = 5_000.0;
pub(crate) const SIZE_LIMIT: f64 = 2_000.0;
pub(crate) const GAP_LIMIT: f64 = 500.0;
pub(crate) const MAX_TEXT_LEN: usize = 500;
pub(crate) const MAX_LABEL_LEN: usize = 200;
pub(crate) const MAX_TARGETS_PER_DELETE: usize = 20;
pub(crate) const MAX_LAYOUT_TARGETS: usize = 40;
pub(crate) const MAX_REPARENT_TARGETS: usize = 20;
pub(crate) const MAX_SELECT_TARGETS: usize = 30;
/// `reparent` 指令中表示「移回页面根」的字面量（避免 null 语义歧义）。
pub const REPARENT_PAGE_LITERAL: &str = "page";
/// 校验报告一次最多列出的错误条数：全量收集、一次报全，
/// 让模型一轮重试就能修完所有问题，而不是逐条挤牙膏。
pub(crate) const MAX_REPORTED_ERRORS: usize = 8;

// JSON Schema（模型参数契约）已拆分至 `program_schema.rs`。

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{validate_program, ArchProgram};

    fn parse(value: serde_json::Value) -> ArchProgram {
        serde_json::from_value(value).expect("程序应可解析")
    }

    fn shape_program(instructions: serde_json::Value) -> serde_json::Value {
        json!({ "version": 1, "instructions": instructions })
    }

    #[test]
    fn serde_round_trip_preserves_program() {
        let program = parse(shape_program(json!([
            {
                "_type": "create_shape",
                "ref": "gateway",
                "shape": "geo",
                "geo": "rectangle",
                "text": "API Gateway",
                "color": "light-blue",
                "fill": "semi",
            },
            {
                "_type": "create_arrow",
                "ref": "link",
                "from": "gateway",
                "to": "shape:abc12345",
                "label": "REST",
                "kind": "elbow",
            },
            {
                "_type": "update_arrow",
                "target": "link",
                "label": "REST/JSON",
                "labelPosition": 0.3,
            },
            { "_type": "layout", "mode": "grid", "targets": ["gateway", "shape:abc12345"], "columns": 2 },
        ])));
        assert!(validate_program(&program).is_ok());
        let round_trip: ArchProgram =
            serde_json::from_value(serde_json::to_value(&program).unwrap()).unwrap();
        assert_eq!(program, round_trip);
    }

    #[test]
    fn geo_rhombus2_matches_schema_spelling() {
        // kebab-case 不会为数字补连字符：Rhombus2 显式 rename 为 "rhombus-2"
        //（与 program_schema.rs / tldraw 取值一致），否则模型按 schema 输出
        // 的 "rhombus-2" 会反序列化失败且错误消息只列出 "rhombus2"。
        let program = parse(shape_program(json!([
            { "_type": "create_shape", "ref": "a", "shape": "geo", "geo": "rhombus-2" },
        ])));
        assert!(validate_program(&program).is_ok());
        let serialized = serde_json::to_value(&program).unwrap();
        assert_eq!(serialized["instructions"][0]["geo"], json!("rhombus-2"));
    }

    #[test]
    fn rejects_frame_into_but_allows_shape_into() {
        // frame 只能位于页面根：frame + into = 嵌套 frame，与前端防御层同口径。
        let nested = parse(shape_program(json!([
            { "_type": "create_shape", "ref": "a", "shape": "frame", "into": "shape:outer" },
        ])));
        assert!(validate_program(&nested).is_err());

        let allowed = parse(shape_program(json!([
            { "_type": "create_shape", "ref": "a", "shape": "note", "into": "shape:outer" },
        ])));
        assert!(validate_program(&allowed).is_ok());
    }

    #[test]
    fn serialization_never_emits_null_for_absent_optionals() {
        // 模型常把省略字段写成显式 null；serde 将其归一为 None，
        // 序列化必须彻底省略这些键——前端解释器按 `!== undefined` 判定
        // 可选字段，载荷里出现 null 会直接击穿 tldraw 形状校验。
        let program = parse(shape_program(json!([
            {
                "_type": "create_shape",
                "ref": "a",
                "shape": "note",
                "text": null,
                "x": null,
                "y": null,
                "w": null,
                "h": null,
                "color": null,
            },
            {
                "_type": "create_arrow",
                "ref": null,
                "from": "a",
                "to": "shape:abc12345",
                "label": null,
                "labelPosition": null,
                "kind": null,
            },
            {
                "_type": "update_arrow",
                "target": "shape:abc12345",
                "label": null,
                "labelPosition": 0.35,
                "kind": null,
                "color": null,
            },
            { "_type": "layout", "mode": "grid", "targets": ["a", "shape:abc12345"], "gap": null, "origin": null },
        ])));
        assert!(validate_program(&program).is_ok());
        let serialized = serde_json::to_value(&program).unwrap();
        let text = serialized.to_string();
        assert!(!text.contains("null"), "序列化不应携带 null：{text}");
        assert!(serialized["instructions"][1].get("labelPosition").is_none());
        assert!(serialized["instructions"][2].get("label").is_none());
    }

    #[test]
    fn rejects_wrong_version_unknown_type_and_unknown_fields() {
        let wrong_version = serde_json::from_value::<ArchProgram>(json!({
            "version": 2,
            "instructions": [{ "_type": "delete_shape", "targets": ["shape:x"] }],
        }))
        .unwrap();
        assert!(validate_program(&wrong_version).is_err());

        assert!(serde_json::from_value::<ArchProgram>(shape_program(json!([
            { "_type": "explode_shape", "ref": "a" },
        ])))
        .is_err());

        // deny_unknown_fields：顶层信封不允许未知字段。
        assert!(serde_json::from_value::<ArchProgram>(json!({
            "version": 1,
            "instructions": [],
            "surprise": true,
        }))
        .is_err());

        // 指令变体内部的未知字段同样拒绝（内部 tag 枚举 + deny_unknown_fields）。
        assert!(serde_json::from_value::<ArchProgram>(shape_program(json!([
            { "_type": "create_shape", "ref": "a", "shape": "note", "surprise": 1 },
        ])))
        .is_err());
    }

    #[test]
    fn rejects_duplicate_refs_and_missing_geo() {
        let duplicate = parse(shape_program(json!([
            { "_type": "create_shape", "ref": "a", "shape": "note" },
            { "_type": "create_shape", "ref": "a", "shape": "note" },
        ])));
        assert!(validate_program(&duplicate).is_err());

        let missing_geo = parse(shape_program(json!([
            { "_type": "create_shape", "ref": "a", "shape": "geo" },
        ])));
        assert!(validate_program(&missing_geo).is_err());
    }

    #[test]
    fn rejects_empty_ref_alias() {
        // 空串 ref：空迭代器上的 all() 因 vacuous truth 恒真，权威校验必须
        // 显式拒绝，不能依赖 schema 层 pattern 兜底。
        let empty_ref = parse(shape_program(json!([
            { "_type": "create_shape", "ref": "", "shape": "note" },
        ])));
        assert!(validate_program(&empty_ref).is_err());
    }

    #[test]
    fn enforces_size_bounds_consistent_with_schema() {
        // 与 program_schema.rs 的 number_schema(1.0, SIZE_LIMIT) 同区间 [1, 2000]：
        // (0,1) 亚像素尺寸拒绝（两层契约曾经漂移）。
        let too_small = parse(shape_program(json!([
            { "_type": "create_shape", "ref": "a", "shape": "geo", "geo": "rectangle", "w": 0.5 },
        ])));
        assert!(validate_program(&too_small).is_err());

        let too_large = parse(shape_program(json!([
            { "_type": "create_shape", "ref": "a", "shape": "geo", "geo": "rectangle", "h": 2001.0 },
        ])));
        assert!(validate_program(&too_large).is_err());

        let lower_edge = parse(shape_program(json!([
            { "_type": "create_shape", "ref": "a", "shape": "geo", "geo": "rectangle", "w": 1.0 },
        ])));
        assert!(validate_program(&lower_edge).is_ok());
    }

    #[test]
    fn move_allows_partial_axes_but_not_mixed_modes() {
        // 单轴合法：省略的轴保持不变（模型不必复述快照里的另一轴坐标）。
        let absolute_x_only = parse(shape_program(json!([
            { "_type": "move_shape", "target": "shape:x", "x": 10.0 },
        ])));
        assert!(validate_program(&absolute_x_only).is_ok());

        let relative_dy_only = parse(shape_program(json!([
            { "_type": "move_shape", "target": "shape:x", "dy": -4.0 },
        ])));
        assert!(validate_program(&relative_dy_only).is_ok());

        // 绝对与相对两族互斥。
        let mixed_all = parse(shape_program(json!([
            { "_type": "move_shape", "target": "shape:x", "x": 10.0, "y": 10.0, "dx": 5.0, "dy": 5.0 },
        ])));
        assert!(validate_program(&mixed_all).is_err());

        let mixed_half = parse(shape_program(json!([
            { "_type": "move_shape", "target": "shape:x", "x": 10.0, "dy": 3.0 },
        ])));
        assert!(validate_program(&mixed_half).is_err());

        // 至少给出一个位移字段。
        let no_fields = parse(shape_program(json!([
            { "_type": "move_shape", "target": "shape:x" },
        ])));
        assert!(validate_program(&no_fields).is_err());
    }

    #[test]
    fn create_shape_requires_both_or_neither_xy() {
        let half_position = parse(shape_program(json!([
            { "_type": "create_shape", "ref": "a", "shape": "note", "x": 10.0 },
        ])));
        let error = validate_program(&half_position).unwrap_err();
        assert!(error.contains("同时给出或同时省略"));

        let both = parse(shape_program(json!([
            { "_type": "create_shape", "ref": "a", "shape": "note", "x": 10.0, "y": 20.0 },
        ])));
        assert!(validate_program(&both).is_ok());

        let neither = parse(shape_program(json!([
            { "_type": "create_shape", "ref": "a", "shape": "note" },
        ])));
        assert!(validate_program(&neither).is_ok());
    }

    #[test]
    fn update_arrow_rules() {
        let ok = parse(shape_program(json!([
            { "_type": "update_arrow", "target": "shape:abc", "labelPosition": 0.3, "color": "red" },
        ])));
        assert!(validate_program(&ok).is_ok());

        let clear_label = parse(shape_program(json!([
            { "_type": "update_arrow", "target": "shape:abc", "label": "" },
        ])));
        assert!(validate_program(&clear_label).is_ok());

        let empty_update = parse(shape_program(json!([
            { "_type": "update_arrow", "target": "shape:abc" },
        ])));
        assert!(validate_program(&empty_update).is_err());

        let bad_position = parse(shape_program(json!([
            { "_type": "update_arrow", "target": "shape:abc", "labelPosition": 1.5 },
        ])));
        assert!(validate_program(&bad_position).is_err());
    }

    #[test]
    fn collects_all_errors_in_one_report() {
        // 三处不同错误必须一次报全，而不是逐条重试。
        let program = parse(shape_program(json!([
            { "_type": "create_shape", "ref": "a", "shape": "geo" },
            { "_type": "create_shape", "ref": "a", "shape": "note" },
            { "_type": "move_shape", "target": "shape:x", "x": 1.0, "dx": 2.0, "dy": 2.0 },
        ])));
        let error = validate_program(&program).unwrap_err();
        assert!(error.starts_with("错误："));
        assert!(error.contains("geo 类型"), "{error}");
        assert!(error.contains("重复"), "{error}");
        assert!(error.contains("二选一"), "{error}");
    }

    #[test]
    fn enforces_instruction_limit_and_arrow_self_link() {
        // serde 层不限制条数（Vec 无 maxItems），上限由 validate_program 强制。
        let too_many = parse(json!({
            "version": 1,
            "instructions": (0..41)
                .map(|_| json!({ "_type": "delete_shape", "targets": ["shape:x"] }))
                .collect::<Vec<_>>(),
        }));
        assert!(validate_program(&too_many).is_err());

        let self_link = parse(shape_program(json!([
            { "_type": "create_arrow", "from": "a", "to": "a" },
        ])));
        assert!(validate_program(&self_link).is_err());
    }

    #[test]
    fn layout_columns_only_for_grid() {
        let row_with_columns = parse(shape_program(json!([
            { "_type": "layout", "mode": "row", "targets": ["a", "b"], "columns": 2 },
        ])));
        assert!(validate_program(&row_with_columns).is_err());
    }

    #[test]
    fn reparent_select_camera_round_trip_and_rules() {
        // 三条新指令的合法形态：serde 往返 + 权威校验通过。
        let program = parse(shape_program(json!([
            { "_type": "create_shape", "ref": "box", "shape": "frame", "into": null },
            { "_type": "create_shape", "ref": "svc", "shape": "note", "text": "服务", "into": "box" },
            { "_type": "reparent", "targets": ["shape:abc12345"], "parent": "box" },
            { "_type": "reparent", "targets": ["svc"], "parent": "page" },
            { "_type": "select_shapes", "targets": ["box", "shape:abc12345"], "zoom": true },
            { "_type": "camera", "mode": "fit" },
            { "_type": "camera", "mode": "point", "point": { "x": 120.0, "y": -40.0 } },
        ])));
        assert!(validate_program(&program).is_ok());
        let round_trip: ArchProgram =
            serde_json::from_value(serde_json::to_value(&program).unwrap()).unwrap();
        assert_eq!(program, round_trip);
    }

    #[test]
    fn reparent_requires_parent_and_bounds_targets() {
        // parent 为必填字段：省略 = 反序列化失败（与 "page" 语义严格区分）。
        assert!(serde_json::from_value::<ArchProgram>(shape_program(json!([
            { "_type": "reparent", "targets": ["shape:x"] },
        ])))
        .is_err());

        let empty_parent = parse(shape_program(json!([
            { "_type": "reparent", "targets": ["shape:x"], "parent": "" },
        ])));
        assert!(validate_program(&empty_parent).is_err());

        let too_many = parse(shape_program(json!([
            {
                "_type": "reparent",
                "targets": (0..21).map(|i| format!("shape:t{i}")).collect::<Vec<_>>(),
                "parent": "page",
            },
        ])));
        assert!(validate_program(&too_many).is_err());
    }

    #[test]
    fn select_shapes_bounds_and_camera_mode_rules() {
        let select_ok = parse(shape_program(json!([
            { "_type": "select_shapes", "targets": ["shape:x"] },
        ])));
        assert!(validate_program(&select_ok).is_ok());

        let select_empty = parse(shape_program(json!([
            { "_type": "select_shapes", "targets": [] },
        ])));
        assert!(validate_program(&select_empty).is_err());

        let too_many = parse(shape_program(json!([
            {
                "_type": "select_shapes",
                "targets": (0..31).map(|i| format!("shape:t{i}")).collect::<Vec<_>>(),
            },
        ])));
        assert!(validate_program(&too_many).is_err());

        // fit 不允许携带 point；point 模式必须给出坐标。
        let fit_with_point = parse(shape_program(json!([
            { "_type": "camera", "mode": "fit", "point": { "x": 0.0, "y": 0.0 } },
        ])));
        assert!(validate_program(&fit_with_point).is_err());

        let point_without_coord = parse(shape_program(json!([
            { "_type": "camera", "mode": "point" },
        ])));
        assert!(validate_program(&point_without_coord).is_err());

        let point_out_of_range = parse(shape_program(json!([
            { "_type": "camera", "mode": "point", "point": { "x": 999999.0, "y": 0.0 } },
        ])));
        assert!(validate_program(&point_out_of_range).is_err());
    }
}
