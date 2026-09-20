/**
 * Excalidraw 元素工厂与场景助手：arch 程序应用层与 Excalidraw 数据模型之间
 * 的全部构造/修复逻辑集中于此。
 *
 * 与 tldraw 时代的关键差异：Excalidraw 元素是**纯 JSON + 场景绝对坐标**
 *（frame 子元素也是绝对坐标，frameId 仅表达归属），且程序化 updateScene
 * 不会触发编辑器的绑定几何重算——因此本模块提供：
 * - 元素工厂（补齐渲染所需的全部字段，index 置 null 交由 updateScene 同步）；
 * - 容器绑定文本（bound text）的定位与度量估算；
 * - 绑定箭头的端点几何重算（边框交点近似）；
 * - 级联移动/删除（frame 子元素、绑定文本、绑定箭头）。
 *
 * 形状 id 保持 `shape:` 前缀——快照/提示词契约与 tldraw 时代一致。
 */

import type {
  ExcalidrawArrowElement,
  ExcalidrawElement,
  ExcalidrawFrameElement,
  ExcalidrawTextElement,
} from "@excalidraw/excalidraw/element/types";

/** 解除 Excalidraw 元素类型的 readonly（草稿是可变工作副本，提交前不回写画布）。 */
export type Mutable<T> = { -readonly [K in keyof T]: T[K] };
export type MutableElement = Mutable<ExcalidrawElement>;
export type MutableTextElement = Mutable<ExcalidrawTextElement>;
export type MutableArrowElement = Mutable<ExcalidrawArrowElement>;
export type MutableFrameElement = Mutable<ExcalidrawFrameElement>;

/** 场景草稿：元素 id → 可变性副本；Map 迭代序即提交时的层叠序。 */
export type SceneDraft = Map<string, MutableElement>;

/** 绑定箭头的 labelPosition 存进 customData，移动后重算标签位置时读取。 */
export const ARROW_LABEL_POSITION_KEY = "archLabelPosition";
/** 元素 customData 中的 DSL 种类（note 渲染为黄底矩形，快照需要还原语义）。 */
export const ARCH_KIND_KEY = "archKind";

const LINE_HEIGHT = 1.25;

export function newShapeId(): string {
  return `shape:${Math.random().toString(36).slice(2, 10)}`;
}

function newTextId(): string {
  return `text:${Math.random().toString(36).slice(2, 10)}`;
}

let seedCounter = 1;
function nextSeed(): number {
  // 随机种子决定手绘扰动形态；同会话内递增即可，无需密码学强度。
  seedCounter = (seedCounter * 48271) % 2147483647;
  return seedCounter;
}

interface BaseInit {
  x: number;
  y: number;
  width: number;
  height: number;
  strokeColor: string;
  backgroundColor: string;
  fillStyle: "solid" | "hachure" | "cross-hatch";
  strokeWidth: number;
  strokeStyle: "solid" | "dashed" | "dotted";
  roughness: number;
  roundness: { type: 2 | 3 } | null;
  frameId?: string | null;
  customData?: Record<string, unknown>;
}

function baseElement(id: string, type: string, init: BaseInit): Record<string, unknown> {
  return {
    id,
    type,
    x: init.x,
    y: init.y,
    width: init.width,
    height: init.height,
    angle: 0,
    strokeColor: init.strokeColor,
    backgroundColor: init.backgroundColor,
    fillStyle: init.fillStyle,
    strokeWidth: init.strokeWidth,
    strokeStyle: init.strokeStyle,
    roughness: init.roughness,
    opacity: 100,
    groupIds: [],
    frameId: init.frameId ?? null,
    roundness: init.roundness,
    seed: nextSeed(),
    version: 1,
    versionNonce: nextSeed(),
    index: null,
    isDeleted: false,
    boundElements: [],
    updated: Date.now(),
    link: null,
    locked: false,
    ...(init.customData ? { customData: init.customData } : {}),
  } as Record<string, unknown>;
}

/** 粗略文本度量：CJK 按全宽、其余按 0.55 字宽估算；多行按行拆分取最宽行。 */
export function estimateTextSize(
  text: string,
  fontSize: number,
): { width: number; height: number } {
  const lines = text.split("\n");
  let width = 0;
  for (const line of lines) {
    let lineWidth = 0;
    for (const ch of line) {
      lineWidth += ch.codePointAt(0)! > 0x2e7f ? fontSize : fontSize * 0.55;
    }
    width = Math.max(width, lineWidth);
  }
  return { width: Math.max(width, 1), height: Math.max(lines.length, 1) * fontSize * LINE_HEIGHT };
}

export interface TextStyle {
  fontSize: number;
  fontFamily: number;
  color: string;
  align: "left" | "center" | "right";
}

export function createTextElement(
  x: number,
  y: number,
  text: string,
  style: TextStyle,
  containerId: string | null,
): MutableTextElement {
  const { width, height } = estimateTextSize(text, style.fontSize);
  return {
    ...baseElement(newTextId(), "text", {
      x,
      y,
      width,
      height,
      strokeColor: style.color,
      backgroundColor: "transparent",
      fillStyle: "solid",
      strokeWidth: 1,
      strokeStyle: "solid",
      roughness: 0,
      roundness: null,
    }),
    type: "text",
    text,
    fontSize: style.fontSize,
    fontFamily: style.fontFamily as ExcalidrawTextElement["fontFamily"],
    textAlign: style.align,
    verticalAlign: containerId ? "middle" : "top",
    containerId,
    originalText: text,
    autoResize: true,
    lineHeight: LINE_HEIGHT as ExcalidrawTextElement["lineHeight"],
  } as unknown as MutableTextElement;
}

export function createShapeElement(
  id: string,
  geo: "rectangle" | "ellipse" | "diamond",
  init: BaseInit,
): MutableElement {
  return { ...baseElement(id, geo, init), type: geo } as unknown as MutableElement;
}

export function createFrameElement(
  id: string,
  init: BaseInit,
  name: string,
): MutableFrameElement {
  return {
    ...baseElement(id, "frame", init),
    type: "frame",
    name,
  } as unknown as MutableFrameElement;
}

export interface ArrowInit extends BaseInit {
  start: { x: number; y: number };
  end: { x: number; y: number };
  startBindingId: string;
  endBindingId: string;
  startArrowhead: ExcalidrawArrowElement["startArrowhead"];
  endArrowhead: ExcalidrawArrowElement["endArrowhead"];
  labelPosition: number;
}

export function createArrowElement(id: string, init: ArrowInit): MutableArrowElement {
  const dx = init.end.x - init.start.x;
  const dy = init.end.y - init.start.y;
  return {
    ...baseElement(id, "arrow", init),
    type: "arrow",
    x: init.start.x,
    y: init.start.y,
    width: Math.abs(dx),
    height: Math.abs(dy),
    points: [
      [0, 0],
      [dx, dy],
    ] as unknown as ExcalidrawArrowElement["points"],
    lastCommittedPoint: null,
    startBinding: { elementId: init.startBindingId, focus: 0, gap: 4 },
    endBinding: { elementId: init.endBindingId, focus: 0, gap: 4 },
    startArrowhead: init.startArrowhead,
    endArrowhead: init.endArrowhead,
    elbowed: false,
    customData: { [ARROW_LABEL_POSITION_KEY]: init.labelPosition, ...(init.customData ?? {}) },
  } as unknown as MutableArrowElement;
}

// ── 场景助手 ──

export function elementCenter(el: ExcalidrawElement): { x: number; y: number } {
  return { x: el.x + el.width / 2, y: el.y + el.height / 2 };
}

/**
 * 边框交点：从元素中心朝 toward 方向与包围盒（矩形近似，椭圆/菱形同按矩形）
 * 的交点；中心重合时退回中心。
 */
export function borderPointToward(
  el: ExcalidrawElement,
  toward: { x: number; y: number },
): { x: number; y: number } {
  const center = elementCenter(el);
  const dx = toward.x - center.x;
  const dy = toward.y - center.y;
  if (dx === 0 && dy === 0) return center;
  const halfW = Math.max(el.width / 2, 1);
  const halfH = Math.max(el.height / 2, 1);
  const scale = Math.min(halfW / Math.max(Math.abs(dx), 1e-6), halfH / Math.max(Math.abs(dy), 1e-6));
  return { x: center.x + dx * scale, y: center.y + dy * scale };
}

/** 容器内绑定文本：水平按对齐、垂直居中地放进容器包围盒。 */
export function positionContainerText(container: MutableElement, text: MutableTextElement): void {
  const pad = 8;
  const innerW = Math.max(container.width - pad * 2, 1);
  const x =
    text.textAlign === "left"
      ? container.x + pad
      : text.textAlign === "right"
        ? container.x + container.width - pad - text.width
        : container.x + (container.width - text.width) / 2;
  text.x = x;
  text.y = container.y + (container.height - text.height) / 2;
  text.width = Math.min(text.width, innerW);
}

/** 箭头标注文本：按 labelPosition（0~1）落在两端点连线上。 */
export function positionArrowLabel(arrow: MutableArrowElement, text: MutableTextElement): void {
  const labelPosition =
    typeof arrow.customData?.[ARROW_LABEL_POSITION_KEY] === "number"
      ? (arrow.customData[ARROW_LABEL_POSITION_KEY] as number)
      : 0.5;
  const t = Math.min(1, Math.max(0, labelPosition));
  const [p0, p1] = [arrow.points[0], arrow.points[arrow.points.length - 1]];
  const cx = arrow.x + p0[0] + (p1[0] - p0[0]) * t;
  const cy = arrow.y + p0[1] + (p1[1] - p0[1]) * t;
  text.x = cx - text.width / 2;
  text.y = cy - text.height / 2;
}

/** 重算绑定箭头的端点几何（两端形状移动/改尺寸后调用）。 */
export function recomputeArrowGeometry(arrow: MutableArrowElement, draft: SceneDraft): void {
  const from = arrow.startBinding ? draft.get(arrow.startBinding.elementId) : undefined;
  const to = arrow.endBinding ? draft.get(arrow.endBinding.elementId) : undefined;
  if (!from || !to) return;
  const start = borderPointToward(from, elementCenter(to));
  const end = borderPointToward(to, elementCenter(from));
  arrow.x = start.x;
  arrow.y = start.y;
  arrow.width = Math.abs(end.x - start.x);
  arrow.height = Math.abs(end.y - start.y);
  arrow.points = [
    [0, 0],
    [end.x - start.x, end.y - start.y],
  ] as unknown as ExcalidrawArrowElement["points"];
}

export function boundTextOf(container: MutableElement, draft: SceneDraft): MutableTextElement | null {
  const binding = container.boundElements?.find((b) => b.type === "text");
  if (!binding) return null;
  const el = draft.get(binding.id);
  return el && el.type === "text" ? (el as MutableTextElement) : null;
}

/**
 * 定向修复：只重算「与 relevantIds 相关」的绑定箭头几何与绑定文本位置
 * （程序化 updateScene 不走编辑器绑定逻辑）。用户手绘的多点箭头/标签
 * 若与本程序无关则原样保留，不被拍平成两点直线。
 */
export function repairSceneBindings(draft: SceneDraft, relevantIds: ReadonlySet<string>): void {
  for (const el of draft.values()) {
    if (el.type !== "arrow") continue;
    const arrow = el as ExcalidrawArrowElement;
    const relevant =
      relevantIds.has(arrow.id) ||
      (arrow.startBinding ? relevantIds.has(arrow.startBinding.elementId) : false) ||
      (arrow.endBinding ? relevantIds.has(arrow.endBinding.elementId) : false);
    if (relevant) recomputeArrowGeometry(arrow, draft);
  }
  for (const el of draft.values()) {
    if (el.type !== "text") continue;
    const text = el as ExcalidrawTextElement;
    if (!text.containerId || !relevantIds.has(text.containerId)) continue;
    const container = draft.get(text.containerId);
    if (!container) continue;
    if (container.type === "arrow") {
      positionArrowLabel(container as MutableArrowElement, text);
    } else {
      positionContainerText(container, text);
    }
  }
}

/** 级联移动：frame 带动子元素（含其绑定文本）；箭头不动，交由修复重算。返回实际移动的元素 id。 */
export function cascadeMove(
  draft: SceneDraft,
  rootIds: Iterable<string>,
  dx: number,
  dy: number,
): Set<string> {
  const moving = new Set<string>(rootIds);
  if (dx === 0 && dy === 0) return moving;
  for (const el of draft.values()) {
    if (el.frameId && moving.has(el.frameId)) moving.add(el.id);
  }
  for (const id of moving) {
    const el = draft.get(id);
    if (!el || el.type === "arrow") continue;
    el.x += dx;
    el.y += dy;
  }
  return moving;
}

/** 级联删除：形状带走绑定文本与绑定箭头；frame 带走子元素。返回实际删除的 id。 */
export function cascadeDelete(draft: SceneDraft, rootIds: Iterable<string>): Set<string> {
  const deleting = new Set<string>(rootIds);
  let grew = true;
  while (grew) {
    grew = false;
    for (const el of draft.values()) {
      if (deleting.has(el.id)) continue;
      const isBoundText = el.type === "text" && (el as ExcalidrawTextElement).containerId;
      const containerId = isBoundText ? (el as ExcalidrawTextElement).containerId! : null;
      const arrowEl = el as MutableArrowElement;
      const isBoundArrow =
        el.type === "arrow" &&
        ((arrowEl.startBinding && deleting.has(arrowEl.startBinding.elementId)) ||
          (arrowEl.endBinding && deleting.has(arrowEl.endBinding.elementId)));
      const isFrameChild = el.frameId && deleting.has(el.frameId);
      if ((containerId && deleting.has(containerId)) || isBoundArrow || isFrameChild) {
        deleting.add(el.id);
        grew = true;
      }
    }
  }
  for (const id of deleting) draft.delete(id);
  // 清理幸存元素上指向已删元素的绑定引用
  for (const el of draft.values()) {
    if (el.boundElements?.some((b) => deleting.has(b.id))) {
      el.boundElements = el.boundElements.filter((b) => !deleting.has(b.id));
    }
    if (el.type === "arrow") {
      const arrow = el as MutableArrowElement;
      if (arrow.startBinding && deleting.has(arrow.startBinding.elementId)) arrow.startBinding = null;
      if (arrow.endBinding && deleting.has(arrow.endBinding.elementId)) arrow.endBinding = null;
    }
  }
  return deleting;
}

/** 绑定登记：把箭头挂到两端形状的 boundElements 上（双向绑定）。 */
export function registerArrowBindings(draft: SceneDraft, arrow: MutableArrowElement): void {
  for (const binding of [arrow.startBinding, arrow.endBinding]) {
    if (!binding) continue;
    const target = draft.get(binding.elementId);
    if (!target) continue;
    const list = (target.boundElements ?? []).filter((b) => b.id !== arrow.id);
    target.boundElements = [...list, { id: arrow.id, type: "arrow" }];
  }
}

/** 绑定登记：把文本挂到容器的 boundElements 上（并同步 frameId 归属）。 */
export function registerBoundText(container: MutableElement, text: MutableTextElement): void {
  const list = (container.boundElements ?? []).filter((b) => b.id !== text.id);
  container.boundElements = [...list, { id: text.id, type: "text" }];
  text.frameId = container.frameId;
}
