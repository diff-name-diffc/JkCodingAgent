import * as DialogPrimitive from "@radix-ui/react-dialog";
import { TriangleAlert } from "lucide-react";
import { Button } from "../ui/button";

/**
 * 删除/丢弃类操作的二次确认弹窗（AlertDialog 语义）。
 * description 必须说明影响范围，例如「删除后使用此服务商的 2 个用途将失效」。
 */
export function ConfirmDialog({
  open,
  title,
  description,
  confirmLabel = "删除",
  cancelLabel = "取消",
  danger = true,
  onConfirm,
  onCancel,
}: {
  open: boolean;
  title: string;
  description: string;
  confirmLabel?: string;
  cancelLabel?: string;
  /** true（默认）确认按钮用危险色，仅用于删除/丢弃类操作。 */
  danger?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  return (
    <DialogPrimitive.Root
      open={open}
      onOpenChange={(next) => {
        if (!next) onCancel();
      }}
    >
      <DialogPrimitive.Portal>
        <DialogPrimitive.Overlay className="ai-set-confirm-overlay" />
        <DialogPrimitive.Content
          className="ai-set-confirm"
          onOpenAutoFocus={(e) => e.preventDefault()}
        >
          <div className="ai-set-confirm-icon">
            <TriangleAlert size={20} strokeWidth={1.5} />
          </div>
          <DialogPrimitive.Title className="ai-set-confirm-title">
            {title}
          </DialogPrimitive.Title>
          <DialogPrimitive.Description className="ai-set-confirm-description">
            {description}
          </DialogPrimitive.Description>
          <div className="ai-set-confirm-actions">
            <Button variant="outline" size="sm" onClick={onCancel}>
              {cancelLabel}
            </Button>
            <Button
              variant={danger ? "destructive" : "default"}
              size="sm"
              onClick={onConfirm}
            >
              {confirmLabel}
            </Button>
          </div>
        </DialogPrimitive.Content>
      </DialogPrimitive.Portal>
    </DialogPrimitive.Root>
  );
}
