/**
 * arch-geometry.ts —— 画布程序应用层的几何/样式纯助手（自 arch-apply.ts 拆出）。
 *
 * 全部为无状态函数（编辑器作为入参传入），与「指令应用」职责分离，便于独立
 * 测试；`AutoPlaceCursor` 由 arch-apply.ts 重导出以保持消费方不变。
 */

import type { Editor, TLShapeId } from "tldraw";
import type { ArchArrowStyleProps, ArchStyleProps } from "./arch-program";

/** 自动放置游标：首个形状视口中心，其后依次右移；容器内按槽位级联。 */
export interface AutoPlaceCursor {
  x: number;
  y: number;
  placed: boolean;
  /** 每个 frame 已自动放置的形状数（容器内槽位级联用）。 */
  frameCounts: Map<string, number>;
}

const AUTO_PLACE_STEP_X = 240;

/** frame 内自动放置：标题栏下方起排，4 列槽位级联（页面坐标）。 */
const FRAME_PLACE_PAD_X = 24;
const FRAME_PLACE_PAD_Y = 60;
const FRAME_PLACE_STEP_X = 208;
const FRAME_PLACE_STEP_Y = 132;
const FRAME_PLACE_COLS = 4;

/** tldraw 页面 id 形如 `page:xxx`。 */
export function isPageId(id: string): boolean {
  return id.startsWith("page:");
}

/**
 * 可选数值字段的安全取值：仅接受有限数字。
 * 权威载荷已保证省略字段为缺席而非 `null`（Rust 序列化层归一），
 * 这里是防御层——`null`/NaN 一律视为未给出，绝不把非数字传给 tldraw。
 */
export function finite(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

export function styleProps(style: ArchStyleProps): Record<string, unknown> {
  const props: Record<string, unknown> = {};
  if (style.color) props.color = style.color;
  if (style.labelColor) props.labelColor = style.labelColor;
  if (style.fill) props.fill = style.fill;
  if (style.size) props.size = style.size;
  if (style.dash) props.dash = style.dash;
  if (style.font) props.font = style.font;
  return props;
}

/**
 * 箭头专用样式子集：DSL 约定箭头只开放 color/labelColor/size/dash
 *（权威层不会为箭头产出 fill/font，这里是事件载荷异常时的防御）。
 */
export function arrowStyleProps(style: ArchArrowStyleProps): Record<string, unknown> {
  const props: Record<string, unknown> = {};
  if (style.color) props.color = style.color;
  if (style.labelColor) props.labelColor = style.labelColor;
  if (style.size) props.size = style.size;
  if (style.dash) props.dash = style.dash;
  return props;
}

export function shapeCenter(editor: Editor, id: TLShapeId): { x: number; y: number } {
  const bounds = editor.getShapePageBounds(id);
  if (bounds) return { x: bounds.midX, y: bounds.midY };
  return { x: 0, y: 0 };
}

/**
 * DSL 的绝对坐标一律是页面坐标，而 tldraw 的 shape.x/y 位于**父容器局部
 * 坐标系**（frame 内形状需扣除容器变换，页面根形状恒等——见
 * `Editor.reparentShapes` 内部的逆变换换算）。写回前统一换算；单轴更新时
 * 另一轴取该形状当前页面坐标，避免把两套坐标系混在同一次写入里。
 */
export function absolutePositionInParentSpace(
  editor: Editor,
  id: TLShapeId,
  shape: { x: number; y: number },
  x: number | undefined,
  y: number | undefined,
): { x: number; y: number } {
  // getShapePageTransform 对现存形状总会返回变换；bounds 的 point 同为
  // 页面坐标，仅作异常兜底——shape.x/y 是父容器局部坐标，不能在这里冒充页面坐标。
  const currentPage =
    editor.getShapePageTransform(id)?.point() ??
    editor.getShapePageBounds(id)?.point ??
    { x: shape.x, y: shape.y };
  return editor.getPointInParentSpace(id, {
    x: x ?? currentPage.x,
    y: y ?? currentPage.y,
  });
}

export function autoPlacePosition(
  editor: Editor,
  cursor: AutoPlaceCursor,
  w: number,
  h: number,
): { x: number; y: number } {
  if (!cursor.placed) {
    const viewport = editor.getViewportPageBounds();
    cursor.placed = true;
    cursor.x = viewport.midX - w / 2;
    cursor.y = viewport.midY - h / 2;
    return { x: cursor.x, y: cursor.y };
  }
  cursor.x += AUTO_PLACE_STEP_X;
  return { x: cursor.x, y: cursor.y };
}

/** frame 内槽位自动放置（页面坐标）；同一容器内逐个级联。 */
export function autoPlaceInFrame(
  editor: Editor,
  cursor: AutoPlaceCursor,
  frameId: TLShapeId,
): { x: number; y: number } {
  const bounds = editor.getShapePageBounds(frameId);
  const origin = bounds ? { x: bounds.x, y: bounds.y } : { x: 0, y: 0 };
  const count = cursor.frameCounts.get(frameId) ?? 0;
  cursor.frameCounts.set(frameId, count + 1);
  const position = {
    x: origin.x + FRAME_PLACE_PAD_X + (count % FRAME_PLACE_COLS) * FRAME_PLACE_STEP_X,
    y: origin.y + FRAME_PLACE_PAD_Y + Math.floor(count / FRAME_PLACE_COLS) * FRAME_PLACE_STEP_Y,
  };
  cursor.placed = true;
  cursor.x = position.x;
  cursor.y = position.y;
  return position;
}
