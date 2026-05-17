import {
  useState,
  useRef,
  useEffect,
  useCallback,
  useImperativeHandle,
  forwardRef,
  memo,
  useMemo,
} from "react";
import type { KeyboardEvent } from "react";
import { invoke, Channel, convertFileSrc } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  Play,
  Sparkles,
  Send,
  Square,
  X,
  Wrench,
  Settings2,
  PlugZap,
  Mic,
  ClipboardList,
  FileText,
  Check,
  Copy,
  Brain,
  ChevronDown,
  ChevronRight,
} from "lucide-react";
import type {
  AgentType,
  BrowserStatus,
  ChecklistPlanState,
  DispatchFeedbackState,
  DispatcherMessage,
  DispatcherMessageUsageStats,
  DispatcherAgentEvent,
  DispatcherAgentTurn,
  DispatcherMode,
  DispatcherSessionRuntimeState,
  DispatcherSessionTokenUsage,
  DispatcherSettings,
  ImageSegment,
  PlanInteraction,
  ProjectMcpStatus,
  SubProcess,
} from "../types";
import assistantAvatarUrl from "../assets/dispatcher-assistant-avatar.png";
import userAvatarUrl from "../assets/dispatcher-user-avatar.png";
import { useDashScopeAsr } from "../hooks/useDashScopeAsr";
import { useDispatcherSessionTokenUsage } from "../hooks/useDispatcherSessionTokenUsage";
import { formatElapsedMmSs, formatTokenCount, isImeComposing } from "../utils";
import { SessionTokenUsageIndicators } from "./SessionTokenUsageIndicators";
import { ToolActivityBubble, type ToolActivityItem } from "./ToolActivityBubble";
import { MarkdownRenderer } from "./markdown/MarkdownRenderer";
import {
  appendAssistantTextSegment,
  appendToolSummarySegment,
  type AssistantThinkingBlock,
  type AssistantTurnSegment,
  buildDispatcherDisplayItems,
  finishLiveToolActivity,
  planLiveToolActivity,
  startLiveToolActivity,
  updateLiveBrowserToolActivity,
} from "./dispatcherChatView";

const MESSAGE_LIST_BOTTOM_THRESHOLD = 48;

function isMessageListNearBottom(element: HTMLDivElement): boolean {
  return (
    element.scrollHeight - element.scrollTop - element.clientHeight <= MESSAGE_LIST_BOTTOM_THRESHOLD
  );
}

// ── Dispatch Approval Dialog ─────────────────────────────────────────────────

interface DispatchApprovalProps {
  dispatchId: string;
  agent: AgentType;
  description: string;
  taskPrompt: string;
  permissionMode: string;
  onApprove: (dispatchId: string, taskPrompt: string) => void;
  onReject: (dispatchId: string) => void;
}

function DispatchApprovalDialog({
  dispatchId,
  agent,
  description,
  taskPrompt,
  permissionMode,
  onApprove,
  onReject,
}: DispatchApprovalProps) {
  const [editedTaskPrompt, setEditedTaskPrompt] = useState(taskPrompt);
  const meta = DISPATCH_AGENT_META[agent];

  return (
    <div style={styles.approvalOverlay}>
      <div style={styles.approvalDialog}>
        <div style={styles.approvalHeader}>
          <span style={styles.approvalIcon}>📋</span>
          <span style={styles.approvalTitle}>{meta.title}</span>
          <span style={styles.approvalAgentBadge}>{meta.badge}</span>
          <span style={styles.approvalBadge}>{permissionMode}</span>
        </div>
        <div style={styles.approvalHint}>{meta.hint}</div>
        <div style={styles.approvalHint}>任务摘要：{description}</div>
        <textarea
          style={styles.approvalTextarea}
          value={editedTaskPrompt}
          onChange={(e) => setEditedTaskPrompt(e.target.value)}
          rows={14}
        />
        <div style={styles.approvalActions}>
          <button style={styles.approvalRejectBtn} onClick={() => onReject(dispatchId)}>
            拒绝
          </button>
          <button
            style={styles.approvalApproveBtn}
            onClick={() => onApprove(dispatchId, editedTaskPrompt)}
          >
            ✓ 批准运行
          </button>
        </div>
      </div>
    </div>
  );
}

function toErrorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function createEmptyUsageStats(): DispatcherMessageUsageStats {
  return {
    promptTokens: 0,
    completionTokens: 0,
    totalTokens: 0,
    elapsedMs: 0,
  };
}

function withLiveElapsed(
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

// ── Message Bubble ───────────────────────────────────────────────────────────

const BubbleCopyButton = memo(function BubbleCopyButton({
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

const UserMessageBubble = memo(function UserMessageBubble({
  message,
}: {
  message: DispatcherMessage;
}) {
  return (
    <div style={styles.messageBubbleWrap(true)}>
      <div style={styles.messageAvatar(true)}>
        <img src={userAvatarUrl} alt="用户头像" style={styles.messageAvatarImage} />
      </div>
      <div style={styles.messageBubbleColumn(true)}>
        <div style={styles.messageBubble(true)}>
          <div style={styles.markdownBody}>
            <MarkdownRenderer content={message.content} variant="chat" />
          </div>
        </div>
        <BubbleCopyButton text={message.content} isUser />
      </div>
    </div>
  );
});

const AssistantTurnBubble = memo(function AssistantTurnBubble({
  segments,
  tools,
  workspaceId,
  usageStats,
  thinking,
  placeholderText,
}: {
  segments: AssistantTurnSegment[];
  tools: ToolActivityItem[];
  workspaceId: string;
  usageStats?: DispatcherMessageUsageStats | null;
  thinking?: AssistantThinkingBlock | null;
  placeholderText?: string | null;
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

          return (
            <div key={`${segment.kind}-${segment.toolCallId ?? segment.toolName ?? index}`}>
              <div style={styles.assistantTurnSection}>
                <div style={{ ...styles.messageBubble(false), ...styles.assistantReplyBubble }}>
                  <div style={styles.markdownBody}>
                    <MarkdownRenderer content={segmentText} variant="chat" />
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

const ThinkingBlock = memo(function ThinkingBlock({
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
          <div style={styles.thinkingBody}>
            <MarkdownRenderer content={trimmedText} variant="chat" />
          </div>
        )}
      </div>
    </div>
  );
});

const AssistantUsageStats = memo(function AssistantUsageStats({
  stats,
}: {
  stats: DispatcherMessageUsageStats;
}) {
  return (
    <div style={styles.assistantUsageStats} title="来自模型标准 usage 字段">
      <span>总 {formatTokenCount(stats.totalTokens)}</span>
      <span>输入 {formatTokenCount(stats.promptTokens)}</span>
      <span>输出 {formatTokenCount(stats.completionTokens)}</span>
      <span>耗时：{formatElapsedMmSs(stats.elapsedMs)}</span>
    </div>
  );
});

const PlanModeToggleButton = memo(function PlanModeToggleButton({
  mode,
  onToggleMode,
}: {
  mode: DispatcherMode;
  onToggleMode: (mode: DispatcherMode) => void;
}) {
  const active = mode === "plan";

  return (
    <button
      type="button"
      style={styles.modeToggleBtn(active)}
      onClick={() => onToggleMode("plan")}
      title={active ? "取消 Plan 模式" : "启用 Plan 模式"}
      aria-pressed={active}
    >
      Plan
    </button>
  );
});

const ThinkingToggleButton = memo(function ThinkingToggleButton({
  active,
  disabled,
  onToggle,
}: {
  active: boolean;
  disabled?: boolean;
  onToggle: () => void;
}) {
  return (
    <button
      type="button"
      style={styles.thinkingToggleBtn(active)}
      onClick={onToggle}
      disabled={disabled}
      title={active ? "关闭深度思考" : "开启深度思考"}
      aria-label={active ? "关闭深度思考" : "开启深度思考"}
      aria-pressed={active}
    >
      <Brain size={15} />
      <span>{active ? "深思" : "思考"}</span>
    </button>
  );
});

const VoiceInputStatusCard = memo(function VoiceInputStatusCard({
  transcript,
  error,
  isRecording,
  onDismissError,
}: {
  transcript: string;
  error: string | null;
  isRecording: boolean;
  onDismissError: () => void;
}) {
  const visibleTranscript = transcript.trim();
  if (!error && !visibleTranscript && !isRecording) {
    return null;
  }

  return (
    <div style={styles.voiceStatusCard(Boolean(error))}>
      <div style={styles.voiceStatusHeader}>
        <span style={styles.voiceStatusBadge(isRecording, Boolean(error))}>
          <Mic size={12} />
          {error ? "语音识别失败" : isRecording ? "正在听写" : "听写完成"}
        </span>
        {error && (
          <button type="button" style={styles.voiceStatusDismissBtn} onClick={onDismissError}>
            收起
          </button>
        )}
      </div>
      {visibleTranscript && <div style={styles.voiceStatusText}>{visibleTranscript}</div>}
      {!visibleTranscript && !error && (
        <div style={styles.voiceStatusHint}>请开始说话，识别到完整句子后会自动发送。</div>
      )}
      {error && <div style={styles.voiceStatusError}>{error}</div>}
    </div>
  );
});

export const InteractionDrawer = memo(function InteractionDrawer({
  checklist,
  planInteraction,
  implementingPlan,
  onAnswerPlanQuestion,
  onImplementPlan,
  onImplementPlanWithClearedContext,
  onStayInPlanMode,
}: {
  checklist: ChecklistPlanState | null;
  planInteraction: PlanInteraction | null;
  implementingPlan: boolean;
  onAnswerPlanQuestion: (answer: string) => void;
  onImplementPlan: (interaction: Extract<PlanInteraction, { kind: "ready" }>) => void;
  onImplementPlanWithClearedContext: (
    interaction: Extract<PlanInteraction, { kind: "ready" }>,
  ) => void;
  onStayInPlanMode: () => void;
}) {
  const [customAnswer, setCustomAnswer] = useState("");

  if (planInteraction?.kind === "question") {
    return (
      <div style={styles.drawer}>
        <div style={styles.drawerHeader}>
          <span style={styles.drawerTitle}>
            <ClipboardList size={14} />
            问题清单
          </span>
        </div>
        <div style={styles.drawerQuestion}>{planInteraction.question}</div>
        <div style={styles.drawerOptionGrid}>
          {planInteraction.options.map((option) => (
            <button
              key={option.id}
              type="button"
              style={styles.drawerOptionBtn}
              onClick={() =>
                onAnswerPlanQuestion(`选择：${option.label}\n说明：${option.description}`)
              }
            >
              <span style={styles.drawerOptionLabel}>{option.label}</span>
              <span style={styles.drawerOptionDesc}>{option.description}</span>
            </button>
          ))}
          <div style={styles.drawerCustomBox}>
            <textarea
              style={styles.drawerCustomInput}
              value={customAnswer}
              onChange={(event) => setCustomAnswer(event.target.value)}
              placeholder="自定义输入..."
              rows={3}
            />
            <button
              type="button"
              style={styles.drawerPrimaryBtn}
              disabled={!customAnswer.trim()}
              onClick={() => {
                onAnswerPlanQuestion(`自定义回答：${customAnswer.trim()}`);
                setCustomAnswer("");
              }}
            >
              发送自定义
            </button>
          </div>
        </div>
      </div>
    );
  }

  if (planInteraction?.kind === "ready") {
    return (
      <div style={styles.drawer}>
        <div style={styles.drawerHeader}>
          <span style={styles.drawerTitle}>
            <FileText size={14} />
            计划已完成
          </span>
          <span style={styles.drawerPath}>{planInteraction.planPath}</span>
        </div>
        <div style={styles.drawerQuestion}>{planInteraction.title}</div>
        <div style={styles.drawerSummary}>{planInteraction.summary}</div>
        <div style={styles.drawerActionRow}>
          <button
            type="button"
            style={styles.drawerPrimaryBtn}
            disabled={implementingPlan}
            onClick={() => onImplementPlan(planInteraction)}
          >
            是，实施此计划
          </button>
          <button
            type="button"
            style={styles.drawerSecondaryBtn}
            disabled={implementingPlan}
            onClick={() => onImplementPlanWithClearedContext(planInteraction)}
          >
            清除上下文后实施
          </button>
          <button type="button" style={styles.drawerGhostBtn} onClick={onStayInPlanMode}>
            否，继续修改
          </button>
        </div>
      </div>
    );
  }

  if (checklist && checklist.items.length > 0) {
    return (
      <div style={styles.drawer}>
        <div style={styles.drawerHeader}>
          <span style={styles.drawerTitle}>
            <ClipboardList size={14} />
            本次任务规划步骤
          </span>
          <span style={styles.drawerPath}>
            {new Date(checklist.updatedAt).toLocaleTimeString()}
          </span>
        </div>
        {checklist.explanation && <div style={styles.drawerSummary}>{checklist.explanation}</div>}
        <div style={styles.checklistRows}>
          {checklist.items.map((item, index) => (
            <div key={item.id ?? `${item.step}-${index}`} style={styles.checklistRow}>
              <span
                style={styles.checklistStatus(item.status)}
                title={
                  item.status === "in_progress"
                    ? "正在执行"
                    : item.status === "completed"
                      ? "已完成"
                      : "等待执行"
                }
              >
                <span style={styles.checklistStatusDot(item.status)} />
              </span>
              <div style={styles.checklistContent}>
                <span style={styles.checklistText(item.status)}>{item.step}</span>
                {(item.agent || item.detail) && (
                  <span style={styles.checklistMeta}>
                    {item.agent ? getSubProcessAgentLabel(item.agent) : "子任务"}
                    {item.detail ? ` · ${item.detail}` : ""}
                  </span>
                )}
              </div>
            </div>
          ))}
        </div>
      </div>
    );
  }

  return null;
});

const EmptyConversationLauncher = memo(function EmptyConversationLauncher({
  conversationKind,
  input,
  composerMode,
  mode,
  isBusy,
  isStopping,
  isRecordingVoice,
  autoApprove,
  thinkingEnabled,
  sessionTokenUsages,
  voiceTranscript,
  voiceError,
  inputRef,
  layoutMode,
  attachedImages,
  onChangeInput,
  onPaste,
  onRemoveImage,
  onSend,
  onStop,
  onResume,
  onToggleMode,
  onToggleThinking,
  onToggleVoiceInput,
  onDismissVoiceError,
  onKeyDown,
  onOpenSettings,
  onOpenMcpStatus,
  onToggleAutoApprove,
  onCompositionStart,
  onCompositionEnd,
}: {
  conversationKind: "project" | "chat";
  input: string;
  composerMode: "send" | "stop" | "resume";
  mode: DispatcherMode;
  isBusy: boolean;
  isStopping: boolean;
  isRecordingVoice: boolean;
  autoApprove: boolean;
  thinkingEnabled: boolean;
  sessionTokenUsages: DispatcherSessionTokenUsage[];
  voiceTranscript: string;
  voiceError: string | null;
  inputRef: React.RefObject<HTMLTextAreaElement | null>;
  layoutMode: "single" | "split";
  attachedImages: ImageSegment[];
  onChangeInput: (value: string) => void;
  onPaste: (e: React.ClipboardEvent<HTMLTextAreaElement>) => void;
  onRemoveImage: (index: number) => void;
  onSend: () => void;
  onStop: () => void;
  onResume: () => void;
  onToggleMode: (mode: DispatcherMode) => void;
  onToggleThinking: () => void;
  onToggleVoiceInput: () => void;
  onDismissVoiceError: () => void;
  onKeyDown: (e: KeyboardEvent<HTMLTextAreaElement>) => void;
  onOpenSettings: () => void;
  onOpenMcpStatus: () => void;
  onToggleAutoApprove: () => void;
  onCompositionStart: () => void;
  onCompositionEnd: () => void;
}) {
  const isStopMode = composerMode === "stop";
  const isResumeMode = composerMode === "resume";
  const isPlainChat = conversationKind === "chat";

  return (
    <div style={styles.emptyLauncherWrap(layoutMode)}>
      <div style={styles.emptyComposerDialog(layoutMode)}>
        <div style={styles.emptyComposerTopBar}>
          <div style={{ display: "flex", alignItems: "center", gap: "8px" }}>
            <Sparkles size={16} color="var(--accent)" />
            <div style={styles.emptyComposerPromptHint}>
              {isPlainChat ? "普通聊天" : "主调度智能体"}
            </div>
          </div>
          <div style={styles.emptyComposerToolRow}>
            {!isPlainChat && (
              <button type="button" style={styles.emptyTopToolBtn} onClick={onOpenMcpStatus}>
                <PlugZap size={14} />
                MCP
              </button>
            )}
            <button type="button" style={styles.emptyTopToolBtn} onClick={onOpenSettings}>
              <Settings2 size={14} />
              设置
            </button>
            {!isPlainChat && (
              <button type="button" style={styles.emptyTopToolBtn} onClick={onToggleAutoApprove}>
                <Wrench size={14} />
                免确认 {autoApprove ? "开" : "关"}
              </button>
            )}
          </div>
        </div>

        <div style={styles.emptyComposerInputShell()}>
          {attachedImages.length > 0 && (
            <div style={styles.attachedImagesContainer}>
              {attachedImages.map((img, idx) => (
                <div key={idx} style={styles.attachedImageWrapper}>
                  <img src={convertFileSrc(img.path)} alt={img.alt || "pasted"} style={styles.attachedImage} />
                  <button
                    style={styles.removeImageBtn}
                    onClick={() => onRemoveImage(idx)}
                    title="移除图片"
                  >
                    <X size={12} />
                  </button>
                </div>
              ))}
            </div>
          )}
          <textarea
            ref={inputRef}
            style={styles.emptyComposerTextarea(layoutMode)}
            placeholder={
              isPlainChat
                ? "输入普通聊天消息，支持粘贴图片..."
                : "描述你的需求、粘贴代码或报错信息，支持粘贴图片..."
            }
            value={input}
            onChange={(e) => onChangeInput(e.target.value)}
            onPaste={onPaste}
            onCompositionStart={onCompositionStart}
            onCompositionEnd={onCompositionEnd}
            onKeyDown={onKeyDown}
            rows={layoutMode === "single" ? 6 : 3}
            disabled={isStopMode || isStopping}
          />
        </div>

        <VoiceInputStatusCard
          transcript={voiceTranscript}
          error={voiceError}
          isRecording={isRecordingVoice}
          onDismissError={onDismissVoiceError}
        />

        <div style={styles.emptyComposerFooter}>
          <div style={styles.emptyComposerBottomRow}>
            <div style={styles.emptyComposerFootnote}>
              <span>Enter 发送</span>
              <span style={styles.emptyComposerFootnoteDot} />
              <span>Shift + Enter 换行</span>
            </div>

            <div style={styles.emptyComposerPrimaryRow}>
              <SessionTokenUsageIndicators entries={sessionTokenUsages} />
              {!isPlainChat && <PlanModeToggleButton mode={mode} onToggleMode={onToggleMode} />}
              <ThinkingToggleButton
                active={thinkingEnabled}
                onToggle={onToggleThinking}
                disabled={composerMode === "stop" || isStopping}
              />
              <button
                type="button"
                style={styles.voiceBtn(isRecordingVoice)}
                onClick={onToggleVoiceInput}
                disabled={composerMode === "stop" || isStopping}
                title={isRecordingVoice ? "停止听写" : "开始语音输入"}
                aria-label={isRecordingVoice ? "停止语音输入" : "开始语音输入"}
              >
                <Mic size={15} />
              </button>
              <button
                type="button"
                style={{
                  ...getEmptyPrimaryComposerButtonStyle(composerMode),
                  opacity: getPrimaryComposerOpacity(
                    composerMode,
                    input,
                    isBusy,
                    isStopping,
                    attachedImages.length > 0,
                  ),
                }}
                onClick={isStopMode ? onStop : isResumeMode && !input.trim() ? onResume : onSend}
                disabled={isComposerActionDisabled(
                  composerMode,
                  input,
                  isBusy,
                  isStopping,
                  attachedImages.length > 0,
                )}
              >
                <span>
                  {getComposerButtonLabel(
                    composerMode,
                    Boolean(input.trim() || attachedImages.length > 0),
                  )}
                </span>
                {isStopMode ? (
                  <Square size={15} />
                ) : isResumeMode && !input.trim() ? (
                  <Play size={15} />
                ) : (
                  <Send size={15} />
                )}
              </button>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
});

function getComposerButtonLabel(mode: "send" | "stop" | "resume", hasInput: boolean): string {
  if (mode === "stop") return "停止";
  if (mode === "resume" && !hasInput) return "继续运行";
  return mode === "send" ? "开始对话" : "发送消息";
}

function isComposerActionDisabled(
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

function getPrimaryComposerOpacity(
  mode: "send" | "stop" | "resume",
  input: string,
  isBusy: boolean,
  isStopping: boolean,
  hasImages = false,
): number {
  return isComposerActionDisabled(mode, input, isBusy, isStopping, hasImages) ? 0.45 : 1;
}

function getPrimaryComposerButtonStyle(mode: "send" | "stop" | "resume") {
  if (mode === "stop") {
    return { ...styles.sendBtn, ...styles.stopBtn };
  }
  if (mode === "resume") {
    return { ...styles.sendBtn, ...styles.resumeBtn };
  }
  return styles.sendBtn;
}

function getEmptyPrimaryComposerButtonStyle(mode: "send" | "stop" | "resume") {
  if (mode === "stop") {
    return { ...styles.emptyComposerSendBtn, ...styles.emptyComposerStopBtn };
  }
  if (mode === "resume") {
    return { ...styles.emptyComposerSendBtn, ...styles.emptyComposerResumeBtn };
  }
  return styles.emptyComposerSendBtn;
}

function getMcpIndicatorState(
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

function getSubProcessAgentLabel(agent: AgentType): string {
  return agent === "claude" ? "Claude" : "Codex";
}

function mergeDispatcherMessages(
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

// ── DispatcherChat ───────────────────────────────────────────────────────────

interface DispatcherChatProps {
  sessionId: string;
  conversationKind?: "project" | "chat";
  projectPath?: string;
  mcpStatus?: ProjectMcpStatus | null;
  mcpChecking?: boolean;
  layoutMode?: "single" | "split";
  subProcesses?: SubProcess[];
  onDispatchApproved?: (
    dispatchId: string,
    agent: AgentType,
    description: string,
    taskPrompt: string,
    permissionMode: string,
    sessionId: string,
  ) => void;
  onDispatchRejected?: (dispatchId: string) => void;
  onDispatchContinue?: (agent: AgentType, text: string, sessionId: string) => void;
  onDispatchExit?: (agent: AgentType, reason: string, sessionId: string) => void;
  onStopActiveRun?: (sessionId: string) => Promise<void>;
  onResumeStoppedRun?: (sessionId: string) => Promise<void>;
  onOpenMcpStatus?: () => void;
  onOpenSettings: () => void;
  onOpenPlanDocument?: (path: string) => void;
  onClosePanel?: () => void;
}

interface PendingDispatchApproval {
  dispatchId: string;
  agent: AgentType;
  description: string;
  taskPrompt: string;
  permissionMode: string;
}

interface DispatcherLiveSessionState {
  hasPendingRun: boolean;
  isLoading: boolean;
  streamingSegments: AssistantTurnSegment[];
  liveThinking: AssistantThinkingBlock | null;
  liveToolCalls: ToolActivityItem[];
  assistantPlaceholder: string | null;
  runError: string | null;
  pendingDispatches: PendingDispatchApproval[];
  activeUsageStats: DispatcherMessageUsageStats | null;
  activeUsageStatsReceivedAt: number;
  usageClockNow: number;
}

function createIdleLiveSessionState(): DispatcherLiveSessionState {
  const now = Date.now();
  return {
    hasPendingRun: false,
    isLoading: false,
    streamingSegments: [],
    liveThinking: null,
    liveToolCalls: [],
    assistantPlaceholder: null,
    runError: null,
    pendingDispatches: [],
    activeUsageStats: null,
    activeUsageStatsReceivedAt: now,
    usageClockNow: now,
  };
}

const dispatcherLiveSessionStates = new Map<string, DispatcherLiveSessionState>();
const dispatcherActiveRunIds = new Map<string, number>();
const dispatcherLiveSessionSubscribers = new Map<
  string,
  Set<(state: DispatcherLiveSessionState) => void>
>();
const dispatcherMessageSubscribers = new Map<
  string,
  Set<(messages: DispatcherMessage[]) => void>
>();

function notifyDispatcherLiveSessionSubscribers(
  sessionId: string,
  state: DispatcherLiveSessionState,
) {
  dispatcherLiveSessionSubscribers.get(sessionId)?.forEach((subscriber) => subscriber(state));
}

function subscribeDispatcherLiveSession(
  sessionId: string,
  subscriber: (state: DispatcherLiveSessionState) => void,
) {
  const subscribers = dispatcherLiveSessionSubscribers.get(sessionId) ?? new Set();
  subscribers.add(subscriber);
  dispatcherLiveSessionSubscribers.set(sessionId, subscribers);
  return () => {
    subscribers.delete(subscriber);
    if (subscribers.size === 0) {
      dispatcherLiveSessionSubscribers.delete(sessionId);
    }
  };
}

function notifyDispatcherMessages(sessionId: string, messages: DispatcherMessage[]) {
  if (messages.length === 0) return;
  dispatcherMessageSubscribers.get(sessionId)?.forEach((subscriber) => subscriber(messages));
}

function subscribeDispatcherMessages(
  sessionId: string,
  subscriber: (messages: DispatcherMessage[]) => void,
) {
  const subscribers = dispatcherMessageSubscribers.get(sessionId) ?? new Set();
  subscribers.add(subscriber);
  dispatcherMessageSubscribers.set(sessionId, subscribers);
  return () => {
    subscribers.delete(subscriber);
    if (subscribers.size === 0) {
      dispatcherMessageSubscribers.delete(sessionId);
    }
  };
}

function isLiveSessionRunning(state: DispatcherLiveSessionState | undefined): boolean {
  return Boolean(state?.hasPendingRun || state?.isLoading);
}

export function getDispatcherSessionRunning(sessionId: string): boolean {
  return isLiveSessionRunning(dispatcherLiveSessionStates.get(sessionId));
}

export function subscribeDispatcherSessionRunning(
  sessionId: string,
  subscriber: (isRunning: boolean) => void,
) {
  return subscribeDispatcherLiveSession(sessionId, (state) => {
    subscriber(isLiveSessionRunning(state));
  });
}

export interface DispatcherChatHandle {
  /** Inject dispatch result and continue the agent conversation */
  continueWithResult: (
    result: string,
    dispatchState: DispatchFeedbackState,
    targetSessionId?: string,
    dispatchId?: string,
  ) => void;
  applyRuntimeState: (state: DispatcherSessionRuntimeState) => void;
}

export const DispatcherChat = forwardRef<DispatcherChatHandle, DispatcherChatProps>(
  function DispatcherChat(
    {
      sessionId,
      conversationKind = "project",
      projectPath = "",
      mcpStatus = null,
      mcpChecking = false,
      layoutMode = "split",
      subProcesses: _subProcesses = [],
      onDispatchApproved,
      onDispatchRejected,
      onDispatchContinue,
      onDispatchExit,
      onStopActiveRun,
      onResumeStoppedRun,
      onOpenMcpStatus,
      onOpenSettings,
      onOpenPlanDocument,
      onClosePanel,
    },
    ref,
  ) {
    const [messages, setMessages] = useState<DispatcherMessage[]>([]);
    const [input, setInput] = useState("");
    const [attachedImages, setAttachedImages] = useState<ImageSegment[]>([]);
    const [isLoading, setIsLoading] = useState(false);
    const [isStopping, setIsStopping] = useState(false);
    const [hasPendingRun, setHasPendingRun] = useState(false);
    const [streamingSegments, setStreamingSegments] = useState<AssistantTurnSegment[]>([]);
    const [liveThinking, setLiveThinking] = useState<AssistantThinkingBlock | null>(null);
    const [liveToolCalls, setLiveToolCalls] = useState<ToolActivityItem[]>([]);
    const [assistantPlaceholder, setAssistantPlaceholder] = useState<string | null>(null);
    const [runError, setRunError] = useState<string | null>(null);
    const [pendingDispatches, setPendingDispatches] = useState<PendingDispatchApproval[]>([]);
    const [autoApprove, setAutoApprove] = useState(false);
    const [mode, setMode] = useState<DispatcherMode>("default");
    const [checklist, setChecklist] = useState<ChecklistPlanState | null>(null);
    const [planInteraction, setPlanInteraction] = useState<PlanInteraction | null>(null);
    const [activePlanPath, setActivePlanPath] = useState<string | null>(null);
    const [implementingPlan, setImplementingPlan] = useState(false);
    const [thinkingEnabled, setThinkingEnabled] = useState(false);
    const [activeUsageStats, setActiveUsageStats] = useState<DispatcherMessageUsageStats | null>(
      null,
    );
    const [usageClockNow, setUsageClockNow] = useState(() => Date.now());
    const isPlainChat = conversationKind === "chat";

    const handlePaste = useCallback((e: React.ClipboardEvent) => {
      const items = e.clipboardData?.items;
      if (!items) return;

      for (let i = 0; i < items.length; i++) {
        if (items[i].type.indexOf("image") !== -1) {
          const blob = items[i].getAsFile();
          if (blob) {
            const reader = new FileReader();
            reader.onload = async (event) => {
              const base64 = event.target?.result as string;
              const base64Data = base64.split(",")[1];
              try {
                const result = await invoke<{ imageId: string; path: string }>("save_chat_image", {
                  sessionId,
                  sessionTitle: "",
                  imageDataBase64: base64Data,
                  mimeType: blob.type,
                });
                setAttachedImages((prev) => [
                  ...prev,
                  {
                    id: crypto.randomUUID(),
                    type: "image",
                    imageId: result.imageId,
                    path: result.path,
                    source: "user_paste",
                    mimeType: blob.type,
                  } as ImageSegment,
                ]);
              } catch (err) {
                console.error("保存图片失败:", err);
              }
            };
            reader.readAsDataURL(blob);
          }
        }
      }
    }, [sessionId]);

    const handleRemoveImage = useCallback((index: number) => {
      setAttachedImages((prev) => prev.filter((_, i) => i !== index));
    }, []);

    const messageListRef = useRef<HTMLDivElement>(null);
    const shouldStickToBottomRef = useRef(true);
    const inputRef = useRef<HTMLTextAreaElement>(null);
    const inputComposingRef = useRef(false);
    const currentSessionIdRef = useRef(sessionId);
    currentSessionIdRef.current = sessionId;
    const historyLoadRef = useRef(0);
    const activeUsageStatsReceivedAtRef = useRef(Date.now());
    const runQueuesRef = useRef<Map<string, Promise<void>>>(new Map());
    const autoApproveRef = useRef(autoApprove);
    autoApproveRef.current = autoApprove;
    const thinkingEnabledRef = useRef(thinkingEnabled);
    thinkingEnabledRef.current = thinkingEnabled;
    const onDispatchApprovedRef = useRef(onDispatchApproved);
    onDispatchApprovedRef.current = onDispatchApproved;
    const onDispatchContinueRef = useRef(onDispatchContinue);
    onDispatchContinueRef.current = onDispatchContinue;
    const onDispatchExitRef = useRef(onDispatchExit);
    onDispatchExitRef.current = onDispatchExit;
    const sessionSubProcesses = useMemo(
      () => _subProcesses.filter((subProcess) => subProcess.sessionId === sessionId),
      [_subProcesses, sessionId],
    );
    const hasRunningSubProcess =
      !isPlainChat && sessionSubProcesses.some((subProcess) => subProcess.status === "running");
    const hasStoppedSubProcess =
      !isPlainChat && sessionSubProcesses.some((subProcess) => subProcess.status === "stopped");
    const composerMode: "send" | "stop" | "resume" =
      hasPendingRun || isLoading || hasRunningSubProcess
        ? "stop"
        : hasStoppedSubProcess
          ? "resume"
          : "send";
    const isComposerBusy = isLoading || isStopping;
    const displayItems = useMemo(() => buildDispatcherDisplayItems(messages), [messages]);
    const currentPendingDispatch = pendingDispatches[0] ?? null;
    const mcpIndicator = getMcpIndicatorState(mcpStatus, mcpChecking);
    const {
      entries: sessionTokenUsageEntries,
      refresh: refreshSessionTokenUsage,
      reset: resetSessionTokenUsage,
    } = useDispatcherSessionTokenUsage(sessionId);
    const liveUsageStats = useMemo(
      () => withLiveElapsed(activeUsageStats, activeUsageStatsReceivedAtRef.current, usageClockNow),
      [activeUsageStats, usageClockNow],
    );

    const applyLiveSessionState = useCallback((state: DispatcherLiveSessionState) => {
      setHasPendingRun(state.hasPendingRun);
      setIsLoading(state.isLoading);
      setStreamingSegments(state.streamingSegments);
      setLiveThinking(state.liveThinking);
      setLiveToolCalls(state.liveToolCalls);
      setAssistantPlaceholder(state.assistantPlaceholder);
      setRunError(state.runError);
      setPendingDispatches(state.pendingDispatches);
      activeUsageStatsReceivedAtRef.current = state.activeUsageStatsReceivedAt;
      setActiveUsageStats(state.activeUsageStats);
      setUsageClockNow(state.usageClockNow);
    }, []);

    const getLiveSessionState = useCallback((targetSessionId: string) => {
      const existing = dispatcherLiveSessionStates.get(targetSessionId);
      if (existing) return existing;
      const created = createIdleLiveSessionState();
      dispatcherLiveSessionStates.set(targetSessionId, created);
      return created;
    }, []);

    const updateLiveSessionState = useCallback(
      (
        targetSessionId: string,
        updater: (state: DispatcherLiveSessionState) => DispatcherLiveSessionState,
      ) => {
        const next = updater(getLiveSessionState(targetSessionId));
        dispatcherLiveSessionStates.set(targetSessionId, next);
        notifyDispatcherLiveSessionSubscribers(targetSessionId, next);
        if (currentSessionIdRef.current === targetSessionId) {
          applyLiveSessionState(next);
        }
      },
      [applyLiveSessionState, getLiveSessionState],
    );

    useEffect(() => {
      const unlisten = listen<BrowserStatus>("browser-status", (event) => {
        const status = event.payload;
        const targetSessionId = status.sessionId;
        updateLiveSessionState(targetSessionId, (state) => ({
          ...state,
          assistantPlaceholder:
            state.liveToolCalls.some(
              (tool) => tool.status === "running" && tool.name.startsWith("browser_"),
            )
              ? status.message || "正在执行浏览器操作..."
              : state.assistantPlaceholder,
          liveToolCalls: updateLiveBrowserToolActivity(state.liveToolCalls, status),
        }));
      });

      return () => {
        unlisten.then((fn) => fn());
      };
    }, [updateLiveSessionState]);

    const scrollMessageListToBottom = useCallback((behavior: ScrollBehavior = "auto") => {
      const element = messageListRef.current;
      if (!element) return;
      element.scrollTo({ top: element.scrollHeight, behavior });
    }, []);

    const handleMessageListScroll = useCallback((event: React.UIEvent<HTMLDivElement>) => {
      shouldStickToBottomRef.current = isMessageListNearBottom(event.currentTarget);
    }, []);

    useEffect(() => {
      if (!activeUsageStats) {
        return;
      }

      const timer = window.setInterval(() => {
        setUsageClockNow(Date.now());
      }, 1000);
      return () => window.clearInterval(timer);
    }, [activeUsageStats]);

    // Load settings (for auto-approve flag)
    useEffect(() => {
      invoke<DispatcherSettings | null>("dispatcher_get_settings")
        .then((s) => {
          if (s) setAutoApprove(s.autoApproveDispatch);
        })
        .catch(console.error);
    }, []);

    // Load history for the active session only. Late responses from a previous
    // session must not overwrite the currently selected session.
    useEffect(() => {
      const loadId = ++historyLoadRef.current;
      shouldStickToBottomRef.current = true;
      setMessages([]);
      setIsStopping(false);
      applyLiveSessionState(getLiveSessionState(sessionId));
      setChecklist(null);
      setPlanInteraction(null);
      setActivePlanPath(null);
      setImplementingPlan(false);

      invoke<DispatcherMessage[]>("dispatcher_list_messages", {
        workspaceId: sessionId,
      })
        .then((loaded) => {
          if (currentSessionIdRef.current !== sessionId || historyLoadRef.current !== loadId)
            return;
          setMessages((prev) =>
            mergeDispatcherMessages(
              loaded.filter((message) => message.workspaceId === sessionId),
              prev.filter((message) => message.workspaceId === sessionId),
            ),
          );
        })
        .catch(console.error);
      invoke<DispatcherSessionRuntimeState>("dispatcher_get_session_runtime_state", {
        sessionId,
      })
        .then((state) => {
          if (currentSessionIdRef.current !== sessionId || historyLoadRef.current !== loadId)
            return;
          setMode(state.mode);
          setChecklist(state.checklist ?? null);
          setPlanInteraction(state.planInteraction ?? null);
          setActivePlanPath(state.activePlanPath ?? null);
        })
        .catch(console.error);
    }, [applyLiveSessionState, getLiveSessionState, sessionId]);

    useEffect(
      () => subscribeDispatcherLiveSession(sessionId, applyLiveSessionState),
      [applyLiveSessionState, sessionId],
    );

    useEffect(
      () =>
        subscribeDispatcherMessages(sessionId, (incoming) => {
          setMessages((prev) =>
            mergeDispatcherMessages(
              prev,
              incoming.filter((message) => message.workspaceId === sessionId),
            ),
          );
        }),
      [sessionId],
    );

    useEffect(() => {
      if (!shouldStickToBottomRef.current) return;
      scrollMessageListToBottom();
    }, [
      messages,
      streamingSegments,
      liveThinking,
      assistantPlaceholder,
      liveToolCalls,
      runError,
      scrollMessageListToBottom,
    ]);

    const createEventChannel = useCallback(
      (targetSessionId: string, runId: number) => {
        const onEvent = new Channel<DispatcherAgentEvent>();
        onEvent.onmessage = (event) => {
          const isActiveRun = dispatcherActiveRunIds.get(targetSessionId) === runId;
          const isCurrentSession = currentSessionIdRef.current === targetSessionId;
          switch (event.event) {
            case "started":
              break;
            case "assistantStarted":
              if (!isActiveRun) return;
              updateLiveSessionState(targetSessionId, (state) => ({
                ...state,
                assistantPlaceholder: "正在分析问题...",
                liveThinking: null,
                streamingSegments: state.streamingSegments.filter(
                  (segment) => segment.kind === "tool-summary",
                ),
              }));
              break;
            case "modelSwitched":
              if (!isActiveRun) return;
              updateLiveSessionState(targetSessionId, (state) => ({
                ...state,
                assistantPlaceholder: `已检测到图片，自动切换到视觉模型 ${event.data.toModel}。`,
                streamingSegments: appendAssistantTextSegment(
                  state.streamingSegments,
                  `> ${event.data.reason}，已从 ${event.data.fromModel} 自动切换到视觉模型 ${event.data.toModel}。\n\n`,
                ),
              }));
              break;
            case "userMessage":
              if (!isActiveRun || event.data.message.workspaceId !== targetSessionId) return;
              notifyDispatcherMessages(targetSessionId, [event.data.message]);
              break;
            case "assistantDelta":
              if (!isActiveRun) return;
              updateLiveSessionState(targetSessionId, (state) => ({
                ...state,
                assistantPlaceholder: null,
                streamingSegments: appendAssistantTextSegment(
                  state.streamingSegments,
                  event.data.delta,
                ),
              }));
              break;
            case "assistantThinkingDelta":
              if (!isActiveRun) return;
              updateLiveSessionState(targetSessionId, (state) => ({
                ...state,
                assistantPlaceholder: null,
                liveThinking: {
                  text: `${state.liveThinking?.text ?? ""}${event.data.delta}`,
                  elapsedMs: event.data.elapsedMs,
                },
              }));
              break;
            case "assistantMessage":
              if (!isActiveRun || event.data.message.workspaceId !== targetSessionId) return;
              updateLiveSessionState(targetSessionId, (state) => ({
                ...state,
                assistantPlaceholder: null,
                liveThinking: null,
                streamingSegments: state.streamingSegments.filter(
                  (segment) => segment.kind === "tool-summary",
                ),
              }));
              notifyDispatcherMessages(targetSessionId, [event.data.message]);
              break;
            case "runUsageUpdated":
              if (!isActiveRun || event.data.workspaceId !== targetSessionId) return;
              {
                const now = Date.now();
                updateLiveSessionState(targetSessionId, (state) => ({
                  ...state,
                  activeUsageStats: event.data.stats,
                  activeUsageStatsReceivedAt: now,
                  usageClockNow: now,
                }));
              }
              void refreshSessionTokenUsage(targetSessionId);
              break;
            case "toolPlanned":
              if (!isActiveRun) return;
              updateLiveSessionState(targetSessionId, (state) => ({
                ...state,
                assistantPlaceholder: "正在规划工具调用...",
                liveToolCalls: planLiveToolActivity(state.liveToolCalls, event.data),
              }));
              break;
            case "toolStarted":
              if (!isActiveRun) return;
              updateLiveSessionState(targetSessionId, (state) => ({
                ...state,
                assistantPlaceholder: "正在执行工具...",
                liveToolCalls: startLiveToolActivity(state.liveToolCalls, event.data),
              }));
              break;
            case "toolSummaryStarted":
              if (!isActiveRun) return;
              break;
            case "toolSummaryDelta":
              if (!isActiveRun) return;
              updateLiveSessionState(targetSessionId, (state) => ({
                ...state,
                streamingSegments: appendToolSummarySegment(
                  state.streamingSegments,
                  event.data,
                ),
              }));
              break;
            case "toolFinished":
              if (!isActiveRun) return;
              updateLiveSessionState(targetSessionId, (state) => ({
                ...state,
                liveToolCalls: finishLiveToolActivity(state.liveToolCalls, event.data),
              }));
              break;
            case "checklistPlanUpdated":
              if (!isActiveRun || !isCurrentSession) return;
              setChecklist(event.data.state);
              break;
            case "planQuestionRequested":
              if (!isActiveRun || !isCurrentSession) return;
              setPlanInteraction(event.data.interaction);
              break;
            case "planDocumentOpened":
              if (!isActiveRun || !isCurrentSession) return;
              setActivePlanPath(event.data.planPath);
              onOpenPlanDocument?.(event.data.planPath);
              break;
            case "planReady":
              if (!isActiveRun || !isCurrentSession) return;
              setPlanInteraction(event.data.interaction);
              if (event.data.interaction.kind === "ready") {
                setActivePlanPath(event.data.interaction.planPath);
                onOpenPlanDocument?.(event.data.interaction.planPath);
              }
              break;
            case "planImplemented":
              if (!isActiveRun || !isCurrentSession) return;
              setActivePlanPath(event.data.implementedPath);
              setPlanInteraction(null);
              onOpenPlanDocument?.(event.data.implementedPath);
              break;
            case "dispatchProposed": {
              const { dispatchId, agent, description, taskPrompt, permissionMode } = event.data;
              if (isPlainChat) {
                return;
              }
              if (autoApproveRef.current && onDispatchApprovedRef.current) {
                onDispatchApprovedRef.current(
                  dispatchId,
                  agent,
                  description,
                  taskPrompt,
                  permissionMode,
                  targetSessionId,
                );
              } else if (isActiveRun) {
                updateLiveSessionState(targetSessionId, (state) => ({
                  ...state,
                  pendingDispatches: [
                    ...state.pendingDispatches,
                    { dispatchId, agent, description, taskPrompt, permissionMode },
                  ],
                }));
              }
              break;
            }
            case "dispatchContinue": {
              onDispatchContinueRef.current?.(event.data.agent, event.data.text, targetSessionId);
              break;
            }
            case "dispatchExit": {
              onDispatchExitRef.current?.(event.data.agent, event.data.reason, targetSessionId);
              break;
            }
            case "finished":
              if (!isActiveRun) return;
              notifyDispatcherMessages(
                targetSessionId,
                event.data.messages.filter((message) => message.workspaceId === targetSessionId),
              );
              void refreshSessionTokenUsage(targetSessionId);
              dispatcherActiveRunIds.delete(targetSessionId);
              updateLiveSessionState(targetSessionId, () => createIdleLiveSessionState());
              break;
          }
        };
        return onEvent;
      },
      [isPlainChat, onOpenPlanDocument, refreshSessionTokenUsage, updateLiveSessionState],
    );

    const enqueueDispatcherRun = useCallback(
      async (
        targetSessionId: string,
        runner: (onEvent: Channel<DispatcherAgentEvent>) => Promise<void>,
      ) => {
        const previous = runQueuesRef.current.get(targetSessionId) ?? Promise.resolve();
        const queued = previous
          .catch(() => undefined)
          .then(async () => {
            const runId = (dispatcherActiveRunIds.get(targetSessionId) ?? 0) + 1;
            dispatcherActiveRunIds.set(targetSessionId, runId);
            const now = Date.now();
            updateLiveSessionState(targetSessionId, () => ({
              ...createIdleLiveSessionState(),
              hasPendingRun: true,
              isLoading: true,
              activeUsageStats: createEmptyUsageStats(),
              activeUsageStatsReceivedAt: now,
              usageClockNow: now,
            }));

            const onEvent = createEventChannel(targetSessionId, runId);

            try {
              await runner(onEvent);
            } finally {
              if (dispatcherActiveRunIds.get(targetSessionId) === runId) {
                updateLiveSessionState(targetSessionId, (state) => ({
                  ...state,
                  hasPendingRun: false,
                  isLoading: false,
                  activeUsageStats: null,
                }));
              }
            }
          });

        runQueuesRef.current.set(targetSessionId, queued);

        try {
          await queued;
        } finally {
          if (runQueuesRef.current.get(targetSessionId) === queued) {
            runQueuesRef.current.delete(targetSessionId);
          }
        }
      },
      [createEventChannel, updateLiveSessionState],
    );

    const sendUserMessage = useCallback(
      async (
        rawText: string,
        images: ImageSegment[] = [],
        targetSessionId = sessionId,
        targetMode: DispatcherMode = mode,
      ) => {
        const text = rawText.trim();
        if (!text && images.length === 0) return;

        setInput("");
        setAttachedImages([]);
        updateLiveSessionState(targetSessionId, (state) => ({
          ...state,
          pendingDispatches: [],
        }));
        if (currentSessionIdRef.current === targetSessionId) {
          shouldStickToBottomRef.current = true;
          window.requestAnimationFrame(() => scrollMessageListToBottom());
        }

        // Build segments JSON for the backend
        const segments: Array<{ type: string; [key: string]: unknown }> = [];
        for (const img of images) {
          segments.push({ ...img });
        }
        if (text) {
          segments.push({
            id: crypto.randomUUID(),
            type: "text",
            text,
          });
        }
        const segmentsJson = JSON.stringify(segments);

        try {
          await enqueueDispatcherRun(targetSessionId, async (onEvent) => {
            if (isPlainChat) {
              await invoke<DispatcherAgentTurn>("dispatcher_send_plain_chat_message", {
                workspaceId: targetSessionId,
                content: text,
                segmentsJson,
                enableThinking: thinkingEnabledRef.current,
                onEvent,
              });
            } else {
              await invoke<DispatcherAgentTurn>("dispatcher_send_message", {
                workspaceId: targetSessionId,
                projectPath,
                content: text,
                segmentsJson,
                mode: targetMode,
                enableThinking: thinkingEnabledRef.current,
                onEvent,
              });
            }
          });
        } catch (err) {
          console.error("发送消息失败:", err);
          updateLiveSessionState(targetSessionId, (state) => ({
            ...state,
            runError: `${isPlainChat ? "聊天" : "调度智能体"}执行失败：${toErrorMessage(err)}`,
          }));
        }
      },
      [
        enqueueDispatcherRun,
        isPlainChat,
        mode,
        projectPath,
        scrollMessageListToBottom,
        sessionId,
        updateLiveSessionState,
      ],
    );

    const voiceInput = useDashScopeAsr({
      workspaceId: sessionId,
      enabled: composerMode !== "stop" && !isStopping,
      onTranscriptReady: async (text) => {
        await sendUserMessage(text, [], sessionId);
      },
    });
    const {
      isRecording: isRecordingVoice,
      transcript: voiceTranscript,
      error: voiceError,
      stopRecording: stopVoiceRecording,
      toggleRecording: toggleVoiceRecording,
      clearError: clearVoiceError,
    } = voiceInput;

    const handleSend = useCallback(async () => {
      const text = input.trim();
      if ((!text && attachedImages.length === 0) || isLoading || isStopping) return;

      try {
        await sendUserMessage(text, attachedImages, sessionId);
      } finally {
        if (isRecordingVoice) {
          await stopVoiceRecording();
        }
      }
    }, [
      input,
      attachedImages,
      isLoading,
      isRecordingVoice,
      isStopping,
      sendUserMessage,
      sessionId,
      stopVoiceRecording,
    ]);

    const handleStop = useCallback(async () => {
      if (isStopping) return;
      setIsStopping(true);
      try {
        if (isRecordingVoice) {
          await stopVoiceRecording();
        }
        await Promise.all([
          invoke("dispatcher_stop_run", { workspaceId: sessionId }).catch(console.error),
          onStopActiveRun?.(sessionId) ?? Promise.resolve(),
        ]);
      } finally {
        setIsStopping(false);
      }
    }, [isRecordingVoice, isStopping, onStopActiveRun, sessionId, stopVoiceRecording]);

    const handleResume = useCallback(async () => {
      await onResumeStoppedRun?.(sessionId);
    }, [onResumeStoppedRun, sessionId]);

    // Expose continueWithResult to parent via ref
    useImperativeHandle(
      ref,
      () => ({
        continueWithResult: async (
          result: string,
          dispatchState: DispatchFeedbackState,
          targetSessionId = sessionId,
          dispatchId?: string,
        ) => {
          updateLiveSessionState(targetSessionId, (state) => ({
            ...state,
            pendingDispatches: [],
          }));

          try {
            if (isPlainChat) {
              return;
            }
            await enqueueDispatcherRun(targetSessionId, async (onEvent) => {
              await invoke<DispatcherAgentTurn>("dispatcher_continue_after_dispatch", {
                workspaceId: targetSessionId,
                projectPath,
                dispatchResult: result,
                dispatchState,
                dispatchId,
                enableThinking: thinkingEnabledRef.current,
                onEvent,
              });
            });
          } catch (err) {
            console.error("dispatcher_continue_after_dispatch 失败:", err);
            updateLiveSessionState(targetSessionId, (state) => ({
              ...state,
              runError: `调度智能体继续执行失败：${toErrorMessage(err)}`,
            }));
          }
        },
        applyRuntimeState: (state: DispatcherSessionRuntimeState) => {
          setMode(state.mode);
          setChecklist(state.checklist ?? null);
          setPlanInteraction(state.planInteraction ?? null);
          setActivePlanPath(state.activePlanPath ?? null);
        },
      }),
      [enqueueDispatcherRun, isPlainChat, projectPath, sessionId, updateLiveSessionState],
    );

    const handleKeyDown = useCallback(
      (e: React.KeyboardEvent) => {
        if (composerMode === "stop") {
          return;
        }
        if (inputComposingRef.current || isImeComposing(e)) {
          return;
        }
        if (e.key === "Enter" && !e.shiftKey) {
          e.preventDefault();
          if (composerMode === "resume" && !input.trim()) {
            handleResume();
            return;
          }
          handleSend();
        }
      },
      [composerMode, handleResume, handleSend, input],
    );

    const handleApproveDispatch = useCallback(
      (dispatchId: string, taskPrompt: string) => {
        const agent = currentPendingDispatch?.agent ?? "claude";
        const pm = currentPendingDispatch?.permissionMode ?? "full_access";
        const description = currentPendingDispatch?.description ?? "未命名子任务";
        updateLiveSessionState(sessionId, (state) => ({
          ...state,
          pendingDispatches: state.pendingDispatches.slice(1),
        }));
        onDispatchApproved?.(dispatchId, agent, description, taskPrompt, pm, sessionId);
      },
      [currentPendingDispatch, onDispatchApproved, sessionId, updateLiveSessionState],
    );

    const handleRejectDispatch = useCallback(
      (dispatchId: string) => {
        updateLiveSessionState(sessionId, (state) => ({
          ...state,
          pendingDispatches: state.pendingDispatches.slice(1),
        }));
        invoke<DispatcherSessionRuntimeState>("dispatcher_clear_checklist_dispatch", {
          sessionId,
          dispatchId,
        })
          .then((state) => {
            setChecklist(state.checklist ?? null);
          })
          .catch(console.error);
        onDispatchRejected?.(dispatchId);
      },
      [onDispatchRejected, sessionId, updateLiveSessionState],
    );

    const handleToggleAutoApprove = useCallback(async () => {
      const next = !autoApprove;
      setAutoApprove(next);
      try {
        const saved = await invoke<DispatcherSettings>("dispatcher_set_auto_approve_dispatch", {
          autoApproveDispatch: next,
        });
        setAutoApprove(saved.autoApproveDispatch);
      } catch (err) {
        setAutoApprove(!next);
        console.error("dispatcher_set_auto_approve_dispatch 失败:", err);
      }
    }, [autoApprove]);

    const handleModeChange = useCallback(
      async (nextMode: DispatcherMode) => {
        if (nextMode === mode) return;
        const previousMode = mode;
        setMode(nextMode);
        try {
          const state = await invoke<DispatcherSessionRuntimeState>("dispatcher_set_session_mode", {
            sessionId,
            mode: nextMode,
          });
          setMode(state.mode);
          setChecklist(state.checklist ?? null);
          setPlanInteraction(state.planInteraction ?? null);
          setActivePlanPath(state.activePlanPath ?? null);
        } catch (err) {
          setMode(previousMode);
          updateLiveSessionState(sessionId, (state) => ({
            ...state,
            runError: `切换模式失败：${toErrorMessage(err)}`,
          }));
        }
      },
      [mode, sessionId, updateLiveSessionState],
    );

    const handleModeToggle = useCallback(
      (nextMode: DispatcherMode) => {
        const resolvedMode = mode === nextMode ? "default" : nextMode;
        handleModeChange(resolvedMode).catch(console.error);
      },
      [handleModeChange, mode],
    );

    const handleToggleThinking = useCallback(() => {
      setThinkingEnabled((value) => !value);
    }, []);

    const handleAnswerPlanQuestion = useCallback(
      async (answer: string) => {
        if (planInteraction?.kind !== "question") return;
        const content = buildPlanQuestionAnswer(planInteraction, answer);
        setPlanInteraction(null);
        await sendUserMessage(content, [], sessionId, "plan");
      },
      [planInteraction, sendUserMessage, sessionId],
    );

    const handleImplementPlan = useCallback(
      async (interaction: Extract<PlanInteraction, { kind: "ready" }>) => {
        setImplementingPlan(true);
        try {
          const content = buildPlanImplementationPrompt(interaction.planPath);
          await handleModeChange("default");
          setPlanInteraction(null);
          await sendUserMessage(content, [], sessionId, "default");
        } catch (err) {
          updateLiveSessionState(sessionId, (state) => ({
            ...state,
            runError: `实施计划失败：${toErrorMessage(err)}`,
          }));
        } finally {
          setImplementingPlan(false);
        }
      },
      [handleModeChange, sendUserMessage, sessionId, updateLiveSessionState],
    );

    const handleImplementPlanWithClearedContext = useCallback(
      async (interaction: Extract<PlanInteraction, { kind: "ready" }>) => {
        setImplementingPlan(true);
        try {
          const content = buildPlanImplementationPrompt(interaction.planPath);
          await handleModeChange("default");
          await invoke("dispatcher_clear_message_context", { workspaceId: sessionId });
          setPlanInteraction(null);
          setChecklist(null);
          await sendUserMessage(content, [], sessionId, "default");
        } catch (err) {
          updateLiveSessionState(sessionId, (state) => ({
            ...state,
            runError: `清除上下文后实施失败：${toErrorMessage(err)}`,
          }));
        } finally {
          setImplementingPlan(false);
        }
      },
      [handleModeChange, sendUserMessage, sessionId, updateLiveSessionState],
    );

    const handleStayInPlanMode = useCallback(() => {
      setPlanInteraction(null);
      setMode("plan");
      inputRef.current?.focus();
    }, []);

    const handleClearHistory = useCallback(async () => {
      try {
        await invoke("dispatcher_clear_messages", {
          workspaceId: sessionId,
        });
        setMessages([]);
        setChecklist(null);
        setPlanInteraction(null);
        setActivePlanPath(null);
        resetSessionTokenUsage();
      } catch (err) {
        console.error("清空消息失败:", err);
      }
    }, [resetSessionTokenUsage, sessionId]);

    const hasLiveSegments = streamingSegments.some((segment) => segment.text.trim());
    const hasAssistantPlaceholder = Boolean(assistantPlaceholder?.trim());
    const isEmpty =
      messages.length === 0 &&
      !hasLiveSegments &&
      !liveThinking &&
      liveToolCalls.length === 0 &&
      !hasAssistantPlaceholder;

    return (
      <div style={styles.container}>
        {/* Header */}
        <div style={styles.header}>
          <div style={styles.headerLeft}>
            <span style={styles.headerIcon}>{isPlainChat ? "💬" : "🤖"}</span>
            <span style={styles.headerTitle}>{isPlainChat ? "聊天" : "调度智能体"}</span>
            {!isPlainChat && activePlanPath && <span style={styles.headerPlanBadge}>Plan</span>}
            {thinkingEnabled && <span style={styles.headerThinkingBadge}>Think</span>}
            {isLoading && <span style={styles.thinkingDot} />}
          </div>
          <div style={styles.headerRight}>
            {!isPlainChat && (
              <>
                <button
                  style={{
                    ...styles.headerBtn,
                    ...(autoApprove ? styles.headerBtnActive : {}),
                  }}
                  onClick={handleToggleAutoApprove}
                  title="开启后，调度给 Claude 或 Codex 子任务时不再弹出审查确认"
                >
                  免确认 {autoApprove ? "开" : "关"}
                </button>
                <button
                  style={styles.headerBtn}
                  onClick={onOpenMcpStatus}
                  title={`项目级 MCP 状态：${mcpIndicator.label}`}
                >
                  <span
                    style={{
                      ...styles.headerSignal,
                      background: mcpIndicator.color,
                      boxShadow: `0 0 0 3px ${mcpIndicator.color}22`,
                    }}
                  />
                  MCP
                </button>
              </>
            )}
            {messages.length > 0 && (
              <button style={styles.headerBtn} onClick={handleClearHistory}>
                清空
              </button>
            )}
            <button style={styles.headerBtn} onClick={onOpenSettings}>
              ⚙ 设置
            </button>
            {onClosePanel && (
              <button
                style={styles.headerBtn}
                onClick={onClosePanel}
                title="关闭会话面板"
                aria-label="关闭会话面板"
              >
                <X size={14} />
              </button>
            )}
          </div>
        </div>

        {/* Messages */}
        <div
          ref={messageListRef}
          className="dispatcher-message-list"
          style={styles.messageList}
          onScroll={handleMessageListScroll}
        >
          {runError && <div style={styles.runErrorBanner}>{runError}</div>}
          {isEmpty && !isPlainChat && (
            <InteractionDrawer
              checklist={checklist}
              planInteraction={planInteraction}
              implementingPlan={implementingPlan}
              onAnswerPlanQuestion={handleAnswerPlanQuestion}
              onImplementPlan={handleImplementPlan}
              onImplementPlanWithClearedContext={handleImplementPlanWithClearedContext}
              onStayInPlanMode={handleStayInPlanMode}
            />
          )}
          {isEmpty && (
            <EmptyConversationLauncher
              conversationKind={conversationKind}
              input={input}
              composerMode={composerMode}
              mode={mode}
              isBusy={isComposerBusy}
              isStopping={isStopping}
              isRecordingVoice={isRecordingVoice}
              autoApprove={autoApprove}
              thinkingEnabled={thinkingEnabled}
              sessionTokenUsages={sessionTokenUsageEntries}
              voiceTranscript={voiceTranscript}
              voiceError={voiceError}
              inputRef={inputRef}
              layoutMode={layoutMode}
              attachedImages={attachedImages}
              onChangeInput={setInput}
              onPaste={handlePaste}
              onRemoveImage={handleRemoveImage}
              onSend={handleSend}
              onStop={handleStop}
              onResume={handleResume}
              onToggleMode={handleModeToggle}
              onToggleThinking={handleToggleThinking}
              onToggleVoiceInput={toggleVoiceRecording}
              onDismissVoiceError={clearVoiceError}
              onKeyDown={handleKeyDown}
              onOpenSettings={onOpenSettings}
              onOpenMcpStatus={() => onOpenMcpStatus?.()}
              onToggleAutoApprove={handleToggleAutoApprove}
              onCompositionStart={() => {
                inputComposingRef.current = true;
              }}
              onCompositionEnd={() => {
                inputComposingRef.current = false;
              }}
            />
          )}
          {displayItems.map((item) =>
            item.kind === "user" ? (
              <UserMessageBubble key={item.id} message={item.message} />
            ) : (
              <AssistantTurnBubble
                key={item.id}
                segments={item.turn.segments}
                tools={item.turn.tools}
                workspaceId={sessionId}
                usageStats={item.turn.usageStats}
                thinking={item.turn.thinking}
              />
            ),
          )}
          {(hasLiveSegments ||
            liveThinking ||
            liveToolCalls.length > 0 ||
            hasAssistantPlaceholder ||
            liveUsageStats) && (
            <AssistantTurnBubble
              segments={streamingSegments}
              tools={liveToolCalls}
              workspaceId={sessionId}
              usageStats={liveUsageStats}
              thinking={liveThinking}
              placeholderText={assistantPlaceholder}
            />
          )}
        </div>

        {/* Input */}
        {!isEmpty && (
          <>
            {!isPlainChat && (
              <InteractionDrawer
                checklist={checklist}
                planInteraction={planInteraction}
                implementingPlan={implementingPlan}
                onAnswerPlanQuestion={handleAnswerPlanQuestion}
                onImplementPlan={handleImplementPlan}
                onImplementPlanWithClearedContext={handleImplementPlanWithClearedContext}
                onStayInPlanMode={handleStayInPlanMode}
              />
            )}
            <VoiceInputStatusCard
              transcript={voiceTranscript}
              error={voiceError}
              isRecording={isRecordingVoice}
              onDismissError={clearVoiceError}
            />
            <div style={styles.inputArea}>
              {attachedImages.length > 0 && (
                <div style={styles.attachedImagesContainer}>
                  {attachedImages.map((img, idx) => (
                    <div key={idx} style={styles.attachedImageWrapper}>
                      <img src={convertFileSrc(img.path)} alt={img.alt || "pasted"} style={styles.attachedImage} />
                      <button
                        style={styles.removeImageBtn}
                        onClick={() => handleRemoveImage(idx)}
                        title="移除图片"
                      >
                        <X size={12} />
                      </button>
                    </div>
                  ))}
                </div>
              )}
              <SessionTokenUsageIndicators entries={sessionTokenUsageEntries} />
              <textarea
                ref={inputRef}
                style={styles.inputTextarea}
                value={input}
                onChange={(e) => setInput(e.target.value)}
                onPaste={handlePaste}
                onCompositionStart={() => {
                  inputComposingRef.current = true;
                }}
                onCompositionEnd={() => {
                  inputComposingRef.current = false;
                }}
                onKeyDown={handleKeyDown}
                rows={1}
                disabled={composerMode === "stop" || isStopping}
                placeholder={isPlainChat ? "发送普通聊天消息..." : "给调度智能体发送消息..."}
              />
              {!isPlainChat && <PlanModeToggleButton mode={mode} onToggleMode={handleModeToggle} />}
              <ThinkingToggleButton
                active={thinkingEnabled}
                onToggle={handleToggleThinking}
                disabled={composerMode === "stop" || isStopping}
              />
              <button
                style={styles.voiceBtn(isRecordingVoice)}
                onClick={toggleVoiceRecording}
                disabled={composerMode === "stop" || isStopping}
                title={isRecordingVoice ? "停止听写" : "开始语音输入"}
                aria-label={isRecordingVoice ? "停止语音输入" : "开始语音输入"}
              >
                <Mic size={15} />
              </button>
              <button
                style={{
                  ...getPrimaryComposerButtonStyle(composerMode),
                  opacity: getPrimaryComposerOpacity(
                    composerMode,
                    input,
                    isComposerBusy,
                    isStopping,
                    attachedImages.length > 0,
                  ),
                }}
                title={getComposerButtonLabel(
                  composerMode,
                  Boolean(input.trim() || attachedImages.length > 0),
                )}
                onClick={
                  composerMode === "stop"
                    ? handleStop
                    : composerMode === "resume" && !input.trim()
                      ? handleResume
                      : handleSend
                }
                disabled={isComposerActionDisabled(
                  composerMode,
                  input,
                  isComposerBusy,
                  isStopping,
                  attachedImages.length > 0,
                )}
              >
                {composerMode === "stop" ? (
                  <Square size={16} color="#fff" />
                ) : composerMode === "resume" && !input.trim() ? (
                  <Play size={16} color="#fff" />
                ) : (
                  <Send size={16} color="#fff" />
                )}
              </button>
            </div>
          </>
        )}

        {/* Dispatch approval overlay */}
        {!isPlainChat && currentPendingDispatch && (
          <DispatchApprovalDialog
            dispatchId={currentPendingDispatch.dispatchId}
            agent={currentPendingDispatch.agent}
            description={currentPendingDispatch.description}
            taskPrompt={currentPendingDispatch.taskPrompt}
            permissionMode={currentPendingDispatch.permissionMode}
            onApprove={handleApproveDispatch}
            onReject={handleRejectDispatch}
          />
        )}
      </div>
    );
  },
);

const DISPATCH_AGENT_META: Record<AgentType, { title: string; badge: string; hint: string }> = {
  claude: {
    title: "Claude 子任务审查",
    badge: "Claude",
    hint: "Claude 更快，适合新功能、算法试验和问题探索。",
  },
  codex: {
    title: "Codex 子任务审查",
    badge: "Codex",
    hint: "Codex 更慢但更仔细，适合重构、结构整理和高风险修改。",
  },
};

// ── Styles ───────────────────────────────────────────────────────────────────

const styles = {
  container: {
    display: "flex",
    flexDirection: "column" as const,
    flex: 1,
    width: "100%",
    height: "100%",
    minWidth: 0,
    minHeight: 0,
    background:
      "radial-gradient(circle at top right, color-mix(in srgb, var(--accent) 10%, transparent), transparent 26%), linear-gradient(180deg, var(--bg-panel), color-mix(in srgb, var(--bg-panel) 78%, var(--bg-subtle)))",
  },
  header: {
    display: "flex",
    alignItems: "center",
    justifyContent: "space-between",
    padding: "12px 18px",
    borderBottom: "1px solid var(--border-dim)",
    background: "color-mix(in srgb, var(--bg-card) 68%, transparent)",
    backdropFilter: "blur(16px)",
    WebkitBackdropFilter: "blur(16px)",
    WebkitAppRegion: "drag" as const,
    flexShrink: 0,
  },
  headerLeft: {
    display: "flex",
    alignItems: "center",
    gap: "8px",
  },
  headerIcon: { fontSize: "16px" },
  headerTitle: {
    fontSize: "13.5px",
    fontWeight: 700,
    letterSpacing: "-0.01em",
    color: "var(--text-primary)",
  },
  headerPlanBadge: {
    border: "1px solid color-mix(in srgb, var(--accent) 22%, var(--border-dim))",
    borderRadius: 999,
    padding: "3px 7px",
    color: "var(--accent)",
    background: "var(--accent-subtle)",
    fontSize: 10,
    fontWeight: 800,
  },
  headerThinkingBadge: {
    border: "1px solid color-mix(in srgb, var(--warning, #d97706) 28%, var(--border-dim))",
    borderRadius: 999,
    padding: "3px 7px",
    color: "var(--warning, #d97706)",
    background: "color-mix(in srgb, var(--warning, #d97706) 10%, transparent)",
    fontSize: 10,
    fontWeight: 800,
  },
  thinkingDot: {
    width: "8px",
    height: "8px",
    borderRadius: "50%",
    background: "var(--accent)",
    animation: "pulse 1.5s ease-in-out infinite",
  },
  headerRight: {
    display: "flex",
    alignItems: "center",
    gap: "4px",
    WebkitAppRegion: "no-drag" as const,
  },
  modeToggleBtn: (active: boolean) => ({
    height: "34px",
    padding: "0 11px",
    borderRadius: "12px",
    border: active
      ? "1px solid color-mix(in srgb, var(--accent) 48%, transparent)"
      : "1px solid var(--border-dim)",
    background: active
      ? "color-mix(in srgb, var(--accent) 14%, var(--bg-card))"
      : "color-mix(in srgb, var(--bg-card) 88%, transparent)",
    color: active ? "var(--accent)" : "var(--text-secondary)",
    fontSize: 12,
    fontWeight: 700,
    cursor: "pointer",
    boxShadow: active ? "0 12px 24px -18px var(--accent)" : "var(--shadow-xs)",
  }),
  thinkingToggleBtn: (active: boolean) => ({
    height: "34px",
    padding: "0 11px",
    borderRadius: "12px",
    border: active
      ? "1px solid color-mix(in srgb, var(--warning, #d97706) 52%, transparent)"
      : "1px solid var(--border-dim)",
    background: active
      ? "color-mix(in srgb, var(--warning, #d97706) 14%, var(--bg-card))"
      : "color-mix(in srgb, var(--bg-card) 88%, transparent)",
    color: active ? "var(--warning, #d97706)" : "var(--text-secondary)",
    display: "inline-flex",
    alignItems: "center",
    justifyContent: "center",
    gap: 6,
    fontSize: 12,
    fontWeight: 700,
    cursor: "pointer",
    flexShrink: 0,
    boxShadow: active ? "0 12px 24px -18px var(--warning, #d97706)" : "var(--shadow-xs)",
  }),
  headerBtn: {
    padding: "6px 10px",
    fontSize: "11px",
    display: "inline-flex",
    alignItems: "center",
    gap: "6px",
    background: "color-mix(in srgb, var(--bg-card) 82%, transparent)",
    border: "1px solid var(--border-dim)",
    borderRadius: "999px",
    color: "var(--text-secondary)",
    cursor: "pointer",
  },
  headerSignal: {
    width: "8px",
    height: "8px",
    borderRadius: "999px",
    flexShrink: 0,
  },
  headerBtnActive: {
    background: "var(--accent-subtle)",
    borderColor: "var(--accent)",
    color: "var(--accent)",
  },
  messageList: {
    flex: 1,
    minHeight: 0,
    overflowY: "auto" as const,
    overflowAnchor: "none" as const,
    padding: "22px 20px 18px",
    display: "flex",
    flexDirection: "column" as const,
    gap: "18px",
  },
  runErrorBanner: {
    border: "1px solid color-mix(in srgb, var(--danger) 36%, var(--border-primary))",
    background: "color-mix(in srgb, var(--danger) 10%, var(--bg-panel))",
    color: "var(--danger)",
    borderRadius: "8px",
    padding: "10px 12px",
    fontSize: "12px",
    lineHeight: 1.5,
    whiteSpace: "pre-wrap" as const,
  },
  drawer: {
    border: "1px solid color-mix(in srgb, var(--accent) 18%, var(--border-dim))",
    background: "color-mix(in srgb, var(--bg-card) 94%, transparent)",
    borderRadius: 14,
    padding: "12px 14px",
    display: "flex",
    flexDirection: "column" as const,
    gap: 10,
    boxShadow: "0 14px 36px rgba(15, 23, 42, 0.08)",
  },
  drawerHeader: {
    display: "flex",
    alignItems: "center",
    justifyContent: "space-between",
    gap: 10,
    minWidth: 0,
  },
  drawerTitle: {
    display: "inline-flex",
    alignItems: "center",
    gap: 7,
    color: "var(--text-primary)",
    fontSize: 12,
    fontWeight: 800,
  },
  drawerPath: {
    minWidth: 0,
    overflow: "hidden",
    textOverflow: "ellipsis",
    whiteSpace: "nowrap" as const,
    color: "var(--text-hint)",
    fontSize: 11,
    fontFamily: "var(--font-mono)",
  },
  drawerQuestion: {
    color: "var(--text-primary)",
    fontSize: 13,
    fontWeight: 700,
    lineHeight: 1.5,
  },
  drawerSummary: {
    color: "var(--text-secondary)",
    fontSize: 12,
    lineHeight: 1.6,
    whiteSpace: "pre-wrap" as const,
  },
  drawerOptionGrid: {
    display: "grid",
    gridTemplateColumns: "repeat(auto-fit, minmax(160px, 1fr))",
    gap: 8,
  },
  drawerOptionBtn: {
    textAlign: "left" as const,
    border: "1px solid var(--border-dim)",
    background: "color-mix(in srgb, var(--bg-panel) 84%, transparent)",
    borderRadius: 10,
    padding: "10px 11px",
    display: "flex",
    flexDirection: "column" as const,
    gap: 5,
    cursor: "pointer",
  },
  drawerOptionLabel: {
    color: "var(--text-primary)",
    fontSize: 12,
    fontWeight: 800,
  },
  drawerOptionDesc: {
    color: "var(--text-secondary)",
    fontSize: 11.5,
    lineHeight: 1.45,
  },
  drawerCustomBox: {
    border: "1px solid var(--border-dim)",
    borderRadius: 10,
    padding: 8,
    display: "flex",
    flexDirection: "column" as const,
    gap: 8,
    background: "color-mix(in srgb, var(--bg-panel) 84%, transparent)",
  },
  drawerCustomInput: {
    border: "none",
    outline: "none",
    resize: "vertical" as const,
    minHeight: 64,
    background: "transparent",
    color: "var(--text-primary)",
    fontSize: 12.5,
    lineHeight: 1.5,
  },
  drawerActionRow: {
    display: "flex",
    alignItems: "center",
    flexWrap: "wrap" as const,
    gap: 8,
  },
  drawerPrimaryBtn: {
    border: "none",
    borderRadius: 9,
    padding: "8px 11px",
    background: "var(--accent)",
    color: "#fff",
    fontSize: 12,
    fontWeight: 800,
    cursor: "pointer",
  },
  drawerSecondaryBtn: {
    border: "1px solid var(--border-dim)",
    borderRadius: 9,
    padding: "8px 11px",
    background: "color-mix(in srgb, var(--bg-card) 88%, transparent)",
    color: "var(--text-primary)",
    fontSize: 12,
    fontWeight: 700,
    cursor: "pointer",
  },
  drawerGhostBtn: {
    border: "none",
    borderRadius: 9,
    padding: "8px 11px",
    background: "transparent",
    color: "var(--text-secondary)",
    fontSize: 12,
    fontWeight: 700,
    cursor: "pointer",
  },
  checklistRows: {
    display: "flex",
    flexDirection: "column" as const,
    gap: 7,
  },
  checklistRow: {
    display: "flex",
    alignItems: "center",
    gap: 8,
    minWidth: 0,
  },
  checklistStatus: (_status: "pending" | "in_progress" | "completed") => ({
    width: 18,
    height: 18,
    borderRadius: 999,
    display: "inline-flex",
    alignItems: "center",
    justifyContent: "center",
    flexShrink: 0,
    position: "relative" as const,
  }),
  checklistStatusDot: (status: "pending" | "in_progress" | "completed") => ({
    width: status === "in_progress" ? 16 : 9,
    height: status === "in_progress" ? 16 : 9,
    borderRadius: 999,
    display: "inline-block",
    background:
      status === "completed"
        ? "var(--success)"
        : status === "pending"
          ? "color-mix(in srgb, var(--text-muted) 55%, transparent)"
          : "transparent",
    border:
      status === "in_progress"
        ? "2px solid color-mix(in srgb, var(--accent) 24%, transparent)"
        : "none",
    borderTopColor: status === "in_progress" ? "var(--accent)" : undefined,
    boxShadow:
      status === "completed"
        ? "0 0 0 4px color-mix(in srgb, var(--success) 13%, transparent)"
        : status === "pending"
          ? "0 0 0 4px color-mix(in srgb, var(--text-muted) 9%, transparent)"
          : "none",
    animation: status === "in_progress" ? "spin 0.85s linear infinite" : undefined,
  }),
  checklistContent: {
    minWidth: 0,
    display: "flex",
    flexDirection: "column" as const,
    gap: 2,
  },
  checklistText: (status: "pending" | "in_progress" | "completed") => ({
    minWidth: 0,
    color: status === "completed" ? "var(--text-muted)" : "var(--text-primary)",
    textDecoration: status === "completed" ? "line-through" : "none",
    fontSize: 12.5,
    lineHeight: 1.45,
  }),
  checklistMeta: {
    minWidth: 0,
    color: "var(--text-muted)",
    fontSize: 11,
    lineHeight: 1.35,
    overflow: "hidden",
    textOverflow: "ellipsis",
    whiteSpace: "nowrap" as const,
  },
  emptyLauncherWrap: (layoutMode: "single" | "split") => ({
    display: "flex",
    flexDirection: "column" as const,
    alignItems: "center",
    justifyContent: "center",
    flex: 1,
    width: "100%",
    minHeight: "100%",
    gap: layoutMode === "single" ? "22px" : "0",
    padding: layoutMode === "single" ? "44px 24px 56px" : "28px 12px 44px",
    boxSizing: "border-box" as const,
  }),
  emptyLauncherHero: (layoutMode: "single" | "split") => ({
    width: "100%",
    maxWidth: layoutMode === "single" ? "920px" : "780px",
    display: "flex",
    flexDirection: "column" as const,
    alignItems: "center",
    gap: "14px",
    marginBottom: layoutMode === "single" ? "4px" : "26px",
    textAlign: "center" as const,
  }),
  emptyLauncherKicker: {
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
    gap: "10px",
    flexWrap: "wrap" as const,
  },
  emptyLauncherBadge: {
    display: "inline-flex",
    alignItems: "center",
    gap: "7px",
    padding: "7px 12px",
    borderRadius: "999px",
    border: "1px solid color-mix(in srgb, var(--accent) 16%, var(--border-dim))",
    background: "color-mix(in srgb, var(--bg-card) 76%, transparent)",
    color: "var(--text-secondary)",
    fontSize: "12px",
    fontWeight: 600,
    boxShadow: "0 12px 30px rgba(15, 23, 42, 0.04)",
  },
  emptyLauncherMeta: {
    fontSize: "12.5px",
    color: "var(--text-muted)",
    letterSpacing: "0.01em",
  },
  emptyLauncherTitle: {
    margin: 0,
    fontSize: "34px",
    lineHeight: 1.04,
    letterSpacing: "-0.04em",
    fontWeight: 700,
    color: "var(--text-primary)",
  },
  emptyLauncherSubtitle: {
    margin: 0,
    fontSize: "15px",
    color: "var(--text-secondary)",
    maxWidth: "700px",
    lineHeight: "1.72",
  },
  emptyComposerDialog: (layoutMode: "single" | "split") => ({
    width: "100%",
    maxWidth: layoutMode === "single" ? "920px" : "860px",
    display: "flex",
    flexDirection: "column" as const,
    gap: "18px",
    justifyContent: "flex-start",
    padding: layoutMode === "single" ? "20px 24px 22px" : "18px",
    borderRadius: "30px",
    border: "1px solid color-mix(in srgb, var(--accent) 10%, var(--border-dim))",
    background:
      "linear-gradient(180deg, color-mix(in srgb, var(--bg-card) 94%, transparent), color-mix(in srgb, var(--bg-subtle) 82%, transparent))",
    boxShadow:
      layoutMode === "single"
        ? "0 28px 72px rgba(15, 23, 42, 0.10)"
        : "0 36px 100px rgba(15, 23, 42, 0.09)",
    backdropFilter: "blur(22px)",
    WebkitBackdropFilter: "blur(22px)",
    boxSizing: "border-box" as const,
  }),
  emptyComposerTopBar: {
    display: "flex",
    alignItems: "center",
    justifyContent: "space-between",
    gap: "12px",
    flexWrap: "wrap" as const,
  },
  emptyComposerPromptHint: {
    fontSize: "12px",
    fontWeight: 700,
    letterSpacing: "0.06em",
    textTransform: "uppercase" as const,
    color: "var(--text-hint)",
  },
  emptyComposerToolRow: {
    display: "flex",
    alignItems: "center",
    gap: "8px",
    flexWrap: "wrap" as const,
  },
  emptyTopToolBtn: {
    display: "inline-flex",
    alignItems: "center",
    gap: "6px",
    height: "34px",
    padding: "0 12px",
    borderRadius: "999px",
    border: "1px solid var(--border-dim)",
    background: "color-mix(in srgb, var(--bg-card) 74%, transparent)",
    color: "var(--text-secondary)",
    fontSize: "12px",
    fontWeight: 600,
    cursor: "pointer",
  },
  emptyComposerInputShell: () => ({
    borderRadius: "24px",
    border: "1px solid color-mix(in srgb, var(--accent) 8%, var(--border-dim))",
    background:
      "linear-gradient(180deg, color-mix(in srgb, var(--bg-card) 96%, transparent), color-mix(in srgb, var(--bg-subtle) 72%, transparent))",
    boxShadow: "inset 0 1px 0 rgba(255,255,255,0.2)",
    display: "flex",
    flexDirection: "column" as const,
  }),
  attachedImagesContainer: {
    display: "flex",
    flexWrap: "wrap" as const,
    gap: "10px",
    padding: "16px 22px 0",
  },
  attachedImageWrapper: {
    position: "relative" as const,
    width: "64px",
    height: "64px",
    borderRadius: "10px",
    overflow: "hidden",
    border: "1px solid var(--border-medium)",
  },
  attachedImage: {
    width: "100%",
    height: "100%",
    objectFit: "cover" as const,
  },
  removeImageBtn: {
    position: "absolute" as const,
    top: "4px",
    right: "4px",
    width: "20px",
    height: "20px",
    borderRadius: "50%",
    background: "rgba(0,0,0,0.6)",
    color: "#fff",
    border: "none",
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
    cursor: "pointer",
  },
  emptyComposerTextarea: (layoutMode: "single" | "split") => ({
    width: "100%",
    minHeight: layoutMode === "single" ? "120px" : "80px",
    padding: layoutMode === "single" ? "18px 22px" : "16px 20px",
    border: "none",
    outline: "none",
    resize: "none" as const,
    background: "transparent",
    color: "var(--text-primary)",
    fontSize: layoutMode === "single" ? "17px" : "19px",
    lineHeight: "1.7",
    fontFamily: "var(--font-ui)",
    boxSizing: "border-box" as const,
    flex: 1,
  }),
  emptyComposerActionRow: {
    display: "flex",
    flexWrap: "wrap" as const,
    gap: "10px",
  },
  emptyComposerActionBtn: {
    display: "inline-flex",
    alignItems: "center",
    gap: "8px",
    minHeight: "38px",
    padding: "8px 13px",
    borderRadius: "999px",
    border: "1px solid color-mix(in srgb, var(--accent) 8%, var(--border-dim))",
    background: "color-mix(in srgb, var(--bg-card) 68%, transparent)",
    color: "var(--text-secondary)",
    fontSize: "12.5px",
    fontWeight: 600,
    cursor: "pointer",
  },
  emptyComposerActionIcon: {
    display: "inline-flex",
    alignItems: "center",
    justifyContent: "center",
    width: "18px",
    height: "18px",
    color: "var(--text-primary)",
  },
  emptyComposerActionLabel: {
    whiteSpace: "nowrap" as const,
  },
  emptyComposerFooter: {
    display: "flex",
    flexDirection: "column" as const,
    gap: "14px",
    paddingTop: "2px",
  },
  emptyComposerFootnote: {
    display: "flex",
    alignItems: "center",
    gap: "10px",
    flexWrap: "wrap" as const,
    color: "var(--text-muted)",
    fontSize: "12px",
    lineHeight: 1.6,
  },
  emptyComposerFootnoteDot: {
    width: "4px",
    height: "4px",
    borderRadius: "999px",
    background: "var(--text-hint)",
    flexShrink: 0,
  },
  emptyComposerBottomRow: {
    display: "flex",
    alignItems: "center",
    justifyContent: "space-between",
    gap: "12px",
    flexWrap: "wrap" as const,
  },
  emptyComposerSecondaryRow: {
    display: "flex",
    alignItems: "center",
    gap: "10px",
    flexWrap: "wrap" as const,
  },
  emptyComposerPrimaryRow: {
    display: "flex",
    alignItems: "center",
    justifyContent: "flex-end",
    gap: "10px",
    flexWrap: "wrap" as const,
  },
  emptySecondaryBtn: {
    display: "inline-flex",
    alignItems: "center",
    justifyContent: "center",
    padding: "8px 12px",
    borderRadius: "999px",
    border: "1px solid var(--border-dim)",
    background: "transparent",
    color: "var(--text-secondary)",
    fontSize: "12px",
    fontWeight: 600,
    cursor: "pointer",
  },
  emptyComposerSendBtn: {
    display: "inline-flex",
    alignItems: "center",
    gap: "8px",
    height: "44px",
    padding: "0 18px",
    borderRadius: "999px",
    border: "none",
    background:
      "linear-gradient(135deg, var(--accent), color-mix(in srgb, var(--accent) 74%, white))",
    color: "#fff",
    fontSize: "13px",
    fontWeight: 700,
    cursor: "pointer",
    boxShadow: "0 18px 28px -16px color-mix(in srgb, var(--accent) 60%, transparent)",
  },
  emptyComposerStopBtn: {
    background: "linear-gradient(135deg, #0f172a, #334155)",
    boxShadow: "0 18px 28px -16px rgba(15, 23, 42, 0.55)",
  },
  emptyComposerResumeBtn: {
    background: "linear-gradient(135deg, #0f766e, #14b8a6)",
    boxShadow: "0 18px 28px -16px rgba(13, 148, 136, 0.5)",
  },
  messageBubbleWrap: (isUser: boolean) => ({
    width: "100%",
    minWidth: 0,
    display: "flex",
    flexDirection: isUser ? ("row-reverse" as const) : ("row" as const),
    gap: "14px",
    alignItems: "flex-start",
    marginBottom: "2px",
  }),
  messageAvatar: (isUser: boolean) => ({
    width: "34px",
    height: "34px",
    borderRadius: "12px",
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
    background: isUser
      ? "linear-gradient(135deg, var(--accent), color-mix(in srgb, var(--accent) 72%, white))"
      : "linear-gradient(135deg, color-mix(in srgb, var(--bg-card) 92%, transparent), color-mix(in srgb, var(--bg-subtle) 84%, transparent))",
    border: "1px solid var(--border-dim)",
    boxShadow: isUser
      ? "0 8px 18px -8px color-mix(in srgb, var(--accent) 48%, transparent)"
      : "0 8px 18px -10px rgba(0,0,0,0.12)",
    flexShrink: 0,
    marginTop: "2px",
    overflow: "hidden",
  }),
  messageAvatarImage: {
    width: "100%",
    height: "100%",
    display: "block",
    objectFit: "cover" as const,
  },
  messageBubble: (isUser: boolean) => ({
    width: isUser ? "fit-content" : "100%",
    maxWidth: "100%",
    minWidth: 0,
    padding: "14px 16px",
    borderRadius: isUser ? "22px 22px 8px 22px" : "22px 22px 22px 8px",
    background: isUser
      ? "linear-gradient(135deg, var(--accent), color-mix(in srgb, var(--accent) 82%, white))"
      : "linear-gradient(180deg, color-mix(in srgb, var(--bg-card) 96%, transparent), color-mix(in srgb, var(--bg-subtle) 82%, transparent))",
    color: isUser ? "#fff" : "var(--text-primary)",
    border: isUser ? "none" : "1px solid color-mix(in srgb, var(--accent) 10%, var(--border-dim))",
    boxShadow: isUser
      ? "0 16px 28px -18px color-mix(in srgb, var(--accent) 50%, transparent)"
      : "0 16px 32px rgba(15, 23, 42, 0.05)",
    fontSize: "14px",
    lineHeight: "1.7",
    overflowWrap: "break-word" as const,
    wordBreak: "normal" as const,
    userSelect: "text" as const,
    WebkitUserSelect: "text" as const,
  }),
  messageBubbleColumn: (isUser: boolean) => ({
    width: "min(760px, calc(100% - 96px))",
    maxWidth: "min(760px, calc(100% - 96px))",
    minWidth: 0,
    display: "flex",
    flexDirection: "column" as const,
    alignItems: isUser ? ("flex-end" as const) : ("flex-start" as const),
    gap: "5px",
  }),
  bubbleCopyButton: (isUser: boolean, status: "idle" | "copied" | "failed") => ({
    display: "inline-flex",
    alignItems: "center",
    justifyContent: "center",
    gap: "4px",
    width: "fit-content",
    minHeight: "24px",
    padding: "3px 7px",
    borderRadius: "7px",
    border: "1px solid var(--border-dim)",
    background:
      status === "copied"
        ? "color-mix(in srgb, var(--success, #16a34a) 14%, var(--bg-card))"
        : status === "failed"
          ? "color-mix(in srgb, var(--danger, #dc2626) 12%, var(--bg-card))"
          : "color-mix(in srgb, var(--bg-card) 90%, transparent)",
    color:
      status === "copied"
        ? "var(--success, #16a34a)"
        : status === "failed"
          ? "var(--danger, #dc2626)"
          : isUser
            ? "color-mix(in srgb, var(--accent) 78%, var(--text-secondary))"
            : "var(--text-hint)",
    fontSize: "11px",
    fontWeight: 600,
    lineHeight: 1.2,
    cursor: "pointer",
    userSelect: "none" as const,
    WebkitUserSelect: "none" as const,
    opacity: status === "idle" ? 0.78 : 1,
  }),
  messageText: {
    whiteSpace: "pre-wrap" as const,
    userSelect: "text" as const,
    WebkitUserSelect: "text" as const,
    fontFamily: "var(--font-ui)",
  },
  markdownBody: {
    fontSize: "14px",
    lineHeight: "1.72",
    userSelect: "text" as const,
    WebkitUserSelect: "text" as const,
    fontFamily: "var(--font-ui)",
  },
  assistantTurnStack: {
    width: "min(760px, calc(100% - 96px))",
    maxWidth: "min(760px, calc(100% - 96px))",
    display: "flex",
    flexDirection: "column" as const,
    gap: "12px",
    minWidth: 0,
  },
  assistantTurnSection: {
    width: "100%",
    minWidth: 0,
  },
  assistantReplyBubble: {
    background:
      "linear-gradient(180deg, color-mix(in srgb, var(--bg-card) 96%, transparent), color-mix(in srgb, var(--bg-subtle) 82%, transparent))",
  },
  assistantUsageStats: {
    display: "flex",
    alignItems: "center",
    gap: "8px",
    flexWrap: "wrap" as const,
    width: "fit-content",
    maxWidth: "100%",
    padding: "4px 8px",
    borderRadius: "999px",
    border: "1px solid var(--border-dim)",
    background: "color-mix(in srgb, var(--bg-card) 82%, transparent)",
    color: "var(--text-hint)",
    fontFamily: "var(--font-mono)",
    fontSize: "10.5px",
    lineHeight: 1.4,
  },
  assistantPlaceholder: {
    display: "inline-flex",
    alignItems: "center",
    gap: "8px",
    color: "var(--text-secondary)",
    fontSize: "13px",
    lineHeight: 1.6,
  },
  assistantPlaceholderDot: {
    width: "8px",
    height: "8px",
    borderRadius: "50%",
    background: "var(--accent)",
    boxShadow: "0 0 0 4px color-mix(in srgb, var(--accent) 16%, transparent)",
    flexShrink: 0,
  },
  toolSummaryCard: {
    width: "100%",
    padding: "14px 16px",
    borderRadius: "22px 22px 22px 8px",
    border: "1px solid color-mix(in srgb, var(--warning, #d97706) 22%, var(--border-dim))",
    background:
      "linear-gradient(180deg, rgba(217,119,6,0.08), color-mix(in srgb, var(--bg-card) 94%, transparent))",
    boxShadow: "0 16px 32px rgba(15, 23, 42, 0.05)",
  },
  thinkingCard: {
    width: "100%",
    borderRadius: "18px 18px 18px 8px",
    border: "1px solid color-mix(in srgb, var(--warning, #d97706) 20%, var(--border-dim))",
    background:
      "linear-gradient(180deg, color-mix(in srgb, var(--warning, #d97706) 8%, var(--bg-card)), color-mix(in srgb, var(--bg-card) 92%, transparent))",
    overflow: "hidden",
    boxShadow: "0 14px 28px rgba(15, 23, 42, 0.04)",
  },
  thinkingHeader: {
    width: "100%",
    border: "none",
    background: "transparent",
    color: "var(--text-secondary)",
    display: "flex",
    alignItems: "center",
    gap: 8,
    padding: "9px 12px",
    cursor: "pointer",
    fontSize: 12,
    fontWeight: 700,
    textAlign: "left" as const,
  },
  thinkingBadge: {
    display: "inline-flex",
    alignItems: "center",
    gap: 6,
    padding: "3px 8px",
    borderRadius: 999,
    background: "color-mix(in srgb, var(--warning, #d97706) 14%, transparent)",
    color: "var(--warning, #d97706)",
    flexShrink: 0,
  },
  thinkingMeta: {
    flex: 1,
    minWidth: 0,
    color: "var(--text-muted)",
    fontFamily: "var(--font-mono)",
    fontSize: 11,
  },
  thinkingBody: {
    borderTop: "1px solid color-mix(in srgb, var(--warning, #d97706) 14%, var(--border-dim))",
    padding: "11px 13px 13px",
    color: "var(--text-secondary)",
    fontSize: 13,
    lineHeight: 1.65,
    userSelect: "text" as const,
    WebkitUserSelect: "text" as const,
  },
  toolSummaryHeader: {
    display: "flex",
    alignItems: "center",
    gap: "8px",
    flexWrap: "wrap" as const,
    marginBottom: "10px",
  },
  toolSummaryBadge: {
    display: "inline-flex",
    alignItems: "center",
    padding: "4px 9px",
    borderRadius: "999px",
    background: "rgba(217,119,6,0.12)",
    color: "var(--warning, #d97706)",
    fontSize: "11px",
    fontWeight: 700,
    letterSpacing: "0.02em",
  },
  toolSummaryName: {
    fontSize: "12px",
    fontWeight: 600,
    color: "var(--text-primary)",
  },
  toolSummaryMode: {
    fontSize: "11px",
    color: "var(--text-secondary)",
  },
  inputArea: {
    display: "flex",
    alignItems: "flex-end",
    gap: "12px",
    padding: "14px 18px 18px",
    background:
      "linear-gradient(180deg, color-mix(in srgb, var(--bg-panel) 0%, transparent), color-mix(in srgb, var(--bg-card) 40%, transparent))",
    flexShrink: 0,
    flexWrap: "wrap" as const,
  },
  inputTextarea: {
    flex: 1,
    padding: "14px 16px",
    fontSize: "14px",
    lineHeight: "1.6",
    background: "color-mix(in srgb, var(--bg-card) 92%, transparent)",
    border: "1px solid color-mix(in srgb, var(--accent) 10%, var(--border-medium))",
    borderRadius: "18px",
    color: "var(--text-primary)",
    resize: "none" as const,
    outline: "none",
    fontFamily: "var(--font-ui)",
    boxShadow: "0 12px 30px rgba(15, 23, 42, 0.05)",
    transition: "border-color 0.2s, box-shadow 0.2s",
  },
  sendBtn: {
    width: "44px",
    height: "44px",
    borderRadius: "16px",
    background:
      "linear-gradient(135deg, var(--accent), color-mix(in srgb, var(--accent) 74%, white))",
    color: "#fff",
    border: "none",
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
    cursor: "pointer",
    transition: "opacity 0.2s, transform 0.1s",
    boxShadow: "0 18px 28px -16px color-mix(in srgb, var(--accent) 60%, transparent)",
  },
  voiceBtn: (active: boolean) => ({
    width: "44px",
    height: "44px",
    borderRadius: "14px",
    border: active
      ? "1px solid color-mix(in srgb, var(--danger) 38%, transparent)"
      : "1px solid var(--border-dim)",
    background: active
      ? "color-mix(in srgb, var(--danger) 14%, var(--bg-card))"
      : "color-mix(in srgb, var(--bg-card) 88%, transparent)",
    color: active ? "var(--danger)" : "var(--text-secondary)",
    display: "inline-flex",
    alignItems: "center",
    justifyContent: "center",
    cursor: "pointer",
    flexShrink: 0,
    boxShadow: active ? "0 12px 24px -18px var(--danger)" : "var(--shadow-xs)",
  }),
  voiceStatusCard: (isError: boolean) => ({
    margin: "0 18px 8px",
    padding: "10px 12px",
    borderRadius: "12px",
    border: isError
      ? "1px solid color-mix(in srgb, var(--danger) 30%, var(--border-dim))"
      : "1px solid color-mix(in srgb, var(--accent) 18%, var(--border-dim))",
    background: isError
      ? "color-mix(in srgb, var(--danger) 8%, var(--bg-card))"
      : "color-mix(in srgb, var(--accent) 7%, var(--bg-card))",
  }),
  voiceStatusHeader: {
    display: "flex",
    alignItems: "center",
    justifyContent: "space-between",
    gap: "10px",
    marginBottom: "6px",
  },
  voiceStatusBadge: (active: boolean, isError: boolean) => ({
    display: "inline-flex",
    alignItems: "center",
    gap: "6px",
    fontSize: "12px",
    fontWeight: 700,
    color: isError ? "var(--danger)" : active ? "var(--accent)" : "var(--text-secondary)",
  }),
  voiceStatusDismissBtn: {
    border: "none",
    background: "transparent",
    color: "var(--text-secondary)",
    fontSize: "12px",
    cursor: "pointer",
  },
  voiceStatusText: {
    fontSize: "13px",
    lineHeight: 1.6,
    color: "var(--text-primary)",
    whiteSpace: "pre-wrap" as const,
  },
  voiceStatusHint: {
    fontSize: "12px",
    color: "var(--text-hint)",
  },
  voiceStatusError: {
    fontSize: "12px",
    lineHeight: 1.5,
    color: "var(--danger)",
  },
  stopBtn: {
    background: "linear-gradient(135deg, #0f172a, #334155)",
    boxShadow: "0 18px 28px -16px rgba(15, 23, 42, 0.55)",
  },
  resumeBtn: {
    background: "linear-gradient(135deg, #0f766e, #14b8a6)",
    boxShadow: "0 18px 28px -16px rgba(13, 148, 136, 0.5)",
  },
  // ── Approval dialog styles ──
  approvalOverlay: {
    position: "absolute" as const,
    inset: 0,
    background: "rgba(0,0,0,0.4)",
    backdropFilter: "blur(4px)",
    WebkitBackdropFilter: "blur(4px)",
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
    zIndex: 100,
  },
  approvalDialog: {
    width: "min(560px, calc(100% - 16px))",
    background: "var(--bg-card)",
    border: "1px solid var(--border-medium)",
    borderRadius: "16px",
    padding: "20px",
    display: "flex",
    flexDirection: "column" as const,
    gap: "16px",
    boxShadow: "0 10px 40px rgba(0,0,0,0.2)",
  },
  approvalHeader: {
    display: "flex",
    alignItems: "center",
    gap: "10px",
  },
  approvalIcon: { fontSize: "18px", color: "var(--accent)" },
  approvalTitle: {
    fontSize: "14.5px",
    fontWeight: 600,
    color: "var(--text-primary)",
    flex: 1,
  },
  approvalAgentBadge: {
    fontSize: "11px",
    padding: "3px 8px",
    borderRadius: "999px",
    background: "color-mix(in srgb, var(--accent) 12%, transparent)",
    color: "var(--accent)",
    border: "1px solid color-mix(in srgb, var(--accent) 22%, transparent)",
    fontWeight: 700,
  },
  approvalBadge: {
    fontSize: "11px",
    padding: "3px 8px",
    borderRadius: "6px",
    background: "var(--bg-subtle)",
    color: "var(--text-secondary)",
    fontFamily: "var(--font-mono)",
    fontWeight: 500,
  },
  approvalHint: {
    fontSize: "12px",
    lineHeight: "1.6",
    color: "var(--text-secondary)",
  },
  approvalTextarea: {
    width: "100%",
    padding: "12px",
    fontSize: "13px",
    lineHeight: "1.6",
    background: "var(--bg-input)",
    border: "1px solid var(--border-medium)",
    borderRadius: "10px",
    color: "var(--text-primary)",
    resize: "vertical" as const,
    outline: "none",
    fontFamily: "var(--font-mono)",
    boxSizing: "border-box" as const,
    boxShadow: "0 2px 6px rgba(0,0,0,0.02) inset",
  },
  approvalActions: {
    display: "flex",
    justifyContent: "flex-end",
    gap: "10px",
    marginTop: "4px",
  },
  approvalRejectBtn: {
    padding: "8px 18px",
    fontSize: "12.5px",
    background: "var(--bg-subtle)",
    border: "1px solid var(--border-medium)",
    borderRadius: "8px",
    color: "var(--text-secondary)",
    cursor: "pointer",
    fontWeight: 500,
  },
  approvalApproveBtn: {
    padding: "8px 18px",
    fontSize: "12.5px",
    background: "var(--accent)",
    color: "#fff",
    border: "none",
    borderRadius: "8px",
    cursor: "pointer",
    fontWeight: 600,
    boxShadow: "0 2px 6px color-mix(in srgb, var(--accent) 40%, transparent)",
  },
};
