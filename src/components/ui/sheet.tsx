import * as React from "react";
import * as DialogPrimitive from "@radix-ui/react-dialog";
import { cn } from "../../lib/cn";

/**
 * 侧边抽屉（UI-09）：基于 Radix Dialog 的右侧 Sheet 变体，用于窄屏详情
 * （Python 运行详情、嵌入式 Artifact 等）。继承 Dialog 的焦点陷阱、
 * Escape 层级与焦点还原语义；滑入动效 180ms 与全局节奏一致。
 */
const Sheet = DialogPrimitive.Root;
const SheetClose = DialogPrimitive.Close;

const SheetContent = React.forwardRef<
  React.ElementRef<typeof DialogPrimitive.Content>,
  React.ComponentPropsWithoutRef<typeof DialogPrimitive.Content> & { width?: string }
>(({ className, children, width, ...props }, ref) => (
  <DialogPrimitive.Portal>
    <DialogPrimitive.Overlay className="ai-dialog-overlay ai-ui-sheet-overlay" />
    <DialogPrimitive.Content
      ref={ref}
      className={cn("ai-ui-sheet-content", className)}
      style={{ width: width ?? "min(420px, 92%)" }}
      {...props}
    >
      {children}
    </DialogPrimitive.Content>
  </DialogPrimitive.Portal>
));
SheetContent.displayName = "SheetContent";

const SheetTitle = DialogPrimitive.Title;
const SheetDescription = DialogPrimitive.Description;

export { Sheet, SheetClose, SheetContent, SheetTitle, SheetDescription };
