/**
 * 画布程序解释器的应用层：把单条类型化指令翻译成 Excalidraw 场景草稿变更。
 *
 * 不做校验（校验在 Rust 权威层 + arch-program-validate.ts 防御层完成）；
 * 这里假定指令结构合法、所有引用已解析为真实元素 id（解析在 arch-executor.ts）。
 *
 * 与 tldraw 时代的差异：全程只改 SceneDraft（元素 Map 的可变副本），不碰画布；
 * 执行器在全部指令成功后一次性 updateScene 提交，天然 all-or-nothing。
 */


import type {
  ArchCamera,
  ArchInstruction,
  ArchLayout,
  ArchMoveShape,
  ArchReparent,
  ArchSelectShapes,
  ArchUpdateArrow,
  ArchUpdateShape,
} from "./arch-program";
import { layoutShapes, type LayoutItem } from "./arch-layout";
import { finite, type AutoPlaceCursor } from "./arch-geometry";
import {
  arrowKindRoundness,
  labelStyle,
  readStyleState,
  restyleArrow,
  restyleShape,
  writeStyleState,
  type ShapeStyleState,
} from "./exc-style-state";
import { arrowheadValue } from "./exc-style";
import {
  ARROW_LABEL_POSITION_KEY,
  boundTextOf,
  cascadeDelete,
  cascadeMove,
  createTextElement,
  estimateTextSize,
  registerBoundText,
  type MutableArrowElement,
  type MutableElement,
  type SceneDraft,
} from "./exc-factory";
import { applyCreateArrow, applyCreateShape } from "./arch-apply-create";

/** 单条指令的解析产物：所有目标引用都已换成真实存在的元素 id。 */
export interface ResolvedInstruction {
  instruction: ArchInstruction;
  /** create_shape/create_arrow 登记的新元素 id。 */
  createdId?: string;
  /** update/move/layout/select_shapes/reparent 的目标（按指令内出现顺序）。 */
  targetIds?: string[];
  /** delete 的目标。 */
  deleteIds?: string[];
  /** create_arrow 的两端。 */
  arrowEnds?: { fromId: string; toId: string };
  /** create_shape.into 解析出的父容器。 */
  parentFrameId?: string;
  /** reparent 的目标容器；null = 移回页面根。 */
  reparentParentId?: string | null;
}

/** 应用上下文：草稿 + 视口锚点 + 自动放置游标 + 延迟生效的视图指令收集。 */
export interface ApplyContext {
  draft: SceneDraft;
  /** 视口中心（场景坐标），autoPlace 锚点。 */
  viewportCenter: { x: number; y: number };
  cursor: AutoPlaceCursor;
  /** select_shapes 收集的选中集合（提交时写入 appState）。 */
  selected: Set<string>;
  zoomSelection: boolean;
  /** 最后一条 camera 指令（提交后生效）。 */
  camera: ArchCamera | null;
  /** 本程序实际移动过的元素 id（frame 级联展开后），供定向绑定修复。 */
  moved: Set<string>;
}

export function createApplyContext(
  draft: SceneDraft,
  viewportCenter: { x: number; y: number },
): ApplyContext {
  return {
    draft,
    viewportCenter,
    cursor: { x: 0, y: 0, placed: false, frameCounts: new Map() },
    selected: new Set(),
    zoomSelection: false,
    camera: null,
    moved: new Set(),
  };
}

// ── 各指令应用 ──

/** 写入/更新/清除形状的文本（geo/note 的绑定标签；frame 的标题）。 */
function setShapeText(draft: SceneDraft, el: MutableElement, text: string): void {
  if (el.type === "frame") {
    el.name = text;
    return;
  }
  const existing = boundTextOf(el, draft);
  if (!text) {
    if (existing) {
      el.boundElements = (el.boundElements ?? []).filter((b) => b.id !== existing.id);
      draft.delete(existing.id);
    }
    return;
  }
  if (existing) {
    existing.text = text;
    existing.originalText = text;
    const metrics = estimateTextSize(text, existing.fontSize);
    existing.width = metrics.width;
    existing.height = metrics.height;
    return;
  }
  const state = readStyleState(el);
  const label = createTextElement(0, 0, text, labelStyle(state), el.id);
  registerBoundText(el, label);
  draft.set(label.id, label);
}

function applyUpdateShape(
  ctx: ApplyContext,
  resolved: ResolvedInstruction,
  instruction: ArchUpdateShape,
): void {
  const { draft } = ctx;
  const el = draft.get(resolved.targetIds![0]);
  if (!el) return;

  if (instruction.text !== undefined) setShapeText(draft, el, instruction.text);

  const x = finite(instruction.x);
  const y = finite(instruction.y);
  if (x !== undefined || y !== undefined) {
    for (const id of cascadeMove(draft, [el.id], (x ?? el.x) - el.x, (y ?? el.y) - el.y)) {
      ctx.moved.add(id);
    }
  }
  const w = finite(instruction.w);
  const h = finite(instruction.h);
  if (w !== undefined) el.width = w;
  if (h !== undefined) el.height = h;

  const state = readStyleState(el);
  const next: ShapeStyleState = {
    color: instruction.color ?? state.color,
    labelColor: instruction.labelColor ?? state.labelColor,
    fill: instruction.fill ?? state.fill,
    dash: instruction.dash ?? state.dash,
    size: instruction.size ?? state.size,
    font: instruction.font ?? state.font,
    align: instruction.align ?? state.align,
  };
  writeStyleState(el, next);
  restyleShape(draft, el, next);
}

function applyUpdateArrow(
  ctx: ApplyContext,
  resolved: ResolvedInstruction,
  instruction: ArchUpdateArrow,
): void {
  const { draft } = ctx;
  const el = draft.get(resolved.targetIds![0]);
  if (!el || el.type !== "arrow") return;
  const arrow = el as MutableArrowElement;

  if (instruction.kind !== undefined) arrow.roundness = arrowKindRoundness(instruction.kind);
  if (instruction.arrowheadStart !== undefined) {
    arrow.startArrowhead = arrowheadValue(instruction.arrowheadStart);
  }
  if (instruction.arrowheadEnd !== undefined) {
    arrow.endArrowhead = arrowheadValue(instruction.arrowheadEnd);
  }
  if (instruction.labelPosition !== undefined) {
    const labelPosition = finite(instruction.labelPosition) ?? 0.5;
    arrow.customData = { ...(arrow.customData ?? {}), [ARROW_LABEL_POSITION_KEY]: labelPosition };
  }
  if (instruction.label !== undefined) {
    const state = readStyleState(arrow);
    if (!instruction.label) {
      const existing = boundTextOf(arrow, draft);
      if (existing) {
        arrow.boundElements = (arrow.boundElements ?? []).filter((b) => b.id !== existing.id);
        draft.delete(existing.id);
      }
    } else {
      const existing = boundTextOf(arrow, draft);
      if (existing) {
        existing.text = instruction.label;
        existing.originalText = instruction.label;
        const metrics = estimateTextSize(instruction.label, existing.fontSize);
        existing.width = metrics.width;
        existing.height = metrics.height;
      } else {
        const label = createTextElement(0, 0, instruction.label, labelStyle(state), arrow.id);
        registerBoundText(arrow, label);
        draft.set(label.id, label);
      }
    }
  }

  const state = readStyleState(arrow);
  const next: ShapeStyleState = {
    ...state,
    color: instruction.color ?? state.color,
    labelColor: instruction.labelColor ?? state.labelColor,
    dash: instruction.dash ?? state.dash,
    size: instruction.size ?? state.size,
  };
  writeStyleState(arrow, next);
  restyleArrow(draft, arrow, next);
}

function applyMoveShape(ctx: ApplyContext, resolved: ResolvedInstruction, instruction: ArchMoveShape): void {
  const { draft } = ctx;
  const el = draft.get(resolved.targetIds![0]);
  if (!el) return;
  const dx = finite(instruction.dx) ?? (finite(instruction.x) !== undefined ? finite(instruction.x)! - el.x : 0);
  const dy = finite(instruction.dy) ?? (finite(instruction.y) !== undefined ? finite(instruction.y)! - el.y : 0);
  for (const id of cascadeMove(draft, [el.id], dx, dy)) ctx.moved.add(id);
}

function applyLayout(ctx: ApplyContext, resolved: ResolvedInstruction, instruction: ArchLayout): void {
  const { draft } = ctx;
  const ids = resolved.targetIds!;
  const items: LayoutItem[] = [];
  let minX = Infinity;
  let minY = Infinity;
  for (const id of ids) {
    const el = draft.get(id);
    if (!el) continue;
    items.push({ id, w: el.width, h: el.height });
    minX = Math.min(minX, el.x);
    minY = Math.min(minY, el.y);
  }
  if (items.length < 2) return;
  const positions = layoutShapes(items, {
    mode: instruction.mode,
    origin: instruction.origin ?? { x: minX, y: minY },
    gap: instruction.gap,
    columns: instruction.columns,
    align: instruction.align,
  });
  for (const id of ids) {
    const next = positions.get(id);
    const el = draft.get(id);
    if (!next || !el) continue;
    // 场景坐标恒为绝对坐标（frame 子元素亦然），直接写回；
    // cascadeMove 让 frame 带着子元素整体移动。
    for (const movedId of cascadeMove(draft, [id], next.x - el.x, next.y - el.y)) {
      ctx.moved.add(movedId);
    }
  }
}

/**
 * 移动形状进/出 frame：场景坐标不变，只改 frameId 归属（含绑定文本同步）。
 * 约束：箭头两端由绑定决定归属、不允许 reparent；frame 只能位于页面根；
 * 目标容器在被移动目标之中时拒绝（循环包含）。
 */
function applyReparent(ctx: ApplyContext, resolved: ResolvedInstruction, instruction: ArchReparent): void {
  const { draft } = ctx;
  const parentId = resolved.reparentParentId ?? null;
  if (parentId) {
    const parent = draft.get(parentId);
    if (!parent || parent.type !== "frame") {
      throw new Error(`reparent 的目标容器「${instruction.parent}」不是 frame`);
    }
  }

  const targetIds = resolved.targetIds!;
  const validIds: string[] = [];
  for (const id of targetIds) {
    const el = draft.get(id);
    if (!el) continue; // 执行期已不存在（如前序指令删除）：跳过
    if (el.type === "arrow") {
      throw new Error("箭头不能 reparent：它的容器由两端形状决定");
    }
    if (el.type === "frame") {
      throw new Error("frame 只能位于页面根，不能作为 reparent 目标");
    }
    validIds.push(id);
  }
  if (validIds.length === 0) return;

  if (parentId && targetIds.includes(parentId)) {
    throw new Error("reparent 的目标容器位于被移动形状之中，会形成循环包含");
  }

  for (const id of validIds) {
    const el = draft.get(id)!;
    el.frameId = parentId;
    const label = boundTextOf(el, draft);
    if (label) label.frameId = parentId;
  }
}

function applySelectShapes(
  ctx: ApplyContext,
  resolved: ResolvedInstruction,
  instruction: ArchSelectShapes,
): void {
  for (const id of resolved.targetIds!) {
    if (ctx.draft.has(id)) ctx.selected.add(id);
  }
  if (instruction.zoom) ctx.zoomSelection = true;
}

export function applyResolvedInstruction(ctx: ApplyContext, resolved: ResolvedInstruction): void {
  const instruction = resolved.instruction;
  switch (instruction._type) {
    case "create_shape":
      applyCreateShape(ctx, resolved, instruction);
      break;
    case "create_arrow":
      applyCreateArrow(ctx, resolved, instruction);
      break;
    case "update_shape":
      applyUpdateShape(ctx, resolved, instruction);
      break;
    case "update_arrow":
      applyUpdateArrow(ctx, resolved, instruction);
      break;
    case "move_shape":
      applyMoveShape(ctx, resolved, instruction);
      break;
    case "delete_shape":
      cascadeDelete(ctx.draft, resolved.deleteIds!);
      break;
    case "layout":
      applyLayout(ctx, resolved, instruction);
      break;
    case "reparent":
      applyReparent(ctx, resolved, instruction);
      break;
    case "select_shapes":
      applySelectShapes(ctx, resolved, instruction);
      break;
    case "camera":
      ctx.camera = instruction;
      break;
  }
}
