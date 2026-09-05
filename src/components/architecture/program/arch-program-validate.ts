/**
 * 画布程序（architecture_run 载荷）的防御性校验。
 *
 * 从 `arch-program.ts` 拆出（类型与校验的变化原因不同）：类型定义留在
 * `arch-program.ts`，经其再导出保持既有导入路径不变。
 *
 * Rust 权威校验（`program_validate.rs`）仍是完整契约的权威源；本防御层
 * 只覆盖「事件载荷异常时会造成**静默错误行为**」的子集——形状/布局模式/
 * 对齐/箭头装饰等枚举（非法值会被应用层落入默认分支或把未赋值的 partial
 * 传给 tldraw）、layout 数值（gap/columns/origin）、labelPosition 区间与
 * 宽高下限。两处取值必须同步维护。
 */

import {
  ARCH_MAX_INSTRUCTIONS,
  ARCH_PROGRAM_VERSION,
  REPARENT_PAGE_LITERAL,
  type ArchInstruction,
  type ArchProgram,
} from "./arch-program";

export type ArchValidationResult =
  | { ok: true; program: ArchProgram }
  | { ok: false; error: string };

const INSTRUCTION_TYPES = new Set([
  "create_shape",
  "create_arrow",
  "update_shape",
  "update_arrow",
  "move_shape",
  "delete_shape",
  "layout",
  "reparent",
  "select_shapes",
  "camera",
]);

const REF_PATTERN = /^[A-Za-z][A-Za-z0-9_-]{0,31}$/;

// ── 枚举白名单（与 arch-program.ts 的联合类型取值一致）──
const SHAPE_KINDS = new Set(["geo", "note", "text", "frame"]);
const GEO_KINDS = new Set([
  "rectangle", "ellipse", "triangle", "diamond", "pentagon", "hexagon",
  "octagon", "star", "rhombus", "rhombus-2", "oval", "cloud", "trapezoid",
  "arrow-right", "arrow-left", "arrow-up", "arrow-down", "x-box", "check-box",
  "heart",
]);
const COLOR_VALUES = new Set([
  "black", "grey", "light-violet", "violet", "blue", "light-blue", "yellow",
  "orange", "green", "light-green", "light-red", "red", "white",
]);
const FILL_VALUES = new Set(["none", "semi", "solid", "pattern", "fill", "lined-fill"]);
const SIZE_VALUES = new Set(["s", "m", "l", "xl"]);
const DASH_VALUES = new Set(["draw", "solid", "dashed", "dotted", "none"]);
const FONT_VALUES = new Set(["draw", "sans", "serif", "mono"]);
const ALIGN_VALUES = new Set(["start", "middle", "end"]);
const ARROW_KIND_VALUES = new Set(["arc", "elbow"]);
const ARROWHEAD_VALUES = new Set([
  "arrow", "triangle", "square", "dot", "pipe", "diamond", "inverted", "bar",
  "none",
]);
const LAYOUT_MODE_VALUES = new Set(["grid", "row", "column"]);
const LAYOUT_ALIGN_VALUES = new Set(["start", "center", "end"]);

/** 形状样式枚举字段 → 白名单（create_shape / update_shape 共用）。 */
const SHAPE_STYLE_ENUMS: ReadonlyArray<readonly [string, ReadonlySet<string>]> = [
  ["color", COLOR_VALUES],
  ["labelColor", COLOR_VALUES],
  ["fill", FILL_VALUES],
  ["size", SIZE_VALUES],
  ["dash", DASH_VALUES],
  ["font", FONT_VALUES],
  ["align", ALIGN_VALUES],
];

/** 箭头样式子集（tldraw 箭头无 fill/font/align）。 */
const ARROW_STYLE_ENUMS: ReadonlyArray<readonly [string, ReadonlySet<string>]> = [
  ["color", COLOR_VALUES],
  ["labelColor", COLOR_VALUES],
  ["size", SIZE_VALUES],
  ["dash", DASH_VALUES],
];

// ── 数值范围（镜像 Rust 契约常量：GAP_LIMIT / columns / labelPosition / w,h）──
const GAP_LIMIT = 500;
const COLUMNS_MIN = 1;
const COLUMNS_MAX = 8;
const LABEL_POSITION_MIN = 0;
const LABEL_POSITION_MAX = 1;
const SIZE_MIN = 1;
const SIZE_LIMIT = 2000;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isFiniteNumber(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value);
}

function checkRef(ref: unknown, refs: Set<string>, index: number): string | null {
  if (typeof ref !== "string" || !REF_PATTERN.test(ref)) {
    return `第 ${index + 1} 条指令的 ref 不合法`;
  }
  if (refs.has(ref)) return `第 ${index + 1} 条指令的 ref「${ref}」重复`;
  refs.add(ref);
  return null;
}

function checkTarget(target: unknown, index: number): string | null {
  if (typeof target !== "string" || target.length === 0 || target.length > 64) {
    return `第 ${index + 1} 条指令的目标引用不合法`;
  }
  return null;
}

function checkEnumValue(
  value: unknown,
  allowed: ReadonlySet<string>,
  field: string,
  index: number,
): string | null {
  if (value === undefined) return null;
  if (typeof value !== "string" || !allowed.has(value)) {
    return `第 ${index + 1} 条指令的 ${field} 取值不合法`;
  }
  return null;
}

function checkStyleEnums(
  style: Record<string, unknown>,
  enums: ReadonlyArray<readonly [string, ReadonlySet<string>]>,
  index: number,
): string | null {
  for (const [field, allowed] of enums) {
    const error = checkEnumValue(style[field], allowed, field, index);
    if (error) return error;
  }
  return null;
}

function checkNumberRange(
  value: unknown,
  field: string,
  index: number,
  min: number,
  max: number,
): string | null {
  if (value === undefined) return null;
  if (!isFiniteNumber(value) || value < min || value > max) {
    return `第 ${index + 1} 条指令的 ${field} 必须在 [${min}, ${max}] 内`;
  }
  return null;
}

/** 与 Rust `validate_program` 同口径的防御性校验。 */
export function validateArchProgram(raw: unknown): ArchValidationResult {
  if (!isRecord(raw)) return { ok: false, error: "程序不是对象" };
  const version = raw.version;
  if (version !== ARCH_PROGRAM_VERSION) {
    return { ok: false, error: `程序版本必须为 ${ARCH_PROGRAM_VERSION}` };
  }
  const instructions = raw.instructions;
  if (!Array.isArray(instructions) || instructions.length === 0) {
    return { ok: false, error: "程序至少需要一条指令" };
  }
  if (instructions.length > ARCH_MAX_INSTRUCTIONS) {
    return { ok: false, error: `指令数超过上限 ${ARCH_MAX_INSTRUCTIONS}` };
  }

  const refs = new Set<string>();
  for (let index = 0; index < instructions.length; index += 1) {
    const instruction = instructions[index];
    if (!isRecord(instruction)) {
      return { ok: false, error: `第 ${index + 1} 条指令不是对象` };
    }
    const type = instruction._type;
    if (typeof type !== "string" || !INSTRUCTION_TYPES.has(type)) {
      return { ok: false, error: `第 ${index + 1} 条指令类型未知：${String(type)}` };
    }
    const error = validateInstruction(instruction as unknown as ArchInstruction, refs, index);
    if (error) return { ok: false, error };
  }
  return { ok: true, program: raw as unknown as ArchProgram };
}

function validateInstruction(
  instruction: ArchInstruction,
  refs: Set<string>,
  index: number,
): string | null {
  const style = instruction as unknown as Record<string, unknown>;
  switch (instruction._type) {
    case "create_shape": {
      const refError = checkRef(instruction.ref, refs, index);
      if (refError) return refError;
      const shapeError = checkEnumValue(instruction.shape, SHAPE_KINDS, "shape", index);
      if (shapeError) return shapeError;
      if (instruction.shape === "geo" && !instruction.geo) {
        return `第 ${index + 1} 条指令创建 geo 形状时必须给出 geo 类型`;
      }
      const geoError = checkEnumValue(instruction.geo, GEO_KINDS, "geo", index);
      if (geoError) return geoError;
      if (instruction.into !== undefined) {
        if (instruction.shape === "frame") {
          return `第 ${index + 1} 条指令的 frame 只能位于页面根，不能使用 into`;
        }
        const intoError = checkTarget(instruction.into, index);
        if (intoError) return intoError;
      }
      const styleError = checkStyleEnums(style, SHAPE_STYLE_ENUMS, index);
      if (styleError) return styleError;
      if ((instruction.x !== undefined) !== (instruction.y !== undefined)) {
        return `第 ${index + 1} 条指令的 x 与 y 必须同时给出或同时省略`;
      }
      for (const [field, value] of [
        ["x", instruction.x],
        ["y", instruction.y],
      ] as const) {
        if (value !== undefined && !isFiniteNumber(value)) {
          return `第 ${index + 1} 条指令的 ${field} 不是有限数字`;
        }
      }
      for (const [field, value] of [
        ["w", instruction.w],
        ["h", instruction.h],
      ] as const) {
        const error = checkNumberRange(value, field, index, SIZE_MIN, SIZE_LIMIT);
        if (error) return error;
      }
      return null;
    }
    case "create_arrow": {
      if (instruction.ref !== undefined) {
        const refError = checkRef(instruction.ref, refs, index);
        if (refError) return refError;
      }
      const fromError = checkTarget(instruction.from, index);
      if (fromError) return fromError;
      const toError = checkTarget(instruction.to, index);
      if (toError) return toError;
      if (instruction.from === instruction.to) {
        return `第 ${index + 1} 条指令的箭头起点与终点相同`;
      }
      const positionError = checkNumberRange(
        instruction.labelPosition,
        "labelPosition",
        index,
        LABEL_POSITION_MIN,
        LABEL_POSITION_MAX,
      );
      if (positionError) return positionError;
      const kindError = checkEnumValue(instruction.kind, ARROW_KIND_VALUES, "kind", index);
      if (kindError) return kindError;
      for (const [field, value] of [
        ["arrowheadStart", instruction.arrowheadStart],
        ["arrowheadEnd", instruction.arrowheadEnd],
      ] as const) {
        const error = checkEnumValue(value, ARROWHEAD_VALUES, field, index);
        if (error) return error;
      }
      return checkStyleEnums(style, ARROW_STYLE_ENUMS, index);
    }
    case "update_shape": {
      const targetError = checkTarget(instruction.target, index);
      if (targetError) return targetError;
      // 与 Rust 权威层同口径：空 update 是静默无操作，必须报错。
      const hasAnyUpdate =
        instruction.text !== undefined ||
        instruction.x !== undefined ||
        instruction.y !== undefined ||
        instruction.w !== undefined ||
        instruction.h !== undefined ||
        instruction.color !== undefined ||
        instruction.labelColor !== undefined ||
        instruction.fill !== undefined ||
        instruction.size !== undefined ||
        instruction.dash !== undefined ||
        instruction.font !== undefined ||
        instruction.align !== undefined;
      if (!hasAnyUpdate) {
        return `第 ${index + 1} 条指令没有给出任何要更新的字段`;
      }
      const styleError = checkStyleEnums(style, SHAPE_STYLE_ENUMS, index);
      if (styleError) return styleError;
      for (const [field, value] of [
        ["x", instruction.x],
        ["y", instruction.y],
      ] as const) {
        if (value !== undefined && !isFiniteNumber(value)) {
          return `第 ${index + 1} 条指令的 ${field} 不是有限数字`;
        }
      }
      for (const [field, value] of [
        ["w", instruction.w],
        ["h", instruction.h],
      ] as const) {
        const error = checkNumberRange(value, field, index, SIZE_MIN, SIZE_LIMIT);
        if (error) return error;
      }
      return null;
    }
    case "update_arrow": {
      const targetError = checkTarget(instruction.target, index);
      if (targetError) return targetError;
      // 与 Rust 权威层同口径：空 update 是静默无操作，必须报错。
      const hasAnyUpdate =
        instruction.label !== undefined ||
        instruction.labelPosition !== undefined ||
        instruction.kind !== undefined ||
        instruction.arrowheadStart !== undefined ||
        instruction.arrowheadEnd !== undefined ||
        instruction.color !== undefined ||
        instruction.labelColor !== undefined ||
        instruction.size !== undefined ||
        instruction.dash !== undefined;
      if (!hasAnyUpdate) {
        return `第 ${index + 1} 条指令没有给出任何要更新的字段`;
      }
      const positionError = checkNumberRange(
        instruction.labelPosition,
        "labelPosition",
        index,
        LABEL_POSITION_MIN,
        LABEL_POSITION_MAX,
      );
      if (positionError) return positionError;
      const kindError = checkEnumValue(instruction.kind, ARROW_KIND_VALUES, "kind", index);
      if (kindError) return kindError;
      for (const [field, value] of [
        ["arrowheadStart", instruction.arrowheadStart],
        ["arrowheadEnd", instruction.arrowheadEnd],
      ] as const) {
        const error = checkEnumValue(value, ARROWHEAD_VALUES, field, index);
        if (error) return error;
      }
      return checkStyleEnums(style, ARROW_STYLE_ENUMS, index);
    }
    case "move_shape": {
      const targetError = checkTarget(instruction.target, index);
      if (targetError) return targetError;
      // x/y 与 dx/dy 均可单轴给出（未给出的轴保持不变），但两族互斥。
      const hasAbsolute = instruction.x !== undefined || instruction.y !== undefined;
      const hasRelative = instruction.dx !== undefined || instruction.dy !== undefined;
      if (!hasAbsolute && !hasRelative) {
        return `第 ${index + 1} 条指令至少给出一个位移字段（x/y 或 dx/dy，可只给一个轴）`;
      }
      if (hasAbsolute && hasRelative) {
        return `第 ${index + 1} 条指令不能混用绝对坐标 (x, y) 与相对位移 (dx, dy)`;
      }
      for (const [field, value] of [
        ["x", instruction.x],
        ["y", instruction.y],
        ["dx", instruction.dx],
        ["dy", instruction.dy],
      ] as const) {
        if (value !== undefined && !isFiniteNumber(value)) {
          return `第 ${index + 1} 条指令的 ${field} 不是有限数字`;
        }
      }
      return null;
    }
    case "delete_shape": {
      if (
        !Array.isArray(instruction.targets) ||
        instruction.targets.length === 0 ||
        instruction.targets.length > 20
      ) {
        return `第 ${index + 1} 条指令的删除目标数必须在 1~20 内`;
      }
      for (const target of instruction.targets) {
        const targetError = checkTarget(target, index);
        if (targetError) return targetError;
      }
      return null;
    }
    case "layout": {
      if (
        !Array.isArray(instruction.targets) ||
        instruction.targets.length < 2 ||
        instruction.targets.length > ARCH_MAX_INSTRUCTIONS
      ) {
        return `第 ${index + 1} 条指令的布局目标数必须在 2~${ARCH_MAX_INSTRUCTIONS} 内`;
      }
      const modeError = checkEnumValue(instruction.mode, LAYOUT_MODE_VALUES, "mode", index);
      if (modeError) return modeError;
      const alignError = checkEnumValue(instruction.align, LAYOUT_ALIGN_VALUES, "align", index);
      if (alignError) return alignError;
      const gapError = checkNumberRange(instruction.gap, "gap", index, 0, GAP_LIMIT);
      if (gapError) return gapError;
      if (instruction.columns !== undefined) {
        if (instruction.mode !== "grid") {
          return `第 ${index + 1} 条指令仅 grid 布局支持 columns`;
        }
        if (
          !Number.isInteger(instruction.columns) ||
          instruction.columns < COLUMNS_MIN ||
          instruction.columns > COLUMNS_MAX
        ) {
          return `第 ${index + 1} 条指令的 columns 必须是 ${COLUMNS_MIN}..${COLUMNS_MAX} 内的整数`;
        }
      }
      if (instruction.origin !== undefined) {
        const origin = instruction.origin;
        if (!isRecord(origin) || !isFiniteNumber(origin.x) || !isFiniteNumber(origin.y)) {
          return `第 ${index + 1} 条指令的 origin 必须是有限坐标 {x, y}`;
        }
      }
      return null;
    }
    case "reparent": {
      if (
        !Array.isArray(instruction.targets) ||
        instruction.targets.length === 0 ||
        instruction.targets.length > 20
      ) {
        return `第 ${index + 1} 条指令的 reparent 目标数必须在 1~20 内`;
      }
      for (const target of instruction.targets) {
        const targetError = checkTarget(target, index);
        if (targetError) return targetError;
      }
      if (typeof instruction.parent !== "string" || instruction.parent.length === 0) {
        return `第 ${index + 1} 条指令缺少 parent（目标 frame 的引用，或 "page" 移回页面根）`;
      }
      if (instruction.parent !== REPARENT_PAGE_LITERAL) {
        const parentError = checkTarget(instruction.parent, index);
        if (parentError) return parentError;
      }
      return null;
    }
    case "select_shapes": {
      if (
        !Array.isArray(instruction.targets) ||
        instruction.targets.length === 0 ||
        instruction.targets.length > 30
      ) {
        return `第 ${index + 1} 条指令的选中目标数必须在 1~30 内`;
      }
      for (const target of instruction.targets) {
        const targetError = checkTarget(target, index);
        if (targetError) return targetError;
      }
      return null;
    }
    case "camera": {
      if (instruction.mode === "fit") {
        if (instruction.point !== undefined) {
          return `第 ${index + 1} 条指令 fit 模式不需要 point`;
        }
        return null;
      }
      if (instruction.mode === "point") {
        const point = instruction.point;
        if (!point || !isFiniteNumber(point.x) || !isFiniteNumber(point.y)) {
          return `第 ${index + 1} 条指令 point 模式必须给出有限的 point 坐标`;
        }
        return null;
      }
      return `第 ${index + 1} 条指令的 camera 模式必须是 fit 或 point`;
    }
    default:
      return `第 ${index + 1} 条指令类型未知`;
  }
}
