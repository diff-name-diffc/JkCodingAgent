/**
 * 画板文件列表的纯函数维护（无 Excalidraw / Tauri 依赖，便于单测）。
 *
 * 列表状态由前端本地维护：保存/重命名/打开后 upsert 元数据、删除后移除，
 * 始终按更新时间倒序，避免每次变更都重新拉全量列表。
 */

import type { ArchitectureCanvasMeta } from "../../types/architecture";

/** upsert 画板元数据并按更新时间倒序（同 id 覆盖，新条目随时间自然置顶）。 */
export function upsertCanvasMeta(
  list: readonly ArchitectureCanvasMeta[],
  meta: ArchitectureCanvasMeta,
): ArchitectureCanvasMeta[] {
  const next = [meta, ...list.filter((item) => item.id !== meta.id)];
  next.sort((a, b) => b.updatedAt - a.updatedAt);
  return next;
}

/** 移除画板元数据（删除后调用）。 */
export function removeCanvasMeta(
  list: readonly ArchitectureCanvasMeta[],
  canvasId: string,
): ArchitectureCanvasMeta[] {
  return list.filter((item) => item.id !== canvasId);
}

/** 画板列表项的更新时间展示格式（yyyy-MM-dd HH:mm，本地时区）。 */
export function formatCanvasTime(timestampMs: number): string {
  const date = new Date(timestampMs);
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(
    date.getHours(),
  )}:${pad(date.getMinutes())}`;
}
