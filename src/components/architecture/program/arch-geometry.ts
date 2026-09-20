/**
 * arch-geometry.ts —— 画布程序应用层的几何纯助手。
 *
 * Excalidraw 元素坐标恒为场景绝对坐标（frame 子元素亦然），tldraw 时代的
 * 父容器局部坐标换算整体删除；自动放置只依赖显式传入的视口/容器包围盒，
 * 保持纯函数、可独立测试。DSL → Excalidraw 的样式映射见 exc-style.ts。
 */

/** 自动放置游标：首个形状视口中心，其后依次右移；容器内按槽位级联。 */
export interface AutoPlaceCursor {
  x: number;
  y: number;
  placed: boolean;
  /** 每个 frame 已自动放置的形状数（容器内槽位级联用）。 */
  frameCounts: Map<string, number>;
}

const AUTO_PLACE_STEP_X = 240;

/** frame 内自动放置：标题栏下方起排，4 列槽位级联（场景坐标）。 */
const FRAME_PLACE_PAD_X = 24;
const FRAME_PLACE_PAD_Y = 60;
const FRAME_PLACE_STEP_X = 208;
const FRAME_PLACE_STEP_Y = 132;
const FRAME_PLACE_COLS = 4;

/**
 * 可选数值字段的安全取值：仅接受有限数字。
 * 权威载荷已保证省略字段为缺席而非 `null`（Rust 序列化层归一），
 * 这里是防御层——`null`/NaN 一律视为未给出。
 */
export function finite(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

export function autoPlacePosition(
  viewportCenter: { x: number; y: number },
  cursor: AutoPlaceCursor,
  w: number,
  h: number,
): { x: number; y: number } {
  if (!cursor.placed) {
    cursor.placed = true;
    cursor.x = viewportCenter.x - w / 2;
    cursor.y = viewportCenter.y - h / 2;
    return { x: cursor.x, y: cursor.y };
  }
  cursor.x += AUTO_PLACE_STEP_X;
  return { x: cursor.x, y: cursor.y };
}

/** frame 内槽位自动放置（场景坐标）；同一容器内逐个级联。 */
export function autoPlaceInFrame(
  frameBounds: { x: number; y: number } | undefined,
  cursor: AutoPlaceCursor,
  frameId: string,
): { x: number; y: number } {
  const origin = frameBounds ?? { x: 0, y: 0 };
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
