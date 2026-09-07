import type {
  AnyContentSegment,
  DispatcherMessage,
  DispatcherMessageWire,
  DispatcherMessageUsageStats,
  TextSegment,
} from "../../types";

// ── Data Utilities ─────────────────────────────────────────────────────────────

export function toErrorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function createEmptyUsageStats(): DispatcherMessageUsageStats {
  return {
    promptTokens: 0,
    completionTokens: 0,
    totalTokens: 0,
    elapsedMs: 0,
  };
}

export function formatTokenGenerationSpeed(completionTokens: number, elapsedMs: number): string {
  const elapsedSeconds = elapsedMs / 1000;
  if (completionTokens <= 0 || elapsedSeconds <= 0) {
    return "0.0";
  }

  const tokensPerSecond = completionTokens / elapsedSeconds;
  if (tokensPerSecond >= 100) {
    return tokensPerSecond.toFixed(0);
  }
  return tokensPerSecond.toFixed(1);
}

export function formatTokenCountK(totalTokens: number): string {
  if (!Number.isFinite(totalTokens) || totalTokens <= 0) return "0.0k";
  const value = totalTokens / 1000;
  return `${value >= 100 ? value.toFixed(0) : value.toFixed(1)}k`;
}

export function mergeDispatcherMessages(
  current: DispatcherMessage[],
  incoming: Array<DispatcherMessage | DispatcherMessageWire>,
): DispatcherMessage[] {
  if (incoming.length === 0) return current;
  const merged = new Map(current.map((m) => [m.id, normalizeCached(m)] as const));
  for (const m of incoming) merged.set(m.id, normalizeCached(m));
  return [...merged.values()].sort((a, b) => {
    const cmp = a.createdAt.localeCompare(b.createdAt);
    return cmp !== 0 ? cmp : a.id.localeCompare(b.id);
  });
}

/**
 * 归一化按对象身份缓存（UI-24b-4）：merge 在每次 finalize / 会话更新事件
 * 都会以「上一轮结果数组」为 current 重入，旧实现对全量消息重复
 * normalize（含 wire 载荷的 JSON.parse），1000 条会话下是 O(n) 热点。
 *
 * 两级键控：① 原始 wire 对象 → 产物；② 产物对象 → 产物自身（normalize
 * 幂等：产物不含 segmentsJson、content 由 segments 派生，重算结果深度相等，
 * 直接复用产物引用可保持下游 memo 稳定）。重入 merge 时 current 携带的是
 * 上一轮产物 → 命中 ② 零开销；只有新到达的 wire 对象真正走归一化。
 * 对象被丢弃后缓存随 GC 回收。
 */
const normalizeCache = new WeakMap<object, DispatcherMessage>();

function normalizeCached(
  message: DispatcherMessage | DispatcherMessageWire,
): DispatcherMessage {
  const hit = normalizeCache.get(message);
  if (hit) return hit;
  const normalized = normalizeDispatcherMessage(message);
  normalizeCache.set(message, normalized);
  // 产物自身也登记（重入 merge 的 current 命中路径）；覆盖写产物对象
  // 为键的条目无副作用——normalize(normalized) 与 normalized 深度相等。
  normalizeCache.set(normalized, normalized);
  return normalized;
}

function normalizeDispatcherMessage(
  message: DispatcherMessage | DispatcherMessageWire,
): DispatcherMessage {
  const segments =
    "segmentsJson" in message ? parseSegmentsJson(message.segmentsJson) : message.segments;
  // 正文一律从 segments 派生（与后端 segments_to_plain_text 同语义：
  // 过滤纯空白文本段、以换行连接）；wire 载荷不携带独立正文字段。
  const content = textFromSegments(segments);

  return {
    ...message,
    segments,
    content,
  };
}

function parseSegmentsJson(raw: string | undefined): AnyContentSegment[] {
  if (!raw || !raw.trim()) return [];
  try {
    const parsed: unknown = JSON.parse(raw);
    return Array.isArray(parsed) ? parsed.filter(isContentSegment) : [];
  } catch (error) {
    console.error("解析 dispatcher 消息 segmentsJson 失败:", error);
    return [];
  }
}

function isContentSegment(segment: unknown): segment is AnyContentSegment {
  if (!segment || typeof segment !== "object") return false;
  const type = (segment as { type?: unknown }).type;
  return type === "text" || type === "image" || type === "file";
}

function textFromSegments(segments: AnyContentSegment[]): string {
  return segments
    .filter((segment): segment is TextSegment => segment.type === "text")
    .map((segment) => segment.text)
    .filter((text) => text.trim().length > 0)
    .join("\n");
}
