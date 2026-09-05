/**
 * 双重感知之结构化通道：把画布现状投影为给模型的文本快照。
 *
 * `formatCanvasSnapshot` 为纯投影函数（可注入假数据单测）；
 * `collectCanvasSnapshot` 负责从 tldraw editor 读取原始数据。
 *
 * 快照行格式（与后端系统提示词 `prompt.rs` 的感知章节同步维护）：
 * - 形状：`[形状id] 类型 "文本" x=.. y=.. w=.. h=.. [parent=形状id] [locked]`
 * - 箭头：`[形状id] arrow "标注" from=形状id to=形状id`（缺端标注该端未连接；
 *   两端皆未连接的自由箭头退回位置尺寸表示）
 * - 头部：页面/视口/形状数；用户当前选中的形状以「选中: …」标注。
 */

import type { Editor, TLArrowBinding, TLShape } from "tldraw";

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

function shapeText(editor: Editor, shape: TLShape): string | undefined {
  try {
    return editor.getShapeUtil(shape).getText(shape);
  } catch {
    return undefined;
  }
}

/** tldraw 页面 id 形如 `page:xxx`；形状/绑定 id 形如 `shape:xxx`。 */
function isPageId(id: string): boolean {
  return id.startsWith("page:");
}

/** 读箭头的两端绑定（binding：fromId=箭头，toId=被指形状，terminal 区分端）。 */
function arrowEndsFor(editor: Editor, shape: TLShape): { from?: string; to?: string } | undefined {
  if (shape.type !== "arrow") return undefined;
  let bindings: TLArrowBinding[];
  try {
    bindings = editor.getBindingsFromShape(shape.id, "arrow");
  } catch {
    return undefined;
  }
  const ends: { from?: string; to?: string } = {};
  for (const binding of bindings) {
    if (binding.props.terminal === "start") ends.from = binding.toId;
    else if (binding.props.terminal === "end") ends.to = binding.toId;
  }
  return ends.from || ends.to ? ends : undefined;
}

/** 从 editor 收集快照输入并投影为文本；空画布返回空串。 */
export function collectCanvasSnapshot(editor: Editor): string {
  const shapes = editor.getCurrentPageShapes();
  if (shapes.length === 0) return "";

  const viewportBox = editor.getViewportPageBounds();
  // 两阶段收集控制大画布的同步成本：先只取轻量字段（bounds 是阅读序的
  // 排序依据），截断到上限后才对幸存形状做 getText / bindings 等重提取。
  // formatCanvasSnapshot 的输入集合与排序依据不变，输出与全量收集一致。
  const shapeById = new Map<string, TLShape>();
  const lightInputs: SnapshotShapeInput[] = [];
  for (const shape of shapes) {
    const bounds = editor.getShapePageBounds(shape);
    if (!bounds) continue;
    shapeById.set(shape.id, shape);
    const parentId =
      typeof shape.parentId === "string" && !isPageId(shape.parentId) ? shape.parentId : undefined;
    lightInputs.push({
      id: shape.id,
      type: shape.type,
      bounds: { x: bounds.x, y: bounds.y, w: bounds.w, h: bounds.h },
      parentId,
      locked: shape.isLocked || undefined,
    });
  }
  const listedIds = new Set(
    readingOrder(lightInputs)
      .slice(0, MAX_SNAPSHOT_SHAPES)
      .map((input) => input.id),
  );
  const shapeInputs = lightInputs.map((input) => {
    const shape = listedIds.has(input.id) ? shapeById.get(input.id) : undefined;
    if (!shape) return input;
    return { ...input, text: shapeText(editor, shape), arrowEnds: arrowEndsFor(editor, shape) };
  });

  return formatCanvasSnapshot({
    pageId: editor.getCurrentPageId(),
    shapes: shapeInputs,
    viewport: {
      x: viewportBox.x,
      y: viewportBox.y,
      w: viewportBox.w,
      h: viewportBox.h,
    },
    selectedIds: editor.getSelectedShapeIds(),
  });
}
