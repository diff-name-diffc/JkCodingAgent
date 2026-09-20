/**
 * DSL 样式状态的读写与重算：创建时把 DSL 样式词落进元素 customData，
 * 更新时读回并与指令增量合并，再重算 Excalidraw 属性（含绑定文本联动）。
 *
 * 从 arch-apply.ts 拆出（500 行红线）：apply 关心「指令语义」，
 * 本模块关心「样式词 ↔ Excalidraw 属性」的往返一致性。
 */

import type { MutableArrowElement, MutableElement, MutableTextElement } from "./exc-factory";
import type {
  ArchAlign,
  ArchArrowKind,
  ArchColor,
  ArchDash,
  ArchFill,
  ArchFont,
  ArchSize,
} from "./arch-program";
import {
  colorPair,
  dashProps,
  fillProps,
  fontFamilyValue,
  sizeProps,
  textAlignValue,
} from "./exc-style";
import { boundTextOf, estimateTextSize, type SceneDraft } from "./exc-factory";

export interface ShapeStyleState {
  color: ArchColor;
  labelColor?: ArchColor;
  fill: ArchFill;
  dash: ArchDash;
  size: ArchSize;
  font?: ArchFont;
  align?: ArchAlign;
}

export function readStyleState(el: MutableElement): ShapeStyleState {
  const data = (el.customData ?? {}) as Record<string, unknown>;
  return {
    color: (data.archColor as ArchColor) ?? "black",
    labelColor: data.archLabelColor as ArchColor | undefined,
    fill: (data.archFill as ArchFill) ?? "none",
    dash: (data.archDash as ArchDash) ?? "draw",
    size: (data.archSize as ArchSize) ?? "m",
    font: data.archFont as ArchFont | undefined,
    align: data.archAlign as ArchAlign | undefined,
  };
}

export function writeStyleState(el: MutableElement, state: ShapeStyleState): void {
  el.customData = {
    ...(el.customData ?? {}),
    archColor: state.color,
    archLabelColor: state.labelColor,
    archFill: state.fill,
    archDash: state.dash,
    archSize: state.size,
    archFont: state.font,
    archAlign: state.align,
  };
}

/** 按样式状态重算形状的描边/填充/字号，并同步绑定文本样式与度量。 */
export function restyleShape(draft: SceneDraft, el: MutableElement, state: ShapeStyleState): void {
  const pair = colorPair(state.color);
  const dash = dashProps(state.dash);
  const size = sizeProps(state.size);
  el.strokeColor = dash.transparentStroke ? "transparent" : pair.stroke;
  el.strokeStyle = dash.strokeStyle;
  el.roughness = dash.roughness;
  el.strokeWidth = size.strokeWidth;
  if (el.type !== "frame") {
    const fill = fillProps(state.fill, state.color);
    el.backgroundColor = fill.backgroundColor;
    el.fillStyle = fill.fillStyle;
  }
  const text = boundTextOf(el, draft);
  if (text) {
    text.fontSize = size.fontSize;
    text.fontFamily = fontFamilyValue(state.font) as MutableTextElement["fontFamily"];
    text.strokeColor = colorPair(state.labelColor ?? state.color).stroke;
    text.textAlign = textAlignValue(state.align);
    const metrics = estimateTextSize(text.originalText, text.fontSize);
    text.width = metrics.width;
    text.height = metrics.height;
  }
}

export function restyleArrow(
  draft: SceneDraft,
  arrow: MutableArrowElement,
  state: Pick<ShapeStyleState, "color" | "labelColor" | "dash" | "size">,
): void {
  const pair = colorPair(state.color);
  const dash = dashProps(state.dash);
  const size = sizeProps(state.size);
  arrow.strokeColor = dash.transparentStroke ? "transparent" : pair.stroke;
  arrow.strokeStyle = dash.strokeStyle;
  arrow.roughness = dash.roughness;
  arrow.strokeWidth = size.strokeWidth;
  const text = boundTextOf(arrow, draft);
  if (text) {
    text.strokeColor = colorPair(state.labelColor ?? state.color).stroke;
    text.fontSize = size.fontSize;
    const metrics = estimateTextSize(text.originalText, text.fontSize);
    text.width = metrics.width;
    text.height = metrics.height;
  }
}

export function arrowKindRoundness(kind: ArchArrowKind | undefined): { type: 2 } | null {
  // arc = 圆角弧线；elbow 在 Excalidraw 上以直线呈现（直角折线需编辑器交互计算）。
  return kind === "elbow" ? null : { type: 2 };
}

/** 形状/箭头标签文本的样式（取自样式状态）。 */
export function labelStyle(state: ShapeStyleState) {
  const size = sizeProps(state.size);
  return {
    fontSize: size.fontSize,
    fontFamily: fontFamilyValue(state.font),
    color: colorPair(state.labelColor ?? state.color).stroke,
    align: textAlignValue(state.align),
  };
}
