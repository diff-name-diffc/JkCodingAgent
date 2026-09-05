//! 架构画布程序（architecture_run 工具载荷）的类型化 AST：指令结构体与样式枚举。
//!
//! 与 `tools/program/ast.rs` 同一模式：**Rust 反序列化 + `validate_program`
//! （`program_validate.rs`）才是权威校验**；随工具定义下发的 JSON Schema
//! （见 `program_schema.rs`）用于尽早约束模型输出。语义层校验（别名解析、
//! 形状存在性、几何计算）由前端画布解释器完成。
//!
//! 模块分工：`program.rs` 为入口（契约常量、测试与统一再导出），
//! 权威语义校验在 `program_validate.rs`。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ArchProgram {
    pub version: u8,
    pub instructions: Vec<ArchInstruction>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "_type", rename_all = "snake_case")]
pub enum ArchInstruction {
    CreateShape(CreateShapeInst),
    CreateArrow(CreateArrowInst),
    UpdateShape(UpdateShapeInst),
    UpdateArrow(UpdateArrowInst),
    MoveShape(MoveShapeInst),
    DeleteShape(DeleteShapeInst),
    Layout(LayoutInst),
    Reparent(ReparentInst),
    SelectShapes(SelectShapesInst),
    Camera(CameraInst),
}

// ── 样式枚举：kebab-case 序列化与 tldraw 样式取值逐字一致，前端直传 ──

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ArchColor {
    Black,
    Grey,
    LightViolet,
    Violet,
    Blue,
    LightBlue,
    Yellow,
    Orange,
    Green,
    LightGreen,
    LightRed,
    Red,
    White,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ArchFill {
    None,
    Semi,
    Solid,
    Pattern,
    Fill,
    LinedFill,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ArchSize {
    S,
    M,
    L,
    Xl,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ArchDash {
    Draw,
    Solid,
    Dashed,
    Dotted,
    None,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ArchFont {
    Draw,
    Sans,
    Serif,
    Mono,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ArchAlign {
    Start,
    Middle,
    End,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ArchGeo {
    Rectangle,
    Ellipse,
    Triangle,
    Diamond,
    Pentagon,
    Hexagon,
    Octagon,
    Star,
    Rhombus,
    // kebab-case 不会在数字前补连字符，需显式指定与 schema/tldraw 一致的取值。
    #[serde(rename = "rhombus-2")]
    Rhombus2,
    Oval,
    Cloud,
    Trapezoid,
    ArrowRight,
    ArrowLeft,
    ArrowUp,
    ArrowDown,
    XBox,
    CheckBox,
    Heart,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ArchShapeKind {
    Geo,
    Note,
    Text,
    Frame,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ArchArrowKind {
    Arc,
    Elbow,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ArchArrowhead {
    Arrow,
    Triangle,
    Square,
    Dot,
    Pipe,
    Diamond,
    Inverted,
    Bar,
    None,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ArchLayoutMode {
    Grid,
    Row,
    Column,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ArchLayoutAlign {
    Start,
    Center,
    End,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ArchCameraMode {
    Fit,
    Point,
}

// ── 指令结构 ──

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateShapeInst {
    /// 程序内别名（`ref` 为 Rust 关键字，序列化字段名保持 `ref`）。
    #[serde(rename = "ref")]
    pub ref_alias: String,
    pub shape: ArchShapeKind,
    /// shape=geo 时必填。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geo: Option<ArchGeo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// 省略时由前端执行器自动放置（首个形状视口中心，其后依次右移）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub w: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub h: Option<f64>,
    /// 直接把新形状放进该 frame（ref 或 shapeId，目标必须是已存在的 frame）。
    /// 坐标语义保持页面绝对坐标：先在页面级创建，再移入容器（位置不变）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub into: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<ArchColor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_color: Option<ArchColor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<ArchFill>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<ArchSize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dash: Option<ArchDash>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font: Option<ArchFont>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align: Option<ArchAlign>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateArrowInst {
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    pub ref_alias: Option<String>,
    /// 已声明的别名或画布快照中的 shapeId。
    pub from: String,
    pub to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_position: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<ArchArrowKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arrowhead_start: Option<ArchArrowhead>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arrowhead_end: Option<ArchArrowhead>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<ArchColor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_color: Option<ArchColor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<ArchSize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dash: Option<ArchDash>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateShapeInst {
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub w: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub h: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<ArchColor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_color: Option<ArchColor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<ArchFill>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<ArchSize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dash: Option<ArchDash>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font: Option<ArchFont>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align: Option<ArchAlign>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateArrowInst {
    pub target: String,
    /// 设为空串表示清除箭头标注。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_position: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<ArchArrowKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arrowhead_start: Option<ArchArrowhead>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arrowhead_end: Option<ArchArrowhead>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<ArchColor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_color: Option<ArchColor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<ArchSize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dash: Option<ArchDash>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MoveShapeInst {
    pub target: String,
    /// 绝对坐标（与 dx/dy 互斥）；可只给一个轴，未给出的轴保持不变。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<f64>,
    /// 相对位移（与 x/y 互斥）；同样可只给一个轴。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dx: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dy: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeleteShapeInst {
    pub targets: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ArchPoint {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LayoutInst {
    pub mode: ArchLayoutMode,
    pub targets: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap: Option<f64>,
    /// 仅 grid 有效。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub columns: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align: Option<ArchLayoutAlign>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<ArchPoint>,
}

/// 把已有形状移入/移出 frame 容器。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReparentInst {
    pub targets: Vec<String>,
    /// 目标容器：frame 的 ref/shapeId；字面量 `"page"` 表示移回页面根。
    /// 必填字段——省略与 `"page"` 语义不同，不允许模型含糊。
    pub parent: String,
}

/// 选中形状（让用户看到 Agent 指的是谁），可选顺带缩放至选中区域。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SelectShapesInst {
    pub targets: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zoom: Option<bool>,
}

/// 相机导航：`fit` 缩放到全部内容；`point` 居中到指定页面坐标。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CameraInst {
    pub mode: ArchCameraMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub point: Option<ArchPoint>,
}
