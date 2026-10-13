import type { SubAgentEvent, SubAgentEventPayload, SubAgentUsage } from "../types";

type LegacyUsage = {
  prompt_tokens: number;
  completion_tokens: number;
  total_tokens: number;
};

/** 新事件使用 camelCase；已保存的旧轨迹仍可带 Rust 原始字段名。 */
export function normalizeSubAgentUsage(usage: SubAgentUsage | LegacyUsage): SubAgentUsage {
  return "prompt_tokens" in usage
    ? {
        promptTokens: usage.prompt_tokens,
        completionTokens: usage.completion_tokens,
        totalTokens: usage.total_tokens,
      }
    : usage;
}

function canonicalJson(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
  if (value && typeof value === "object") {
    return `{${Object.entries(value)
      .sort(([left], [right]) => left.localeCompare(right))
      .map(([key, item]) => `${JSON.stringify(key)}:${canonicalJson(item)}`)
      .join(",")}}`;
  }
  return JSON.stringify(value) ?? "null";
}

/** 同毫秒内先回放 checkpoint 再应用新增实时事件，字段顺序不同不制造重复动作。 */
export function mergeSubAgentTraceEvents(
  checkpoint: SubAgentEvent[],
  live: SubAgentEventPayload[],
): SubAgentEvent[] {
  function normalize(event: SubAgentEvent): SubAgentEvent {
    return {
      event: event.event,
      timestampMs: event.timestampMs,
      data: {
        ...event.data,
        ...(event.data.tokenUsage
          ? { tokenUsage: normalizeSubAgentUsage(event.data.tokenUsage) }
          : {}),
      },
    };
  }
  const saved = checkpoint.map(normalize);
  // 终态轨迹是完整权威快照。普通 Rig 会聚合连续文本增量，不能再叠加原始实时增量。
  if (saved.some((event) => event.event === "Finished" || event.event === "Failed")) {
    return saved;
  }
  const overlaps = new Map<string, number>();
  for (const event of saved) {
    const identity = canonicalJson(event);
    overlaps.set(identity, (overlaps.get(identity) ?? 0) + 1);
  }
  // 只抵消两个来源的重叠；同一来源中同毫秒的重复文本增量必须完整保留。
  const additions = live.map(normalize).filter((event) => {
    const identity = canonicalJson(event);
    const count = overlaps.get(identity) ?? 0;
    if (count > 0) {
      overlaps.set(identity, count - 1);
      return false;
    }
    return true;
  });
  return [...saved, ...additions].sort(
    (left, right) => (left.timestampMs ?? 0) - (right.timestampMs ?? 0),
  );
}
