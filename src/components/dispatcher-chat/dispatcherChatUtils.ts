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
  const merged = new Map(current.map((m) => [m.id, normalizeDispatcherMessage(m)] as const));
  for (const m of incoming) merged.set(m.id, normalizeDispatcherMessage(m));
  return [...merged.values()].sort((a, b) => {
    const cmp = a.createdAt.localeCompare(b.createdAt);
    return cmp !== 0 ? cmp : a.id.localeCompare(b.id);
  });
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
