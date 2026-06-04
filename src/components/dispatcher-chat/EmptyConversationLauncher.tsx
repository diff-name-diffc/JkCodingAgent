import { memo } from "react";
import type { KeyboardEvent } from "react";
import { Sparkles, Send, Square, Play, X, Settings2, PlugZap, Wrench, Mic } from "lucide-react";
import { convertFileSrc } from "@tauri-apps/api/core";
import type {
  DispatcherMode,
  DispatcherSessionTokenUsage,
  ImageSegment,
} from "../../types";
import { SessionTokenUsageIndicators } from "../SessionTokenUsageIndicators";
import { PlanModeToggleButton, ThinkingToggleButton } from "./ComposerButtons";
import { VoiceInputStatusCard } from "./VoiceInputStatusCard";
import {
  getComposerButtonLabel,
  isComposerActionDisabled,
  getPrimaryComposerOpacity,
} from "./dispatcherChatUtils";
import { dispatcherChatStyles as styles } from "./dispatcherChatStyles";

function getEmptyPrimaryComposerButtonStyle(mode: "send" | "stop" | "resume") {
  if (mode === "stop") {
    return { ...styles.emptyComposerSendBtn, ...styles.emptyComposerStopBtn };
  }
  if (mode === "resume") {
    return { ...styles.emptyComposerSendBtn, ...styles.emptyComposerResumeBtn };
  }
  return styles.emptyComposerSendBtn;
}

export const EmptyConversationLauncher = memo(function EmptyConversationLauncher({
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
  onDrop,
  onDragOver,
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
  onDrop: (e: React.DragEvent) => void;
  onDragOver: (e: React.DragEvent) => void;
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

        <div
          style={styles.emptyComposerInputShell()}
          onDrop={onDrop}
          onDragOver={onDragOver}
        >
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
