import type {
  AnyContentSegment,
  DispatcherMessage,
  DispatcherMessageUsageStats,
  ProjectMcpStatus,
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

export function getMcpIndicatorState(
  mcpStatus: ProjectMcpStatus | null,
  mcpChecking: boolean,
): { color: string; label: string } {
  if (mcpChecking) {
    return { color: "var(--warning)", label: "检查中" };
  }
  if (!mcpStatus || mcpStatus.aggregate === "not_configured") {
    return { color: "var(--text-hint)", label: "未配置" };
  }
  if (mcpStatus.aggregate === "healthy") {
    return { color: "var(--success)", label: "正常" };
  }
  return { color: "var(--danger)", label: "异常" };
}

export function mergeDispatcherMessages(
  current: DispatcherMessage[],
  incoming: DispatcherMessage[],
): DispatcherMessage[] {
  if (incoming.length === 0) return current;
  const merged = new Map(current.map((m) => [m.id, normalizeDispatcherMessage(m)] as const));
  for (const m of incoming) merged.set(m.id, normalizeDispatcherMessage(m));
  return [...merged.values()].sort((a, b) => {
    const cmp = a.createdAt.localeCompare(b.createdAt);
    return cmp !== 0 ? cmp : a.id.localeCompare(b.id);
  });
}

type DispatcherMessageWire = Omit<DispatcherMessage, "segments" | "content"> & {
  segments?: AnyContentSegment[];
  content?: string;
  segmentsJson?: string;
};

export function normalizeDispatcherMessage(message: DispatcherMessageWire): DispatcherMessage {
  const segments = Array.isArray(message.segments)
    ? message.segments
    : parseSegmentsJson(message.segmentsJson);
  const content = typeof message.content === "string" ? message.content : textFromSegments(segments);

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
    .join("\n");
}
