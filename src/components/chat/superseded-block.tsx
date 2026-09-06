import * as React from "react";

/**
 * 被降级为「中间推理」的正文段折叠块（UI-12 抽为共享组件）：
 * AssistantMessage（历史）与 StreamingMessage（实时）同轮次必须渲染一致——
 * 实时管道里 demoteActiveTextSegments 降级的段落同样走此折叠灰块。
 */
export function SupersededBlock({ text }: { text: string }) {
  const [open, setOpen] = React.useState(false);
  return (
    <div className="rounded-md border border-dashed border-border/70 bg-muted/30">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
        className="w-full px-3 py-1.5 text-left text-[11px] text-muted-foreground hover:text-foreground"
      >
        {open ? "收起中间推理" : "查看中间推理"}
      </button>
      {open && (
        <pre className="chat-scroll max-h-48 overflow-auto px-3 pb-2 font-mono text-[12px] leading-relaxed text-muted-foreground">
          {text}
        </pre>
      )}
    </div>
  );
}
