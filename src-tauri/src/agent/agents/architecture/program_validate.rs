//! 架构画布程序的 Rust 侧权威语义校验。
//!
//! 校验消息不带「错误：」前缀、不以句号结尾——`validate_program` 负责把
//! 多条错误用「；」拼接并统一加前缀/收尾标点。错误一次全量报出（上限
//! `MAX_REPORTED_ERRORS`），让模型单轮重试即可修完，避免逐条挤牙膏式重试。
//!
//! 数值上限常量由 `program.rs` 单点定义（`program_schema.rs` 的 JSON Schema
//! 与本文件共用），保持模型契约与权威校验一致。

use std::collections::HashSet;

use super::program::{
    ARCH_MAX_INSTRUCTIONS, ARCH_PROGRAM_VERSION, COORD_LIMIT, GAP_LIMIT, MAX_LABEL_LEN,
    MAX_LAYOUT_TARGETS, MAX_REPARENT_TARGETS, MAX_REPORTED_ERRORS, MAX_SELECT_TARGETS,
    MAX_TARGETS_PER_DELETE, MAX_TEXT_LEN, NUDGE_LIMIT, REPARENT_PAGE_LITERAL, SIZE_LIMIT,
};
use super::program_ast::{
    ArchCameraMode, ArchInstruction, ArchLayoutMode, ArchProgram, ArchShapeKind,
};

fn check_ref(ref_alias: &str, refs: &mut HashSet<String>, index: usize) -> Result<(), String> {
    // 空串必须显式拒绝：空迭代器上的 all() 因 vacuous truth 恒真，
    // 权威校验不能依赖 schema 层的 pattern 兜底。
    let valid = !ref_alias.is_empty()
        && ref_alias.len() <= 32
        && ref_alias.chars().enumerate().all(|(i, c)| {
            if i == 0 {
                c.is_ascii_alphabetic()
            } else {
                c.is_ascii_alphanumeric() || c == '_' || c == '-'
            }
        });
    if !valid {
        return Err(format!(
            "第 {} 条指令的 ref「{}」不合法（需字母开头，仅含字母/数字/_/-，≤32 字符）",
            index + 1,
            ref_alias
        ));
    }
    if !refs.insert(ref_alias.to_string()) {
        return Err(format!(
            "第 {} 条指令的 ref「{}」与前面的指令重复",
            index + 1,
            ref_alias
        ));
    }
    Ok(())
}

fn check_target(target: &str, index: usize) -> Result<(), String> {
    // 与 check_text / JSON Schema maxLength 同口径按字符（码点）计数：
    // 字节数会让非 ASCII 引用在 Rust 层被拒、schema 层放行，两层契约漂移。
    if target.is_empty() || target.chars().count() > 64 {
        return Err(format!(
            "第 {} 条指令的目标引用为空或超过 64 字符",
            index + 1
        ));
    }
    Ok(())
}

fn check_coord(value: Option<f64>, index: usize, field: &str) -> Result<(), String> {
    if let Some(v) = value {
        if !v.is_finite() || v.abs() > COORD_LIMIT {
            return Err(format!(
                "第 {} 条指令的 {} 超出范围 [-{:.0}, {:.0}]",
                index + 1,
                field,
                COORD_LIMIT,
                COORD_LIMIT
            ));
        }
    }
    Ok(())
}

fn check_size(value: Option<f64>, index: usize, field: &str) -> Result<(), String> {
    if let Some(v) = value {
        // 与 program_schema.rs 的 number_schema(1.0, SIZE_LIMIT) 同区间：
        // (0,1) 的亚像素尺寸无意义，两层契约必须一致（曾经漂移）。
        if !v.is_finite() || v < 1.0 || v > SIZE_LIMIT {
            return Err(format!(
                "第 {} 条指令的 {} 必须在 [1, {:.0}] 内",
                index + 1,
                field,
                SIZE_LIMIT
            ));
        }
    }
    Ok(())
}

fn check_text(value: &Option<String>, index: usize, max_len: usize) -> Result<(), String> {
    if let Some(text) = value {
        if text.chars().count() > max_len {
            return Err(format!(
                "第 {} 条指令的文本超过 {} 字符上限",
                index + 1,
                max_len
            ));
        }
    }
    Ok(())
}

fn check_label_position(value: Option<f64>, index: usize) -> Result<(), String> {
    if let Some(position) = value {
        if !(0.0..=1.0).contains(&position) {
            return Err(format!(
                "第 {} 条指令的 labelPosition 必须在 [0, 1] 内",
                index + 1
            ));
        }
    }
    Ok(())
}

/// 把收集到的校验错误拼成单条消息：`错误：…；…。`
/// 超过 `MAX_REPORTED_ERRORS` 条时附「另有 N 处未列出」。
fn join_errors(errors: Vec<String>) -> String {
    let total = errors.len();
    let mut message = errors
        .into_iter()
        .take(MAX_REPORTED_ERRORS)
        .collect::<Vec<_>>()
        .join("；");
    if total > MAX_REPORTED_ERRORS {
        message.push_str(&format!(
            "；…另有 {} 处错误未列出",
            total - MAX_REPORTED_ERRORS
        ));
    }
    format!("错误：{message}。")
}

/// Rust 侧权威语义校验：结构之外的跨指令规则（refs 唯一、move 互斥、
/// 布局目标数等）。**全量收集**所有错误后一次性返回（上限
/// `MAX_REPORTED_ERRORS`），失败消息以「错误：」开头，工具直接回传给模型。
pub fn validate_program(program: &ArchProgram) -> Result<(), String> {
    if program.version != ARCH_PROGRAM_VERSION {
        return Err(format!(
            "错误：画布程序版本必须为 {}，收到 {}。",
            ARCH_PROGRAM_VERSION, program.version
        ));
    }
    if program.instructions.is_empty() {
        return Err("错误：画布程序至少需要一条指令。".to_string());
    }
    if program.instructions.len() > ARCH_MAX_INSTRUCTIONS {
        return Err(format!(
            "错误：画布程序最多 {} 条指令，收到 {} 条。",
            ARCH_MAX_INSTRUCTIONS,
            program.instructions.len()
        ));
    }

    let mut errors: Vec<String> = Vec::new();
    let mut refs = HashSet::new();
    for (index, instruction) in program.instructions.iter().enumerate() {
        match instruction {
            ArchInstruction::CreateShape(inst) => {
                if let Err(error) = check_ref(&inst.ref_alias, &mut refs, index) {
                    errors.push(error);
                }
                if inst.shape == ArchShapeKind::Geo && inst.geo.is_none() {
                    errors.push(format!(
                        "第 {} 条指令创建 geo 形状时必须给出 geo 类型",
                        index + 1
                    ));
                }
                // frame 只能位于页面根（与前端防御层/reparent 约束同口径）：
                // frame + into 等于嵌套 frame，必须拦截。
                if inst.shape == ArchShapeKind::Frame && inst.into.is_some() {
                    errors.push(format!(
                        "第 {} 条指令的 frame 只能位于页面根，不能使用 into",
                        index + 1
                    ));
                }
                if let Some(into) = &inst.into {
                    if let Err(error) = check_target(into, index) {
                        errors.push(error);
                    }
                }
                if inst.x.is_some() != inst.y.is_some() {
                    errors.push(format!(
                        "第 {} 条指令的 x 与 y 必须同时给出或同时省略（同时省略才会自动放置）",
                        index + 1
                    ));
                }
                if let Err(error) = check_text(&inst.text, index, MAX_TEXT_LEN) {
                    errors.push(error);
                }
                for (value, field) in [(inst.x, "x"), (inst.y, "y")] {
                    if let Err(error) = check_coord(value, index, field) {
                        errors.push(error);
                    }
                }
                for (value, field) in [(inst.w, "w"), (inst.h, "h")] {
                    if let Err(error) = check_size(value, index, field) {
                        errors.push(error);
                    }
                }
            }
            ArchInstruction::CreateArrow(inst) => {
                if let Some(ref_alias) = &inst.ref_alias {
                    if let Err(error) = check_ref(ref_alias, &mut refs, index) {
                        errors.push(error);
                    }
                }
                for target in [&inst.from, &inst.to] {
                    if let Err(error) = check_target(target, index) {
                        errors.push(error);
                    }
                }
                if inst.from == inst.to {
                    errors.push(format!("第 {} 条指令的箭头起点与终点相同", index + 1));
                }
                if let Err(error) = check_text(&inst.label, index, MAX_LABEL_LEN) {
                    errors.push(error);
                }
                if let Err(error) = check_label_position(inst.label_position, index) {
                    errors.push(error);
                }
            }
            ArchInstruction::UpdateShape(inst) => {
                if let Err(error) = check_target(&inst.target, index) {
                    errors.push(error);
                }
                if let Err(error) = check_text(&inst.text, index, MAX_TEXT_LEN) {
                    errors.push(error);
                }
                for (value, field) in [(inst.x, "x"), (inst.y, "y")] {
                    if let Err(error) = check_coord(value, index, field) {
                        errors.push(error);
                    }
                }
                for (value, field) in [(inst.w, "w"), (inst.h, "h")] {
                    if let Err(error) = check_size(value, index, field) {
                        errors.push(error);
                    }
                }
                let has_any_update = inst.text.is_some()
                    || inst.x.is_some()
                    || inst.y.is_some()
                    || inst.w.is_some()
                    || inst.h.is_some()
                    || inst.color.is_some()
                    || inst.label_color.is_some()
                    || inst.fill.is_some()
                    || inst.size.is_some()
                    || inst.dash.is_some()
                    || inst.font.is_some()
                    || inst.align.is_some();
                if !has_any_update {
                    errors.push(format!("第 {} 条指令没有给出任何要更新的字段", index + 1));
                }
            }
            ArchInstruction::UpdateArrow(inst) => {
                if let Err(error) = check_target(&inst.target, index) {
                    errors.push(error);
                }
                if let Err(error) = check_text(&inst.label, index, MAX_LABEL_LEN) {
                    errors.push(error);
                }
                if let Err(error) = check_label_position(inst.label_position, index) {
                    errors.push(error);
                }
                let has_any_update = inst.label.is_some()
                    || inst.label_position.is_some()
                    || inst.kind.is_some()
                    || inst.arrowhead_start.is_some()
                    || inst.arrowhead_end.is_some()
                    || inst.color.is_some()
                    || inst.label_color.is_some()
                    || inst.size.is_some()
                    || inst.dash.is_some();
                if !has_any_update {
                    errors.push(format!("第 {} 条指令没有给出任何要更新的字段", index + 1));
                }
            }
            ArchInstruction::MoveShape(inst) => {
                if let Err(error) = check_target(&inst.target, index) {
                    errors.push(error);
                }
                // x/y 可只给一个轴（未给出的轴保持不变），但绝对坐标族
                // (x, y) 与相对位移族 (dx, dy) 互斥，且至少给出一个字段。
                let has_absolute = inst.x.is_some() || inst.y.is_some();
                let has_relative = inst.dx.is_some() || inst.dy.is_some();
                if has_absolute && has_relative {
                    errors.push(format!(
                        "第 {} 条指令不能混用绝对坐标 (x, y) 与相对位移 (dx, dy)，请二选一",
                        index + 1
                    ));
                } else if !has_absolute && !has_relative {
                    errors.push(format!(
                        "第 {} 条指令至少给出一个位移字段：绝对坐标 x/y 或相对位移 dx/dy（可只给一个轴）",
                        index + 1
                    ));
                } else {
                    for (value, field) in [(inst.x, "x"), (inst.y, "y")] {
                        if let Err(error) = check_coord(value, index, field) {
                            errors.push(error);
                        }
                    }
                    for (value, field) in [(inst.dx, "dx"), (inst.dy, "dy")] {
                        if let Some(v) = value {
                            if !v.is_finite() || v.abs() > NUDGE_LIMIT {
                                errors.push(format!(
                                    "第 {} 条指令的 {} 超出 [-{:.0}, {:.0}]",
                                    index + 1,
                                    field,
                                    NUDGE_LIMIT,
                                    NUDGE_LIMIT
                                ));
                            }
                        }
                    }
                }
            }
            ArchInstruction::DeleteShape(inst) => {
                if inst.targets.is_empty() || inst.targets.len() > MAX_TARGETS_PER_DELETE {
                    errors.push(format!(
                        "第 {} 条指令的删除目标数必须在 1~{} 内",
                        index + 1,
                        MAX_TARGETS_PER_DELETE
                    ));
                }
                for target in &inst.targets {
                    if let Err(error) = check_target(target, index) {
                        errors.push(error);
                    }
                }
            }
            ArchInstruction::Layout(inst) => {
                if inst.targets.len() < 2 || inst.targets.len() > MAX_LAYOUT_TARGETS {
                    errors.push(format!(
                        "第 {} 条指令的布局目标数必须在 2~{} 内",
                        index + 1,
                        MAX_LAYOUT_TARGETS
                    ));
                }
                for target in &inst.targets {
                    if let Err(error) = check_target(target, index) {
                        errors.push(error);
                    }
                }
                if let Some(gap) = inst.gap {
                    if !gap.is_finite() || !(0.0..=GAP_LIMIT).contains(&gap) {
                        errors.push(format!(
                            "第 {} 条指令的 gap 必须在 [0, {:.0}] 内",
                            index + 1,
                            GAP_LIMIT
                        ));
                    }
                }
                if let Some(columns) = inst.columns {
                    if inst.mode != ArchLayoutMode::Grid {
                        errors.push(format!("第 {} 条指令仅 grid 布局支持 columns", index + 1));
                    } else if !(1..=8).contains(&columns) {
                        errors.push(format!("第 {} 条指令的 columns 必须在 1~8 内", index + 1));
                    }
                }
                if let Some(origin) = inst.origin {
                    for (value, field) in
                        [(Some(origin.x), "origin.x"), (Some(origin.y), "origin.y")]
                    {
                        if let Err(error) = check_coord(value, index, field) {
                            errors.push(error);
                        }
                    }
                }
            }
            ArchInstruction::Reparent(inst) => {
                if inst.targets.is_empty() || inst.targets.len() > MAX_REPARENT_TARGETS {
                    errors.push(format!(
                        "第 {} 条指令的 reparent 目标数必须在 1~{} 内",
                        index + 1,
                        MAX_REPARENT_TARGETS
                    ));
                }
                for target in &inst.targets {
                    if let Err(error) = check_target(target, index) {
                        errors.push(error);
                    }
                }
                // 字面量 "page" 表示移回页面根；其余取值按普通目标引用校验。
                if inst.parent != REPARENT_PAGE_LITERAL {
                    if let Err(error) = check_target(&inst.parent, index) {
                        errors.push(error);
                    }
                }
            }
            ArchInstruction::SelectShapes(inst) => {
                if inst.targets.is_empty() || inst.targets.len() > MAX_SELECT_TARGETS {
                    errors.push(format!(
                        "第 {} 条指令的选中目标数必须在 1~{} 内",
                        index + 1,
                        MAX_SELECT_TARGETS
                    ));
                }
                for target in &inst.targets {
                    if let Err(error) = check_target(target, index) {
                        errors.push(error);
                    }
                }
            }
            ArchInstruction::Camera(inst) => match inst.mode {
                ArchCameraMode::Fit => {
                    if inst.point.is_some() {
                        errors.push(format!(
                            "第 {} 条指令 fit 模式不需要 point（fit 自动缩放至全部内容）",
                            index + 1
                        ));
                    }
                }
                ArchCameraMode::Point => match inst.point {
                    Some(point) => {
                        for (value, field) in
                            [(Some(point.x), "point.x"), (Some(point.y), "point.y")]
                        {
                            if let Err(error) = check_coord(value, index, field) {
                                errors.push(error);
                            }
                        }
                    }
                    None => {
                        errors.push(format!(
                            "第 {} 条指令 point 模式必须给出 point 坐标",
                            index + 1
                        ));
                    }
                },
            },
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(join_errors(errors))
    }
}
