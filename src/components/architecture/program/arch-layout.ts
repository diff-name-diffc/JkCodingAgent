/**
 * 声明式布局的纯函数几何计算（layout 指令的执行核心）。
 *
 * 输入为形状当前的宽高与布局参数，输出每个形状新的**左上角页坐标**。
 * 与 tldraw 解耦，便于单测：执行器负责量取尺寸、构造 origin、写回坐标。
 */

import type { ArchLayoutAlign, ArchLayoutMode } from "./arch-program";

export interface LayoutItem {
  id: string;
  w: number;
  h: number;
}

export interface LayoutOptions {
  mode: ArchLayoutMode;
  /** 布局区锚点（左上角）。执行器默认传 targets 当前包围盒左上角。 */
  origin: { x: number; y: number };
  /** 间距，默认 40。 */
  gap?: number;
  /** 仅 grid：列数，默认 ⌈√n⌉。 */
  columns?: number;
  /** 交叉轴对齐（row 的垂直 / column 的水平 / grid 的行内垂直），默认 center。 */
  align?: ArchLayoutAlign;
}

export const DEFAULT_LAYOUT_GAP = 40;

function crossOffset(align: ArchLayoutAlign, slot: number, size: number): number {
  switch (align) {
    case "start":
      return 0;
    case "end":
      return slot - size;
    case "center":
    default:
      return (slot - size) / 2;
  }
}

/** 按布局参数计算每个形状的新左上角坐标。 */
export function layoutShapes(
  items: LayoutItem[],
  options: LayoutOptions,
): Map<string, { x: number; y: number }> {
  const positions = new Map<string, { x: number; y: number }>();
  if (items.length === 0) return positions;

  const gap = options.gap ?? DEFAULT_LAYOUT_GAP;
  const align = options.align ?? "center";
  const { x: originX, y: originY } = options.origin;

  if (options.mode === "row") {
    const maxH = Math.max(...items.map((item) => item.h));
    let x = originX;
    for (const item of items) {
      positions.set(item.id, { x, y: originY + crossOffset(align, maxH, item.h) });
      x += item.w + gap;
    }
    return positions;
  }

  if (options.mode === "column") {
    const maxW = Math.max(...items.map((item) => item.w));
    let y = originY;
    for (const item of items) {
      positions.set(item.id, { x: originX + crossOffset(align, maxW, item.w), y });
      y += item.h + gap;
    }
    return positions;
  }

  // grid：列宽取该列最大宽、行高取该行最大高，逐项累加。
  const count = items.length;
  const columns = Math.min(Math.max(options.columns ?? Math.ceil(Math.sqrt(count)), 1), count);
  const rowCount = Math.ceil(count / columns);

  const colWidths = new Array<number>(columns).fill(0);
  const rowHeights = new Array<number>(rowCount).fill(0);
  items.forEach((item, index) => {
    const row = Math.floor(index / columns);
    const col = index % columns;
    colWidths[col] = Math.max(colWidths[col], item.w);
    rowHeights[row] = Math.max(rowHeights[row], item.h);
  });

  const rowY = new Array<number>(rowCount).fill(originY);
  for (let row = 1; row < rowCount; row += 1) {
    rowY[row] = rowY[row - 1] + rowHeights[row - 1] + gap;
  }
  const colX = new Array<number>(columns).fill(originX);
  for (let col = 1; col < columns; col += 1) {
    colX[col] = colX[col - 1] + colWidths[col - 1] + gap;
  }

  items.forEach((item, index) => {
    const row = Math.floor(index / columns);
    const col = index % columns;
    positions.set(item.id, {
      x: colX[col],
      y: rowY[row] + crossOffset(align, rowHeights[row], item.h),
    });
  });
  return positions;
}
