import { memo, type RefObject } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { X, Mic, Play, Send, Square } from "lucide-react";
import type {
  ChecklistPlanState,
  DispatcherMode,
  DispatcherSessionTokenUsage,
  ImageSegment,
  PlanInteraction,
} from "../../types";
import { SessionTokenUsageIndicators } from "../SessionTokenUsageIndicators";
import { PlanModeToggleButton, ThinkingToggleButton } from "./ComposerButtons";
import { VoiceInputStatusCard } from "./VoiceInputStatusCard";
import { InteractionDrawer } from "./InteractionDrawer";
import {
  getComposerButtonLabel,
  isComposerActionDisabled,
  getPrimaryComposerOpacity,
} from "./dispatcherChatUtils";
import { dispatcherChatStyles as styles } from "./dispatcherChatStyles";

function getPrimaryComposerButtonStyle(mode: "send" | "stop" | "resume") {
  if (mode === "stop") return { ...styles.sendBtn, ...styles.stopBtn };
  if (mode === "resume") return { ...styles.sendBtn, ...styles.resumeBtn };
  return styles.sendBtn;
}

interface ComposerInputProps {
  isPlainChat: boolean;
  input: string;
  attachedImages: ImageSegment[];
  composerMode: "send" | "stop" | "resume";
  mode: DispatcherMode;
  thinkingEnabled: boolean;
  isComposerBusy: boolean;
  isStopping: boolean;
  isRecordingVoice: boolean;
  voiceTranscript: string;
  voiceError: string | null;
  inputRef: RefObject<HTMLTextAreaElement | null>;
  checklist: ChecklistPlanState | null;
  planInteraction: PlanInteraction | null;
  implementingPlan: boolean;
  sessionTokenUsageEntries: DispatcherSessionTokenUsage[];
  onChangeInput: (value: string) => void;
  onPaste: (e: React.ClipboardEvent) => void;
  onDrop: (e: React.DragEvent) => void;
  onDragOver: (e: React.DragEvent) => void;
  onRemoveImage: (index: number) => void;
  onSend: () => void;
  onStop: () => void;
  onResume: () => void;
  onKeyDown: (e: React.KeyboardEvent) => void;
  onToggleMode: (mode: DispatcherMode) => void;
  onToggleThinking: () => void;
  onToggleVoiceInput: () => void;
  onDismissVoiceError: () => void;
  onCompositionStart: () => void;
  onCompositionEnd: () => void;
  onAnswerPlanQuestion: (answer: string) => void;
  onImplementPlan: (interaction: Extract<PlanInteraction, { kind: "ready" }>) => void;
  onImplementPlanWithClearedContext: (interaction: Extract<PlanInteraction, { kind: "ready" }>) => void;
  onStayInPlanMode: () => void;
}

export const ComposerInput = memo(function ComposerInput({
  isPlainChat,
  input,
  attachedImages,
  composerMode,
  mode,
  thinkingEnabled,
  isComposerBusy,
  isStopping,
  isRecordingVoice,
  voiceTranscript,
  voiceError,
  inputRef,
  checklist,
  planInteraction,
  implementingPlan,
  sessionTokenUsageEntries,
  onChangeInput,
  onPaste,
  onDrop,
  onDragOver,
  onRemoveImage,
  onSend,
  onStop,
  onResume,
  onKeyDown,
  onToggleMode,
  onToggleThinking,
  onToggleVoiceInput,
  onDismissVoiceError,
  onCompositionStart,
  onCompositionEnd,
  onAnswerPlanQuestion,
  onImplementPlan,
  onImplementPlanWithClearedContext,
  onStayInPlanMode,
}: ComposerInputProps) {
  return (
    <>
      {!isPlainChat && (
        <InteractionDrawer
          checklist={checklist}
          planInteraction={planInteraction}
          implementingPlan={implementingPlan}
          onAnswerPlanQuestion={onAnswerPlanQuestion}
          onImplementPlan={onImplementPlan}
          onImplementPlanWithClearedContext={onImplementPlanWithClearedContext}
          onStayInPlanMode={onStayInPlanMode}
        />
      )}
      <VoiceInputStatusCard
        transcript={voiceTranscript}
        error={voiceError}
        isRecording={isRecordingVoice}
        onDismissError={onDismissVoiceError}
      />
      <div
        style={styles.inputArea}
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
        <SessionTokenUsageIndicators entries={sessionTokenUsageEntries} />
        <textarea
          ref={inputRef}
          style={styles.inputTextarea}
          value={input}
          onChange={(e) => onChangeInput(e.target.value)}
          onPaste={onPaste}
          onCompositionStart={onCompositionStart}
          onCompositionEnd={onCompositionEnd}
          onKeyDown={onKeyDown}
          rows={1}
          disabled={composerMode === "stop" || isStopping}
          placeholder={isPlainChat ? "发送普通聊天消息..." : "给调度智能体发送消息..."}
        />
        {!isPlainChat && <PlanModeToggleButton mode={mode} onToggleMode={onToggleMode} />}
        <ThinkingToggleButton
          active={thinkingEnabled}
          onToggle={onToggleThinking}
          disabled={composerMode === "stop" || isStopping}
        />
        <button
          style={styles.voiceBtn(isRecordingVoice)}
          onClick={onToggleVoiceInput}
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
              ? onStop
              : composerMode === "resume" && !input.trim()
                ? onResume
                : onSend
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
  );
});
