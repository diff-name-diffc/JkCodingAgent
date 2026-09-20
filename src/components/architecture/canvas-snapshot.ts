/**
 * 双重感知之结构化通道：把画布现状投影为给模型的文本快照。
 *
 * `formatCanvasSnapshot` 为纯投影函数（可注入假数据单测）；
 * `collectCanvasSnapshot` 负责从 Excalidraw 场景（api + 元素数组）读取原始数据。
 *
 * 快照行格式（与后端系统提示词 `prompt.rs` 的感知章节同步维护）：
 * - 形状：`[形状id] 类型 "文本" x=.. y=.. w=.. h=.. [parent=形状id] [locked]`
 * - 箭头：`[形状id] arrow "标注" from=形状id to=形状id`（缺端标注该端未连接；
 *   两端皆未连接的自由箭头退回位置尺寸表示）
 * - 头部：页面/视口/形状数；用户当前选中的形状以「选中: …」标注。
 */

import type { ExcalidrawImperativeAPI } from "@excalidraw/excalidraw/types";
import type {
  ExcalidrawArrowElement,
  ExcalidrawElement,
  ExcalidrawTextElement,
} from "@excalidraw/excalidraw/element/types";
import { ARCH_KIND_KEY } from "./program/exc-factory";

export const MAX_SNAPSHOT_SHAPES = 150;
const MAX_SNAPSHOT_TEXT_CHARS = 60;

export interface SnapshotShapeInput {
  id: string;
  type: string;
  text?: string;
  bounds: { x: number; y: number; w: number; h: number };
  /** 父容器（frame/group）；页面根为 undefined。 */
  parentId?: string;
  locked?: boolean;
  /** 仅箭头：两端连接的形状；两端都未连接的自由箭头为 undefined。 */
  arrowEnds?: { from?: string; to?: string };
}

export interface SnapshotInput {
  pageId: string;
  shapes: SnapshotShapeInput[];
  viewport: { x: number; y: number; w: number; h: number };
  /** 用户当前选中的形状 id（空 = 无选中）。 */
  selectedIds?: string[];
}

/** 快照数值取整显示（坐标/尺寸为整数像素即可读，无小数语义）。
 * 原名 round1 有误导——实现一直是取整而非保留一位小数。 */
function roundInt(value: number): number {
  return Math.round(value);
}

function truncateText(text: string): string {
  const chars = [...text];
  if (chars.length <= MAX_SNAPSHOT_TEXT_CHARS) return text;
  return `${chars.slice(0, MAX_SNAPSHOT_TEXT_CHARS).join("")}…`;
}

function intersects(
  bounds: SnapshotShapeInput["bounds"],
  viewport: SnapshotInput["viewport"],
): boolean {
  return (
    bounds.x < viewport.x + viewport.w &&
    bounds.x + bounds.w > viewport.x &&
    bounds.y < viewport.y + viewport.h &&
    bounds.y + bounds.h > viewport.y
  );
}

/** 阅读序（先上后下、同行先左后右；y 按 32px 分桶容忍轻微错位）。 */
function readingOrder(shapes: SnapshotShapeInput[]): SnapshotShapeInput[] {
  return [...shapes].sort((a, b) => {
    const rowA = Math.round(a.bounds.y / 32);
    const rowB = Math.round(b.bounds.y / 32);
    if (rowA !== rowB) return rowA - rowB;
    return a.bounds.x - b.bounds.x;
  });
}

/** 单行快照投影：箭头优先用连接关系表达，自由箭头退回位置尺寸。 */
function shapeLine(shape: SnapshotShapeInput, viewport: SnapshotInput["viewport"]): string {
  const parts: string[] = [`[${shape.id}] ${shape.type}`];
  if (shape.text?.trim()) parts.push(`"${truncateText(shape.text.trim())}"`);

  const ends = shape.arrowEnds;
  const hasArrowEnds = shape.type === "arrow" && ends && (ends.from || ends.to);
  if (hasArrowEnds) {
    parts.push(`from=${ends.from ?? "none"} to=${ends.to ?? "none"}`);
  } else {
    const { bounds } = shape;
    parts.push(
      `x=${roundInt(bounds.x)} y=${roundInt(bounds.y)} w=${roundInt(bounds.w)} h=${roundInt(bounds.h)}`,
    );
  }

  if (shape.parentId) parts.push(`parent=${shape.parentId}`);
  if (shape.locked) parts.push("locked");
  if (!intersects(shape.bounds, viewport)) parts.push("（视口外）");
  return parts.join(" ");
}

/** 纯投影：shapes + viewport → 快照文本。空画布返回空串。 */
export function formatCanvasSnapshot(input: SnapshotInput): string {
  const { shapes, viewport, pageId } = input;
  if (shapes.length === 0) return "";

  let header = `[画布快照] 页面: ${pageId} | 视口: (${roundInt(viewport.x)},${roundInt(viewport.y)},${roundInt(viewport.w)}×${roundInt(viewport.h)}) | 形状数: ${shapes.length}`;
  if (input.selectedIds && input.selectedIds.length > 0) {
    header += ` | 选中: ${input.selectedIds.join(", ")}（用户当前选中）`;
  }
  const lines: string[] = [header];

  const ordered = readingOrder(shapes);
  const listed = ordered.slice(0, MAX_SNAPSHOT_SHAPES);
  for (const shape of listed) {
    lines.push(shapeLine(shape, viewport));
  }
  if (ordered.length > listed.length) {
    lines.push(`…另有 ${ordered.length - listed.length} 个形状未列出`);
  }
  return lines.join("\n");
}

/** 元素的快照类型名：note 经 customData 还原语义（渲染上是黄底矩形）。 */
function snapshotType(el: ExcalidrawElement): string {
  const archKind = el.customData?.[ARCH_KIND_KEY];
  if (archKind === "note") return "note";
  return el.type;
}

/** 元素文本：容器取绑定文本，frame 取标题名，text 元素取正文。 */
function elementText(el: ExcalidrawElement, byId: Map<string, ExcalidrawElement>): string | undefined {
  if (el.type === "text") {
    // 容器绑定文本不单列（其文本随容器行展示），自由文本取正文。
    return (el as ExcalidrawTextElement).containerId ? undefined : (el as ExcalidrawTextElement).text;
  }
  if (el.type === "frame") return (el as { name?: string }).name ?? undefined;
  const binding = el.boundElements?.find((b) => b.type === "text");
  if (!binding) return undefined;
  const text = byId.get(binding.id);
  return text?.type === "text" ? (text as ExcalidrawTextElement).text : undefined;
}

/** 读箭头的两端绑定（Excalidraw 单向存储在箭头自身）。 */
function arrowEndsOf(el: ExcalidrawElement): { from?: string; to?: string } | undefined {
  if (el.type !== "arrow") return undefined;
  const arrow = el as ExcalidrawArrowElement;
  const ends = {
    from: arrow.startBinding?.elementId,
    to: arrow.endBinding?.elementId,
  };
  return ends.from || ends.to ? ends : undefined;
}

/** 从 Excalidraw api 收集快照输入并投影为文本；空画布返回空串。 */
export function collectCanvasSnapshot(api: ExcalidrawImperativeAPI): string {
  const elements = api.getSceneElements();
  // 容器绑定文本是形状的实现细节，不作为独立形状行进入快照。
  const shapes = elements.filter(
    (el) => !(el.type === "text" && (el as ExcalidrawTextElement).containerId),
  );
  if (shapes.length === 0) return "";

  const appState = api.getAppState();
  const zoom = appState.zoom.value || 1;
  // viewport = (scene + scroll) * zoom → 视口场景矩形原点为 -scroll
  const viewport = {
    x: -appState.scrollX,
    y: -appState.scrollY,
    w: appState.width / zoom,
    h: appState.height / zoom,
  };

  const byId = new Map(elements.map((el) => [el.id, el]));
  const lightInputs: SnapshotShapeInput[] = [];
  for (const el of shapes) {
    lightInputs.push({
      id: el.id,
      type: snapshotType(el),
      bounds: { x: el.x, y: el.y, w: el.width, h: el.height },
      parentId: el.frameId ?? undefined,
      locked: el.locked || undefined,
    });
  }
  const listedIds = new Set(
    readingOrder(lightInputs)
      .slice(0, MAX_SNAPSHOT_SHAPES)
      .map((input) => input.id),
  );
  const shapeInputs = lightInputs.map((input) => {
    const el = listedIds.has(input.id) ? byId.get(input.id) : undefined;
    if (!el) return input;
    return { ...input, text: elementText(el, byId), arrowEnds: arrowEndsOf(el) };
  });

  return formatCanvasSnapshot({
    pageId: "page",
    shapes: shapeInputs,
    viewport,
    selectedIds: Object.keys(appState.selectedElementIds ?? {}),
  });
}
