import * as React from "react";
import * as DialogPrimitive from "@radix-ui/react-dialog";
import { X } from "lucide-react";
import { cn } from "../../lib/cn";

/**
 * shadcn 风格 Dialog 封装（UI-09）：项目内 Radix Dialog 原语的统一出口，
 * 自带 portal/焦点陷阱/Escape 层级（DismissableLayer）与焦点还原。
 * 骨架参考 settings/ConfirmDialog.tsx 的既有用法。
 */
const Dialog = DialogPrimitive.Root;
const DialogTrigger = DialogPrimitive.Trigger;
const DialogClose = DialogPrimitive.Close;

const DialogOverlay = React.forwardRef<
  React.ElementRef<typeof DialogPrimitive.Overlay>,
  React.ComponentPropsWithoutRef<typeof DialogPrimitive.Overlay>
>(({ className, ...props }, ref) => (
  <DialogPrimitive.Overlay
    ref={ref}
    className={cn("ai-dialog-overlay ai-ui-dialog-overlay", className)}
    {...props}
  />
));
DialogOverlay.displayName = "DialogOverlay";

const DialogContent = React.forwardRef<
  React.ElementRef<typeof DialogPrimitive.Content>,
  React.ComponentPropsWithoutRef<typeof DialogPrimitive.Content> & { hideClose?: boolean }
>(({ className, children, hideClose, ...props }, ref) => (
  <DialogPrimitive.Portal>
    <DialogOverlay />
    <DialogPrimitive.Content
      ref={ref}
      className={cn("ai-ui-dialog-content", className)}
      {...props}
    >
      {children}
      {hideClose ? null : (
        <DialogPrimitive.Close asChild>
          <button type="button" className="ai-ui-dialog-close" aria-label="关闭">
            <X className="h-4 w-4" />
          </button>
        </DialogPrimitive.Close>
      )}
    </DialogPrimitive.Content>
  </DialogPrimitive.Portal>
));
DialogContent.displayName = "DialogContent";

const DialogTitle = DialogPrimitive.Title;
const DialogDescription = DialogPrimitive.Description;

export { Dialog, DialogTrigger, DialogClose, DialogOverlay, DialogContent, DialogTitle, DialogDescription };
