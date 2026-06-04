import { useState } from "react";
import type { AgentType } from "../../types";
import { DISPATCH_AGENT_META, dispatcherChatStyles as styles } from "./dispatcherChatStyles";

export interface DispatchApprovalProps {
  dispatchId: string;
  agent: AgentType;
  description: string;
  taskPrompt: string;
  permissionMode: string;
  onApprove: (dispatchId: string, taskPrompt: string) => void;
  onReject: (dispatchId: string) => void;
}

export function DispatchApprovalDialog({
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
