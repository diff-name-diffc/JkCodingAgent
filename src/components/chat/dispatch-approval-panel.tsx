import * as React from "react";
import { Bot, Check, X } from "lucide-react";
import type { AgentType } from "../../types";
import { Button } from "../ui/button";
import { Textarea } from "../ui/textarea";
import { Badge } from "../ui/badge";

export interface DispatchApprovalPanelProps {
  dispatchId: string;
  agent: AgentType;
  description: string;
  taskPrompt: string;
  permissionMode: string;
  onApprove: (dispatchId: string, taskPrompt: string) => void;
  onReject: (dispatchId: string) => void;
}

export function DispatchApprovalPanel({
  dispatchId,
  agent,
  description,
  taskPrompt,
  permissionMode,
  onApprove,
  onReject,
}: DispatchApprovalPanelProps) {
  const [editedTaskPrompt, setEditedTaskPrompt] = React.useState(taskPrompt);
  const agentLabel = agent === "claude" ? "Claude" : "Codex";

  return (
    <div className="absolute inset-0 z-30 flex items-center justify-center bg-background/70 p-4 backdrop-blur-sm">
      <div className="flex max-h-[86vh] w-full max-w-2xl flex-col overflow-hidden rounded-lg border border-border bg-card shadow-soft">
        <div className="flex items-center gap-2 border-b border-border px-4 py-3">
          <Bot className="h-4 w-4 text-primary" />
          <div className="min-w-0 flex-1">
            <div className="text-sm font-medium">确认启动子任务</div>
            <div className="truncate text-xs text-muted-foreground">{description}</div>
          </div>
          <Badge variant="outline">{agentLabel}</Badge>
          <Badge variant="secondary">{permissionMode}</Badge>
        </div>

        <div className="min-h-0 flex-1 space-y-2 overflow-auto p-4">
          <Textarea
            value={editedTaskPrompt}
            onChange={(event) => setEditedTaskPrompt(event.target.value)}
            rows={16}
            aria-label="子任务提示词"
            className="min-h-[320px] resize-y font-mono text-xs leading-relaxed"
          />
        </div>

        <div className="flex justify-end gap-2 border-t border-border px-4 py-3">
          <Button variant="ghost" onClick={() => onReject(dispatchId)}>
            <X className="h-4 w-4" />
            拒绝
          </Button>
          <Button onClick={() => onApprove(dispatchId, editedTaskPrompt)}>
            <Check className="h-4 w-4" />
            批准运行
          </Button>
        </div>
      </div>
    </div>
  );
}
