/**
 * DSL 样式 → Excalidraw 元素属性的纯映射（arch 程序应用层的唯一样式出处）。
 *
 * DSL 的样式词汇沿用 tldraw 时代的契约（color/fill/size/dash/font/align 与
 * 箭头样式子集），这里负责把它们落到 Excalidraw 的属性模型上：
 * - color/labelColor：调色板表查 hex（stroke 描边色 + bg 同族浅填充色）；
 * - fill：背景色 + fillStyle（hachure/cross-hatch/solid）；semi 用 60% alpha；
 * - dash：strokeStyle + roughness（draw = 手绘质感，其余为利落几何线）；
 * - size：strokeWidth 与 fontSize 双通道；
 * - font：Excalifont/Helvetica/Virgil/Cascadia 字体族 id；
 * - align：textAlign（容器内文本恒垂直居中）。
 *
 * 全部为无状态纯函数，便于独立单测。
 */

import type { Arrowhead } from "@excalidraw/excalidraw/element/types";

// Excalidraw FONT_FAMILY 常量值（@excalidraw/excalidraw constants.ts）：
// 稳定契约，本地落一份避免纯函数模块把整包运行时拉进测试环境。
const FONT_FAMILY_EXCALIFONT = 5;
const FONT_FAMILY_VIRGIL = 1;
const FONT_FAMILY_HELVETICA = 2;
const FONT_FAMILY_CASCADIA = 3;
import type {
  ArchAlign,
  ArchArrowhead,
  ArchColor,
  ArchDash,
  ArchFill,
  ArchFont,
  ArchSize,
} from "./arch-program";

export interface ArchColorPair {
  /** 描边/文字色。 */
  stroke: string;
  /** 同族浅色填充（fill 非 none 时的背景色）。 */
  bg: string;
}

/** DSL 颜色词 → Excalidraw 调色板（open-color 系）。 */
export const ARCH_COLORS: Record<ArchColor, ArchColorPair> = {
  black: { stroke: "#1e1e1e", bg: "#e9ecef" },
  grey: { stroke: "#868e96", bg: "#f1f3f5" },
  "light-violet": { stroke: "#da77f2", bg: "#f8f0fc" },
  violet: { stroke: "#9775fa", bg: "#e5dbff" },
  blue: { stroke: "#1971c2", bg: "#a5d8ff" },
  "light-blue": { stroke: "#4dabf7", bg: "#d0ebff" },
  yellow: { stroke: "#f08c00", bg: "#ffec99" },
  orange: { stroke: "#e8590c", bg: "#ffd8a8" },
  green: { stroke: "#2f9e44", bg: "#b2f2bb" },
  "light-green": { stroke: "#66a80f", bg: "#d8f5a2" },
  "light-red": { stroke: "#fa5252", bg: "#ffe3e3" },
  red: { stroke: "#e03131", bg: "#ffc9c9" },
  white: { stroke: "#ced4da", bg: "#ffffff" },
};

export const DEFAULT_COLOR: ArchColor = "black";

export function colorPair(color: ArchColor | undefined): ArchColorPair {
  return ARCH_COLORS[color ?? DEFAULT_COLOR];
}

export interface FillProps {
  backgroundColor: string;
  fillStyle: "solid" | "hachure" | "cross-hatch";
}

/** fill → 背景色 + fillStyle；semi 以 60% alpha 表达「半透浅色」。 */
export function fillProps(fill: ArchFill | undefined, color: ArchColor | undefined): FillProps {
  const { bg } = colorPair(color);
  switch (fill) {
    case "none":
      return { backgroundColor: "transparent", fillStyle: "solid" };
    case "semi":
      return { backgroundColor: `${bg}99`, fillStyle: "solid" };
    case "pattern":
      return { backgroundColor: bg, fillStyle: "hachure" };
    case "lined-fill":
      return { backgroundColor: bg, fillStyle: "cross-hatch" };
    case "solid":
    case "fill":
    default:
      return { backgroundColor: bg, fillStyle: "solid" };
  }
}

export interface DashProps {
  strokeStyle: "solid" | "dashed" | "dotted";
  roughness: number;
  /** dash=none 时把描边色置为 transparent。 */
  transparentStroke: boolean;
}

export function dashProps(dash: ArchDash | undefined): DashProps {
  switch (dash) {
    case "solid":
      return { strokeStyle: "solid", roughness: 0, transparentStroke: false };
    case "dashed":
      return { strokeStyle: "dashed", roughness: 0, transparentStroke: false };
    case "dotted":
      return { strokeStyle: "dotted", roughness: 0, transparentStroke: false };
    case "none":
      return { strokeStyle: "solid", roughness: 0, transparentStroke: true };
    case "draw":
    default:
      return { strokeStyle: "solid", roughness: 1, transparentStroke: false };
  }
}

export interface SizeProps {
  strokeWidth: number;
  fontSize: number;
}

export function sizeProps(size: ArchSize | undefined): SizeProps {
  switch (size) {
    case "s":
      return { strokeWidth: 1, fontSize: 16 };
    case "l":
      return { strokeWidth: 2, fontSize: 28 };
    case "xl":
      return { strokeWidth: 4, fontSize: 36 };
    case "m":
    default:
      return { strokeWidth: 2, fontSize: 20 };
  }
}

/** font → Excalidraw 字体族 id；serif 无对应族，取 Virgil 顶位。 */
export function fontFamilyValue(font: ArchFont | undefined): number {
  switch (font) {
    case "sans":
      return FONT_FAMILY_HELVETICA;
    case "mono":
      return FONT_FAMILY_CASCADIA;
    case "serif":
      return FONT_FAMILY_VIRGIL;
    case "draw":
    default:
      return FONT_FAMILY_EXCALIFONT;
  }
}

export function textAlignValue(align: ArchAlign | undefined): "left" | "center" | "right" {
  switch (align) {
    case "start":
      return "left";
    case "end":
      return "right";
    case "middle":
    default:
      return "center";
  }
}

/** DSL 箭头头部词 → Excalidraw Arrowhead（无对应的取最近语义）。 */
export function arrowheadValue(head: ArchArrowhead | undefined): Arrowhead | null {
  switch (head) {
    case "arrow":
      return "arrow";
    case "triangle":
      return "triangle";
    case "square":
    case "pipe":
    case "bar":
      return "bar";
    case "dot":
      return "dot";
    case "diamond":
      return "diamond";
    case "inverted":
      return "triangle_outline";
    case "none":
    default:
      return null;
  }
}
