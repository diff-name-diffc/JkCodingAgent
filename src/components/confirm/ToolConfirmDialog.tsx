import * as DialogPrimitive from "@radix-ui/react-dialog";
import { ShieldAlert } from "lucide-react";
import { Button } from "../ui/button";
import type { ToolConfirmRequest } from "./tool-confirm-store";

/**
 * 命令审查「需用户确认」弹窗：审查 AI 判定不通过、但用户可人工放行的命令，
 * 在此展示工具、目标、命令与拦截原因，由用户裁决「允许执行 / 拒绝」。
 *
 * 提权（sudo）命令额外显著提示，避免用户误以为会以当前登录用户身份执行。
 * 复用 `settings/ConfirmDialog` 同款的 Radix AlertDialog 结构与 `.ai-set-confirm*`
 * 样式类（无关闭叉、Esc 关闭等价于拒绝，避免误触放行）。
 */
export function ToolConfirmDialog({
  open,
  request,
  remaining,
  onApprove,
  onReject,
}: {
  open: boolean;
  request: ToolConfirmRequest;
  /** 排队中的确认请求总数（>1 时提示还有几条）。 */
  remaining?: number;
  onApprove: () => void;
  onReject: () => void;
}) {
  return (
    <DialogPrimitive.Root
      open={open}
      onOpenChange={(next) => {
        // Esc / 点击遮罩一律按「拒绝」处理：绝不让关闭动作变成放行。
        if (!next) onReject();
      }}
    >
      <DialogPrimitive.Portal>
        <DialogPrimitive.Overlay className="ai-set-confirm-overlay" />
        <DialogPrimitive.Content
          className="ai-set-confirm"
          style={{ maxWidth: "min(560px, 92vw)" }}
          onOpenAutoFocus={(e) => e.preventDefault()}
        >
          <div className="ai-set-confirm-icon">
            <ShieldAlert size={20} strokeWidth={1.5} />
          </div>
          <DialogPrimitive.Title className="ai-set-confirm-title">
            {request.elevated ? "提权命令需要确认" : "命令需要确认"}
          </DialogPrimitive.Title>
          <DialogPrimitive.Description className="ai-set-confirm-description">
            {request.elevated
              ? "该命令将以 sudo 提权（root）执行，安全审查未自动放行。请确认内容与风险后再决定是否允许。"
              : "该命令未通过安全审查的自动放行，请确认内容与风险后再决定是否允许。"}
          </DialogPrimitive.Description>

          <div className="ai-tool-confirm-body">
            <div className="ai-tool-confirm-row">
              <span className="ai-tool-confirm-label">工具</span>
              <span className="ai-tool-confirm-value">{request.tool}</span>
            </div>
            <div className="ai-tool-confirm-row">
              <span className="ai-tool-confirm-label">目标</span>
              <span className="ai-tool-confirm-value">{request.target}</span>
            </div>
            <div className="ai-tool-confirm-row is-block">
              <span className="ai-tool-confirm-label">命令</span>
              <pre className="ai-tool-confirm-code">{request.command}</pre>
            </div>
            <div className="ai-tool-confirm-row is-block">
              <span className="ai-tool-confirm-label">拦截原因</span>
              <p className="ai-tool-confirm-reason">{request.reason}</p>
            </div>
            {remaining !== undefined && remaining > 0 && (
              <p className="ai-tool-confirm-hint">
                还有 {remaining} 条待确认请求，将按顺序依次展示。
              </p>
            )}
          </div>

          <div className="ai-set-confirm-actions">
            <Button variant="outline" size="sm" onClick={onReject}>
              拒绝
            </Button>
            <Button variant="destructive" size="sm" onClick={onApprove}>
              允许执行
            </Button>
          </div>
        </DialogPrimitive.Content>
      </DialogPrimitive.Portal>
    </DialogPrimitive.Root>
  );
}
