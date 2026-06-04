import type {
  AgentType,
  DispatcherMessage,
  DispatcherMessageUsageStats,
  PlanInteraction,
  ProjectMcpStatus,
} from "../../types";

// ── Constants ──────────────────────────────────────────────────────────────────

export const MESSAGE_LIST_BOTTOM_THRESHOLD = 48;
export const SEARCHABLE_CONTENT_SELECTOR = ".dispatcher-searchable-content";
export const SEARCH_MATCH_SELECTOR = "mark.dispatcher-search-match";

// ── DOM Search Utilities ───────────────────────────────────────────────────────

export function isMessageListNearBottom(element: HTMLDivElement): boolean {
  return (
    element.scrollHeight - element.scrollTop - element.clientHeight <= MESSAGE_LIST_BOTTOM_THRESHOLD
  );
}

export function unwrapConversationSearchMatches(root: HTMLElement) {
  const marks = Array.from(root.querySelectorAll(SEARCH_MATCH_SELECTOR));
  for (const mark of marks) {
    const parent = mark.parentNode;
    if (!parent) {
      continue;
    }
    parent.replaceChild(document.createTextNode(mark.textContent ?? ""), mark);
    parent.normalize();
  }
}

function createHighlightedTextFragment(
  text: string,
  query: string,
  startIndex: number,
): { fragment: DocumentFragment; nextIndex: number } {
  const fragment = document.createDocumentFragment();
  const lowerText = text.toLowerCase();
  const lowerQuery = query.toLowerCase();
  let cursor = 0;
  let matchIndex = startIndex;

  while (cursor < text.length) {
    const foundAt = lowerText.indexOf(lowerQuery, cursor);
    if (foundAt < 0) {
      fragment.append(document.createTextNode(text.slice(cursor)));
      break;
    }

    if (foundAt > cursor) {
      fragment.append(document.createTextNode(text.slice(cursor, foundAt)));
    }

    const mark = document.createElement("mark");
    mark.className = "dispatcher-search-match";
    mark.dataset.searchMatchIndex = String(matchIndex);
    mark.textContent = text.slice(foundAt, foundAt + query.length);
    fragment.append(mark);

    matchIndex += 1;
    cursor = foundAt + query.length;
  }

  return { fragment, nextIndex: matchIndex };
}

export function highlightConversationSearchMatches(root: HTMLElement, query: string) {
  const searchableNodes = Array.from(root.querySelectorAll<HTMLElement>(SEARCHABLE_CONTENT_SELECTOR));
  let nextIndex = 0;

  for (const node of searchableNodes) {
    const walker = document.createTreeWalker(node, NodeFilter.SHOW_TEXT, {
      acceptNode(textNode) {
        const value = textNode.nodeValue ?? "";
        if (!value.toLowerCase().includes(query.toLowerCase())) {
          return NodeFilter.FILTER_REJECT;
        }
        if (textNode.parentElement?.closest(SEARCH_MATCH_SELECTOR)) {
          return NodeFilter.FILTER_REJECT;
        }
        return NodeFilter.FILTER_ACCEPT;
      },
    });
    const textNodes: Text[] = [];
    while (walker.nextNode()) {
      textNodes.push(walker.currentNode as Text);
    }

    for (const textNode of textNodes) {
      const { fragment, nextIndex: updatedIndex } = createHighlightedTextFragment(
        textNode.nodeValue ?? "",
        query,
        nextIndex,
      );
      nextIndex = updatedIndex;
      textNode.replaceWith(fragment);
    }
  }

  return nextIndex;
}

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

export function withLiveElapsed(
  stats: DispatcherMessageUsageStats | null,
  receivedAt: number,
  now: number,
): DispatcherMessageUsageStats | null {
  if (!stats) {
    return null;
  }

  return {
    ...stats,
    elapsedMs: stats.elapsedMs + Math.max(0, now - receivedAt),
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

// ── Composer Helpers ───────────────────────────────────────────────────────────

export function getComposerButtonLabel(mode: "send" | "stop" | "resume", hasInput: boolean): string {
  if (mode === "stop") return "停止";
  if (mode === "resume" && !hasInput) return "继续运行";
  return mode === "send" ? "开始对话" : "发送消息";
}

export function isComposerActionDisabled(
  mode: "send" | "stop" | "resume",
  input: string,
  isBusy: boolean,
  isStopping: boolean,
  hasImages = false,
): boolean {
  if (mode === "stop") return isStopping;
  if (mode === "resume") return (!input.trim() && !hasImages && isBusy) || isStopping;
  return (!input.trim() && !hasImages) || isBusy || isStopping;
}

export function getPrimaryComposerOpacity(
  mode: "send" | "stop" | "resume",
  input: string,
  isBusy: boolean,
  isStopping: boolean,
  hasImages = false,
): number {
  return isComposerActionDisabled(mode, input, isBusy, isStopping, hasImages) ? 0.45 : 1;
}

export function getMcpIndicatorState(
  mcpStatus: ProjectMcpStatus | null,
  mcpChecking: boolean,
): { color: string; label: string } {
  if (mcpChecking) {
    return { color: "#d97706", label: "检查中" };
  }
  if (!mcpStatus || mcpStatus.aggregate === "not_configured") {
    return { color: "var(--text-hint)", label: "未配置" };
  }
  if (mcpStatus.aggregate === "healthy") {
    return { color: "#1f9d55", label: "正常" };
  }
  return { color: "#dc2626", label: "异常" };
}

export function getSubProcessAgentLabel(agent: AgentType): string {
  return agent === "claude" ? "Claude" : "Codex";
}

export function mergeDispatcherMessages(
  current: DispatcherMessage[],
  incoming: DispatcherMessage[],
): DispatcherMessage[] {
  if (incoming.length === 0) return current;
  const merged = new Map(current.map((m) => [m.id, m] as const));
  for (const m of incoming) merged.set(m.id, m);
  return [...merged.values()].sort((a, b) => {
    const cmp = a.createdAt.localeCompare(b.createdAt);
    return cmp !== 0 ? cmp : a.id.localeCompare(b.id);
  });
}

// ── Plan Prompt Builders (public API) ──────────────────────────────────────────

export function buildPlanQuestionAnswer(
  interaction: Extract<PlanInteraction, { kind: "question" }>,
  answer: string,
) {
  return [
    "[规划问题答复]",
    `问题：${interaction.question}`,
    answer,
    "",
    "请基于以上答复继续完善计划书；如果仍缺关键信息，可以继续提问。",
  ].join("\n");
}

export function buildPlanImplementationPrompt(planPath: string) {
  return [
    "请实施已确认的 Plan 计划书。",
    "",
    `计划书路径：${planPath}`,
    "",
    "请考虑计划书中的实际任务内容，按照 Claude 和 Codex 各自擅长点派遣子任务：Claude 优先处理新功能、探索和快速实现，Codex 优先处理重构、结构治理和高风险一致性修改。",
    "不要重新规划步骤，也不要调用 update_plan。提示子 Agent 按照上述计划书路径中的规划 MD 进行编码任务即可；子 Agent 需要自行读取该计划书。",
    "派遣后等待执行结束，汇总验证结果。实施完成并验证后，调用 mark_plan_implemented 标记计划已实现。",
  ].join("\n");
}
