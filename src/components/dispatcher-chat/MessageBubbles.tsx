import { memo, useMemo, useState, useEffect, useCallback } from "react";
import { Check, Copy, Brain, ChevronDown, ChevronRight } from "lucide-react";
import type { DispatcherMessage, DispatcherMessageUsageStats, PythonCodeRunRecord } from "../../types";
import type { PythonCodeRunTarget } from "../../types";
import {
  type AssistantThinkingBlock,
  type AssistantTurnSegment,
} from "../dispatcherChatView";
import { MarkdownRenderer } from "../markdown/MarkdownRenderer";
import { ToolActivityBubble, type ToolActivityItem } from "../ToolActivityBubble";
import { formatElapsedMmSs, formatTokenCount } from "../../utils";
import assistantAvatarUrl from "../../assets/dispatcher-assistant-avatar.png";
import userAvatarUrl from "../../assets/dispatcher-user-avatar.png";
import { formatTokenGenerationSpeed } from "./dispatcherChatUtils";
import { dispatcherChatStyles as styles } from "./dispatcherChatStyles";

// ── BubbleCopyButton ───────────────────────────────────────────────────────────

export const BubbleCopyButton = memo(function BubbleCopyButton({
  text,
  isUser,
}: {
  text: string;
  isUser: boolean;
}) {
  const [status, setStatus] = useState<"idle" | "copied" | "failed">("idle");
  const copied = status === "copied";
  const failed = status === "failed";

  useEffect(() => {
    if (status === "idle") {
      return;
    }

    const timer = window.setTimeout(() => setStatus("idle"), 1600);
    return () => window.clearTimeout(timer);
  }, [status]);

  const handleCopy = useCallback(async () => {
    try {
      await navigator.clipboard.writeText(text);
      setStatus("copied");
    } catch (error) {
      console.error("Failed to copy message bubble text", error);
      setStatus("failed");
    }
  }, [text]);

  return (
    <button
      type="button"
      style={styles.bubbleCopyButton(isUser, status)}
      onClick={handleCopy}
      title={copied ? "已复制气泡原文" : failed ? "复制失败" : "复制气泡原文"}
      aria-label={copied ? "已复制气泡原文" : failed ? "复制失败" : "复制气泡原文"}
    >
      {copied ? <Check size={12} /> : <Copy size={12} />}
      <span>{copied ? "已复制" : failed ? "失败" : "复制"}</span>
    </button>
  );
});

// ── UserMessageBubble ──────────────────────────────────────────────────────────

export const UserMessageBubble = memo(function UserMessageBubble({
  message,
  onRunPython,
  pythonRunRecords,
}: {
  message: DispatcherMessage;
  onRunPython?: (target: PythonCodeRunTarget) => void;
  pythonRunRecords?: Record<string, PythonCodeRunRecord>;
}) {
  return (
    <div style={styles.messageBubbleWrap(true)}>
      <div style={styles.messageAvatar(true)}>
        <img src={userAvatarUrl} alt="用户头像" style={styles.messageAvatarImage} />
      </div>
      <div style={styles.messageBubbleColumn(true)}>
        <div style={styles.messageBubble(true)}>
          <div className="dispatcher-searchable-content" style={styles.markdownBody}>
            <MarkdownRenderer
              content={message.content}
              variant="chat"
              messageId={message.id}
              onRunPython={onRunPython}
              pythonRunRecords={pythonRunRecords}
            />
          </div>
        </div>
        <BubbleCopyButton text={message.content} isUser />
      </div>
    </div>
  );
});

// ── AssistantTurnBubble ────────────────────────────────────────────────────────

export const AssistantTurnBubble = memo(function AssistantTurnBubble({
  segments,
  tools,
  workspaceId,
  usageStats,
  thinking,
  placeholderText,
  streaming = false,
  onRunPython,
  pythonRunRecords,
}: {
  segments: AssistantTurnSegment[];
  tools: ToolActivityItem[];
  workspaceId: string;
  usageStats?: DispatcherMessageUsageStats | null;
  thinking?: AssistantThinkingBlock | null;
  placeholderText?: string | null;
  streaming?: boolean;
  onRunPython?: (target: PythonCodeRunTarget) => void;
  pythonRunRecords?: Record<string, PythonCodeRunRecord>;
}) {
  const { enrichedTools, displaySegments } = useMemo(() => {
    const summaryMap = new Map<string, string>();
    const displaySegments: AssistantTurnSegment[] = [];

    for (const segment of segments) {
      if (segment.kind === "tool-summary") {
        const key = segment.toolCallId ?? segment.toolName;
        if (key) {
          const existing = summaryMap.get(key) ?? "";
          summaryMap.set(key, existing + segment.text);
        }
        continue;
      }
      if (segment.text.trim()) {
        displaySegments.push(segment);
      }
    }

    const enriched = tools.map((tool) => {
      let summary = summaryMap.get(tool.key);
      if (!summary && tool.name) {
        summary = summaryMap.get(tool.name);
      }
      if (summary) {
        return { ...tool, summaryText: summary };
      }
      return tool;
    });

    return { enrichedTools: enriched, displaySegments };
  }, [segments, tools]);

  const visibleSegments = displaySegments;
  const visiblePlaceholder = placeholderText?.trim() ?? "";
  const visibleThinking = thinking?.text.trim() ? thinking : null;
  if (
    visibleSegments.length === 0 &&
    enrichedTools.length === 0 &&
    !visiblePlaceholder &&
    !usageStats &&
    !visibleThinking
  ) {
    return null;
  }

  return (
    <div style={styles.messageBubbleWrap(false)}>
      <div style={styles.messageAvatar(false)}>
        <img src={assistantAvatarUrl} alt="AI 头像" style={styles.messageAvatarImage} />
      </div>
      <div style={styles.assistantTurnStack}>
        {enrichedTools.length > 0 && (
          <div style={styles.assistantTurnSection}>
            <ToolActivityBubble tools={enrichedTools} workspaceId={workspaceId} />
          </div>
        )}
        {visibleThinking && (
          <ThinkingBlock text={visibleThinking.text} elapsedMs={visibleThinking.elapsedMs} />
        )}
        {visiblePlaceholder && (
          <div style={styles.assistantTurnSection}>
            <div style={{ ...styles.messageBubble(false), ...styles.assistantReplyBubble }}>
              <div style={styles.assistantPlaceholder}>
                <span style={styles.assistantPlaceholderDot} />
                <span>{visiblePlaceholder}</span>
              </div>
            </div>
          </div>
        )}
        {visibleSegments.map((segment, index) => {
          const segmentText = segment.text.trim();
          const isLastSegment = index === visibleSegments.length - 1;
          const isStreamingThis = streaming && isLastSegment && segment.kind === "assistant-text";
          if (!segmentText && segment.kind !== "tool-summary") {
            return null;
          }
          return (
            <div key={`${segment.kind}-${segment.toolCallId ?? segment.toolName ?? index}`}>
              <div style={styles.assistantTurnSection}>
                <div style={{ ...styles.messageBubble(false), ...styles.assistantReplyBubble }}>
                  <div className="dispatcher-searchable-content" style={styles.markdownBody}>
                    <MarkdownRenderer
                      content={segmentText}
                      variant="chat"
                      streaming={isStreamingThis}
                      messageId={segment.messageId}
                      onRunPython={!isStreamingThis ? onRunPython : undefined}
                      pythonRunRecords={pythonRunRecords}
                    />
                  </div>
                </div>
                <BubbleCopyButton text={segmentText} isUser={false} />
              </div>
            </div>
          );
        })}
        {usageStats && <AssistantUsageStats stats={usageStats} />}
      </div>
    </div>
  );
});

// ── ThinkingBlock ──────────────────────────────────────────────────────────────

export const ThinkingBlock = memo(function ThinkingBlock({
  text,
  elapsedMs,
}: {
  text: string;
  elapsedMs: number;
}) {
  const [expanded, setExpanded] = useState(false);
  const trimmedText = text.trim();
  if (!trimmedText) {
    return null;
  }

  return (
    <div style={styles.assistantTurnSection}>
      <div style={styles.thinkingCard}>
        <button
          type="button"
          style={styles.thinkingHeader}
          onClick={() => setExpanded((value) => !value)}
          aria-expanded={expanded}
          title={expanded ? "收起思考过程" : "展开思考过程"}
        >
          <span style={styles.thinkingBadge}>
            <Brain size={13} />
            Think
          </span>
          <span style={styles.thinkingMeta}>思考 {formatElapsedMmSs(elapsedMs)}</span>
          {expanded ? <ChevronDown size={15} /> : <ChevronRight size={15} />}
        </button>
        {expanded && (
          <div className="dispatcher-searchable-content" style={styles.thinkingBody}>
            <MarkdownRenderer content={trimmedText} variant="chat" />
          </div>
        )}
      </div>
    </div>
  );
});

// ── AssistantUsageStats ────────────────────────────────────────────────────────

export const AssistantUsageStats = memo(function AssistantUsageStats({
  stats,
}: {
  stats: DispatcherMessageUsageStats;
}) {
  const tokenSpeed = formatTokenGenerationSpeed(stats.completionTokens, stats.elapsedMs);

  return (
    <div style={styles.assistantUsageStats} title="来自模型标准 usage 字段">
      <span>总 {formatTokenCount(stats.totalTokens)}</span>
      <span>输入 {formatTokenCount(stats.promptTokens)}</span>
      <span>输出 {formatTokenCount(stats.completionTokens)}</span>
      <span>耗时：{formatElapsedMmSs(stats.elapsedMs)}</span>
      <span>速度：{tokenSpeed} t/s</span>
    </div>
  );
});
