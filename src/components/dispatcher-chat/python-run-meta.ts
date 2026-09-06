import type { PythonCodeRunRecord } from "../../types";

/**
 * Python 运行元信息纯函数（UI-20）：耗时/来源/开始时间统一由记录字段推导。
 * 记录无 exitCode 字段——终态语义由 status + errorReason 承担（不虚构退出码）。
 */

/**
 * 运行耗时：终态（done/failed/stopped）= updatedAt − createdAt；
 * running = nowMs − createdAt（调用方以 1s 间隔驱动 nowMs，仅抽屉打开时挂载）。
 * 非法日期返回 null（不显示，不猜 0）。
 */
export function formatPythonRunDuration(
  record: Pick<PythonCodeRunRecord, "status" | "createdAt" | "updatedAt">,
  nowMs: number = Date.now(),
): string | null {
  const start = Date.parse(record.createdAt);
  if (Number.isNaN(start)) return null;
  const end = record.status === "running" ? nowMs : Date.parse(record.updatedAt);
  if (Number.isNaN(end)) return null;
  const ms = Math.max(0, end - start);
  if (ms < 1000) return `${ms}ms`;
  const secs = ms / 1000;
  if (secs < 60) return `${secs.toFixed(1)}s`;
  return `${Math.floor(secs / 60)}m ${Math.round(secs % 60)}s`;
}

/** 来源信息：消息定位（id 前 8 位）+ 代码块序号（1 起）。 */
export function pythonRunSourceLabel(source: {
  messageId: string;
  codeBlockIndex: number;
}): string {
  return `消息 ${source.messageId.slice(0, 8)} · 代码块 #${source.codeBlockIndex + 1}`;
}

/** 开始时间（本地时区 HH:MM:SS）；非法日期返回 null。 */
export function formatPythonRunStartedAt(record: { createdAt: string }): string | null {
  const start = Date.parse(record.createdAt);
  if (Number.isNaN(start)) return null;
  const date = new Date(start);
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`;
}
