/**
 * create_shape / create_arrow 指令的应用实现（自 arch-apply.ts 拆出，500 行红线）。
 *
 * 创建类指令的共同模式：样式状态落 customData（供后续 update 读回合并），
 * 文本一律走 Excalidraw 容器绑定文本（bound text），箭头端点取边框交点。
 */

import type { MutableElement } from "./exc-factory";
import type { ArchCreateArrow, ArchCreateShape } from "./arch-program";
import { autoPlaceInFrame, autoPlacePosition, finite } from "./arch-geometry";
import { arrowheadValue, colorPair, dashProps, fillProps, sizeProps } from "./exc-style";
import {
  arrowKindRoundness,
  labelStyle,
  writeStyleState,
  type ShapeStyleState,
} from "./exc-style-state";
import {
  ARCH_KIND_KEY,
  ARROW_LABEL_POSITION_KEY,
  borderPointToward,
  boundTextOf,
  createArrowElement,
  createFrameElement,
  createShapeElement,
  createTextElement,
  elementCenter,
  estimateTextSize,
  positionArrowLabel,
  registerArrowBindings,
  registerBoundText,
  type SceneDraft,
} from "./exc-factory";
import type { ApplyContext, ResolvedInstruction } from "./arch-apply";

const DEFAULT_GEO_SIZE = { w: 200, h: 100 };
const DEFAULT_NOTE_SIZE = { w: 200, h: 200 };
const DEFAULT_FRAME_SIZE = { w: 400, h: 300 };

function frameBounds(draft: SceneDraft, frameId: string): { x: number; y: number } | undefined {
  const frame = draft.get(frameId);
  return frame ? { x: frame.x, y: frame.y } : undefined;
}

export function applyCreateShape(
  ctx: ApplyContext,
  resolved: ResolvedInstruction,
  instruction: ArchCreateShape,
): void {
  const { draft, cursor } = ctx;
  const id = resolved.createdId!;
  const parentFrameId = resolved.parentFrameId;
  const w = finite(instruction.w);
  const h = finite(instruction.h);

  const defaultSize =
    instruction.shape === "note"
      ? DEFAULT_NOTE_SIZE
      : instruction.shape === "frame"
        ? DEFAULT_FRAME_SIZE
        : DEFAULT_GEO_SIZE;
  const finalW = w ?? defaultSize.w;
  const finalH = h ?? defaultSize.h;

  const position =
    finite(instruction.x) !== undefined && finite(instruction.y) !== undefined
      ? { x: finite(instruction.x)!, y: finite(instruction.y)! }
      : parentFrameId
        ? autoPlaceInFrame(frameBounds(draft, parentFrameId), cursor, parentFrameId)
        : autoPlacePosition(ctx.viewportCenter, cursor, finalW, finalH);

  const state: ShapeStyleState = {
    color: instruction.color ?? (instruction.shape === "note" ? "yellow" : "black"),
    labelColor: instruction.labelColor,
    fill: instruction.fill ?? (instruction.shape === "note" ? "solid" : "none"),
    dash: instruction.dash ?? "draw",
    size: instruction.size ?? "m",
    font: instruction.font,
    align: instruction.align,
  };

  let element: MutableElement;
  if (instruction.shape === "frame") {
    element = createFrameElement(
      id,
      {
        x: position.x,
        y: position.y,
        width: finalW,
        height: finalH,
        strokeColor: "#adb5bd",
        backgroundColor: "transparent",
        fillStyle: "solid",
        strokeWidth: 1,
        strokeStyle: "solid",
        roughness: 0,
        roundness: null,
        customData: { [ARCH_KIND_KEY]: "frame" },
      },
      instruction.text ?? "",
    );
  } else if (instruction.shape === "text") {
    const style = labelStyle(state);
    const text = instruction.text ?? "";
    const metrics = estimateTextSize(text, style.fontSize);
    const textEl = createTextElement(position.x, position.y, text, style, null);
    textEl.id = id; // ref 直接映射到文本元素 id
    textEl.width = w ?? metrics.width;
    textEl.height = metrics.height;
    textEl.autoResize = w === undefined;
    textEl.customData = { [ARCH_KIND_KEY]: "text" };
    element = textEl;
  } else {
    const pair = colorPair(state.color);
    const dash = dashProps(state.dash);
    const fill = fillProps(state.fill, state.color);
    element = createShapeElement(id, instruction.geo ?? "rectangle", {
      x: position.x,
      y: position.y,
      width: finalW,
      height: finalH,
      strokeColor: dash.transparentStroke ? "transparent" : pair.stroke,
      backgroundColor: fill.backgroundColor,
      fillStyle: fill.fillStyle,
      strokeWidth: sizeProps(state.size).strokeWidth,
      strokeStyle: dash.strokeStyle,
      roughness: dash.roughness,
      roundness: { type: 3 },
      customData: { [ARCH_KIND_KEY]: instruction.shape },
    });
    if (instruction.text) {
      const label = createTextElement(0, 0, instruction.text, labelStyle(state), id);
      registerBoundText(element, label);
      draft.set(label.id, label);
    }
  }

  writeStyleState(element, state);
  if (parentFrameId) {
    const parent = draft.get(parentFrameId);
    if (!parent || parent.type !== "frame") {
      throw new Error(`into 的目标不是 frame，无法把「${instruction.ref}」放入`);
    }
    element.frameId = parentFrameId;
    // 绑定文本与容器同归属（frame 裁剪/移动以 frameId 为准）。
    const label = boundTextOf(element, draft);
    if (label) label.frameId = parentFrameId;
  }
  draft.set(id, element);
}

export function applyCreateArrow(
  ctx: ApplyContext,
  resolved: ResolvedInstruction,
  instruction: ArchCreateArrow,
): void {
  const { draft } = ctx;
  const arrowId = resolved.createdId!;
  const from = draft.get(resolved.arrowEnds!.fromId);
  const to = draft.get(resolved.arrowEnds!.toId);
  if (!from || !to) throw new Error("箭头两端形状不存在");

  const state: ShapeStyleState = {
    color: instruction.color ?? "black",
    labelColor: instruction.labelColor,
    fill: "none",
    dash: instruction.dash ?? "draw",
    size: instruction.size ?? "m",
  };
  const pair = colorPair(state.color);
  const dash = dashProps(state.dash);

  const start = borderPointToward(from, elementCenter(to));
  const end = borderPointToward(to, elementCenter(from));
  const labelPosition = finite(instruction.labelPosition) ?? 0.5;

  const arrow = createArrowElement(arrowId, {
    x: start.x,
    y: start.y,
    width: 0,
    height: 0,
    start,
    end,
    startBindingId: from.id,
    endBindingId: to.id,
    startArrowhead: arrowheadValue(instruction.arrowheadStart),
    endArrowhead:
      instruction.arrowheadEnd === undefined ? "arrow" : arrowheadValue(instruction.arrowheadEnd),
    strokeColor: dash.transparentStroke ? "transparent" : pair.stroke,
    backgroundColor: "transparent",
    fillStyle: "solid",
    strokeWidth: sizeProps(state.size).strokeWidth,
    strokeStyle: dash.strokeStyle,
    roughness: dash.roughness,
    roundness: arrowKindRoundness(instruction.kind),
    labelPosition,
    customData: { [ARROW_LABEL_POSITION_KEY]: labelPosition },
  });
  registerArrowBindings(draft, arrow);
  draft.set(arrowId, arrow);

  if (instruction.label) {
    const label = createTextElement(0, 0, instruction.label, labelStyle(state), arrowId);
    registerBoundText(arrow, label);
    positionArrowLabel(arrow, label);
    draft.set(label.id, label);
  }
  writeStyleState(arrow, state);
}
