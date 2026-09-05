/**
 * 画布程序（architecture_run 载荷）的前端类型与契约常量。
 *
 * 类型与 Rust 侧权威 AST（`src-tauri/src/agent/agents/architecture/program.rs`）
 * 一一对应，字段命名/枚举取值/校验规则必须同步维护。Rust 侧已做权威校验，
 * 前端的 `validateArchProgram`（拆分至 `arch-program-validate.ts`）是防御层：
 * 事件载荷异常时显式失败而非静默。
 */

export const ARCH_PROGRAM_VERSION = 1;
export const ARCH_MAX_INSTRUCTIONS = 40;

/** reparent 的页面根字面量（与 Rust `REPARENT_PAGE_LITERAL` 同值）。 */
export const REPARENT_PAGE_LITERAL = "page";

export type ArchColor =
  | "black"
  | "grey"
  | "light-violet"
  | "violet"
  | "blue"
  | "light-blue"
  | "yellow"
  | "orange"
  | "green"
  | "light-green"
  | "light-red"
  | "red"
  | "white";

export type ArchFill = "none" | "semi" | "solid" | "pattern" | "fill" | "lined-fill";
export type ArchSize = "s" | "m" | "l" | "xl";
export type ArchDash = "draw" | "solid" | "dashed" | "dotted" | "none";
export type ArchFont = "draw" | "sans" | "serif" | "mono";
export type ArchAlign = "start" | "middle" | "end";
export type ArchGeo =
  | "rectangle"
  | "ellipse"
  | "triangle"
  | "diamond"
  | "pentagon"
  | "hexagon"
  | "octagon"
  | "star"
  | "rhombus"
  | "rhombus-2"
  | "oval"
  | "cloud"
  | "trapezoid"
  | "arrow-right"
  | "arrow-left"
  | "arrow-up"
  | "arrow-down"
  | "x-box"
  | "check-box"
  | "heart";
export type ArchShapeKind = "geo" | "note" | "text" | "frame";
export type ArchArrowKind = "arc" | "elbow";
export type ArchArrowhead =
  | "arrow"
  | "triangle"
  | "square"
  | "dot"
  | "pipe"
  | "diamond"
  | "inverted"
  | "bar"
  | "none";
export type ArchLayoutMode = "grid" | "row" | "column";
export type ArchLayoutAlign = "start" | "center" | "end";

export interface ArchStyleProps {
  color?: ArchColor;
  labelColor?: ArchColor;
  fill?: ArchFill;
  size?: ArchSize;
  dash?: ArchDash;
  font?: ArchFont;
  align?: ArchAlign;
}

export interface ArchCreateShape extends ArchStyleProps {
  _type: "create_shape";
  ref: string;
  shape: ArchShapeKind;
  geo?: ArchGeo;
  text?: string;
  x?: number;
  y?: number;
  w?: number;
  h?: number;
  /** 直接把新形状放进该 frame（ref 或形状 id，目标必须是 frame）。 */
  into?: string;
}

export interface ArchCreateArrow extends ArchArrowStyleProps {
  _type: "create_arrow";
  ref?: string;
  from: string;
  to: string;
  label?: string;
  labelPosition?: number;
  kind?: ArchArrowKind;
  arrowheadStart?: ArchArrowhead;
  arrowheadEnd?: ArchArrowhead;
}

export interface ArchUpdateShape extends ArchStyleProps {
  _type: "update_shape";
  target: string;
  text?: string;
  x?: number;
  y?: number;
  w?: number;
  h?: number;
}

/** 仅箭头支持的样式子集（tldraw 箭头无 fill/font/align）。 */
export interface ArchArrowStyleProps {
  color?: ArchColor;
  labelColor?: ArchColor;
  size?: ArchSize;
  dash?: ArchDash;
}

export interface ArchUpdateArrow extends ArchArrowStyleProps {
  _type: "update_arrow";
  target: string;
  /** 空串表示清除箭头标注。 */
  label?: string;
  labelPosition?: number;
  kind?: ArchArrowKind;
  arrowheadStart?: ArchArrowhead;
  arrowheadEnd?: ArchArrowhead;
}

export interface ArchMoveShape {
  _type: "move_shape";
  target: string;
  /** 绝对坐标：可只给一个轴，未给出的轴保持不变。与 dx/dy 互斥。 */
  x?: number;
  y?: number;
  /** 相对位移：同样可只给一个轴。与 x/y 互斥。 */
  dx?: number;
  dy?: number;
}

export interface ArchDeleteShape {
  _type: "delete_shape";
  targets: string[];
}

export interface ArchLayout {
  _type: "layout";
  mode: ArchLayoutMode;
  targets: string[];
  gap?: number;
  columns?: number;
  align?: ArchLayoutAlign;
  origin?: { x: number; y: number };
}

export interface ArchReparent {
  _type: "reparent";
  targets: string[];
  /** 目标容器：frame 的 ref/形状 id；字面量 "page" 表示移回页面根。必填。 */
  parent: string;
}

export interface ArchSelectShapes {
  _type: "select_shapes";
  targets: string[];
  zoom?: boolean;
}

export type ArchCameraMode = "fit" | "point";

export interface ArchCamera {
  _type: "camera";
  mode: ArchCameraMode;
  /** mode=point 时必填。 */
  point?: { x: number; y: number };
}

export type ArchInstruction =
  | ArchCreateShape
  | ArchCreateArrow
  | ArchUpdateShape
  | ArchUpdateArrow
  | ArchMoveShape
  | ArchDeleteShape
  | ArchLayout
  | ArchReparent
  | ArchSelectShapes
  | ArchCamera;

export interface ArchProgram {
  version: number;
  instructions: ArchInstruction[];
}

// 校验实现拆分至 arch-program-validate.ts（类型与校验的变化原因不同）；
// 经再导出保持既有 `from "./arch-program"` 导入路径不变。
export { validateArchProgram } from "./arch-program-validate";
export type { ArchValidationResult } from "./arch-program-validate";
