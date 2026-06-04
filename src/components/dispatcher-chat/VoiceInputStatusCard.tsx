import { memo } from "react";
import { Mic } from "lucide-react";
import { dispatcherChatStyles as styles } from "./dispatcherChatStyles";

export const VoiceInputStatusCard = memo(function VoiceInputStatusCard({
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
