import { useEffect, useState } from "react";
import * as DialogPrimitive from "@radix-ui/react-dialog";
import { Button } from "../ui/button";

/** 画板命名/重命名输入弹窗（Enter 提交，Esc 关闭）。 */
export function CanvasNameDialog({
  open,
  title,
  initialName,
  onSubmit,
  onCancel,
}: {
  open: boolean;
  title: string;
  initialName: string;
  onSubmit: (name: string) => void;
  onCancel: () => void;
}) {
  const [name, setName] = useState(initialName);

  useEffect(() => {
    if (open) setName(initialName);
  }, [open, initialName]);

  const submit = () => {
    const trimmed = name.trim();
    if (trimmed) onSubmit(trimmed);
  };

  return (
    <DialogPrimitive.Root
      open={open}
      onOpenChange={(next) => {
        if (!next) onCancel();
      }}
    >
      <DialogPrimitive.Portal>
        <DialogPrimitive.Overlay className="ai-arch-dialog-overlay" />
        <DialogPrimitive.Content
          className="ai-arch-dialog"
          onOpenAutoFocus={(event) => event.preventDefault()}
        >
          <DialogPrimitive.Title className="ai-arch-dialog-title">{title}</DialogPrimitive.Title>
          <input
            className="ai-arch-dialog-input"
            value={name}
            autoFocus
            maxLength={80}
            placeholder="输入画板名称"
            onChange={(event) => setName(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") submit();
            }}
          />
          <div className="ai-arch-dialog-actions">
            <Button variant="outline" size="sm" onClick={onCancel}>
              取消
            </Button>
            <Button size="sm" disabled={!name.trim()} onClick={submit}>
              确定
            </Button>
          </div>
        </DialogPrimitive.Content>
      </DialogPrimitive.Portal>
    </DialogPrimitive.Root>
  );
}
