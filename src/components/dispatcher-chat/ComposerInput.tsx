import { memo, useCallback, type RefObject } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { X, Mic, Play, Send, Square } from "lucide-react";
import { useComposedInput } from "../../hooks/useComposedInput";
import type {
  ChecklistPlanState,
  DispatcherModelConfig,
  DispatcherMode,
  DispatcherSessionTokenUsage,
  ImageSegment,
  PlanInteraction,
} from "../../types";
import { SessionTokenUsageIndicators } from "../SessionTokenUsageIndicators";
import { PlanModeToggleButton } from "./ComposerButtons";
import { VoiceInputStatusCard } from "./VoiceInputStatusCard";
import { InteractionDrawer } from "./InteractionDrawer";
import { CommandComposer } from "../ui/chatPrimitives";
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
  chatModelConfigs: DispatcherModelConfig[];
  activeChatModelIndex: number;
  onChangeInput: (value: string) => void;
  onPaste: (e: React.ClipboardEvent) => void;
  onDrop: (e: React.DragEvent) => void;
  onDragOver: (e: React.DragEvent) => void;
  onRemoveImage: (index: number) => void;
  onSend: () => void;
  onStop: () => void;
  onResume: () => void;
  onToggleMode: (mode: DispatcherMode) => void;
  onToggleVoiceInput: () => void;
  onDismissVoiceError: () => void;
  onAnswerPlanQuestion: (answer: string) => void;
  onImplementPlan: (interaction: Extract<PlanInteraction, { kind: "ready" }>) => void;
  onImplementPlanWithClearedContext: (
    interaction: Extract<PlanInteraction, { kind: "ready" }>,
  ) => void;
  onStayInPlanMode: () => void;
  onSelectChatModel: (value: string) => void;
}

export const ComposerInput = memo(function ComposerInput({
  isPlainChat,
  input,
  attachedImages,
  composerMode,
  mode,
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
  chatModelConfigs,
  activeChatModelIndex,
  onChangeInput,
  onPaste,
  onDrop,
  onDragOver,
  onRemoveImage,
  onSend,
  onStop,
  onResume,
  onToggleMode,
  onToggleVoiceInput,
  onDismissVoiceError,
  onAnswerPlanQuestion,
  onImplementPlan,
  onImplementPlanWithClearedContext,
  onStayInPlanMode,
  onSelectChatModel,
}: ComposerInputProps) {
  const onEnter = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key !== "Enter" || e.shiftKey) return;
      e.preventDefault();
      if (composerMode === "stop") return;
      if (composerMode === "resume" && !input.trim()) {
        onResume();
        return;
      }
      if (composerMode === "send") onSend();
    },
    [composerMode, input, onSend, onResume],
  );
  const composed = useComposedInput(onEnter);

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
      <CommandComposer style={styles.inputArea} onDrop={onDrop} onDragOver={onDragOver}>
        {attachedImages.length > 0 && (
          <div style={styles.attachedImagesContainer}>
            {attachedImages.map((img, idx) => (
              <div key={idx} style={styles.attachedImageWrapper}>
                <img
                  src={convertFileSrc(img.path)}
                  alt={img.alt || "pasted"}
                  style={styles.attachedImage}
                />
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
        <SessionTokenUsageIndicators
          entries={sessionTokenUsageEntries}
          chatModelConfigs={isPlainChat ? chatModelConfigs : []}
          activeChatModelIndex={activeChatModelIndex}
          modelSwitchDisabled={composerMode === "stop" || isStopping}
          onSelectChatModel={onSelectChatModel}
        />
        <textarea
          ref={inputRef}
          style={styles.inputTextarea}
          value={input}
          onChange={(e) => onChangeInput(e.target.value)}
          onPaste={onPaste}
          onCompositionStart={composed.onCompositionStart}
          onCompositionEnd={composed.onCompositionEnd}
          onKeyDown={composed.handleKeyDown}
          rows={1}
          disabled={composerMode === "stop" || isStopping}
          placeholder={isPlainChat ? "发送普通聊天消息..." : "给调度智能体发送消息..."}
        />
        {!isPlainChat && <PlanModeToggleButton mode={mode} onToggleMode={onToggleMode} />}
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
      </CommandComposer>
    </>
  );
});
