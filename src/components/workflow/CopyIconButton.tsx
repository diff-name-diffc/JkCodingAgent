import { useEffect, useRef, useState } from "react";
import { Check, Copy } from "lucide-react";
import { copyTextToClipboard } from "../../lib/clipboard-fallback";
import { cn } from "../../lib/cn";
import { toast } from "../Toast";
import { Button } from "../ui/button";

export interface CopyIconButtonProps {
  /** 待复制文本（原始 markdown / 完整输出，非展示层截断值）；空值禁用。 */
  value: string | null | undefined;
  /** 无障碍名与悬停提示（如「复制结论文本」）。 */
  label: string;
  className?: string;
}

/** 复制成功反馈复位延时（与聊天消息 / 代码块复制按钮同一量级）。 */
const COPIED_RESET_MS = 1500;

/**
 * 工作流域共享的复制图标按钮：Copy ↔ Check 成功反馈 + 空值禁用。
 * 写剪贴板统一走 copyTextToClipboard（WKWebView 三级降级链），
 * 失败 toast 提示（复制静默落空比失败提示更难排查）。
 */
export function CopyIconButton({ value, label, className }: CopyIconButtonProps) {
  const [copied, setCopied] = useState(false);
  const timerRef = useRef<number | null>(null);

  useEffect(() => {
    if (!copied) return;
    timerRef.current = window.setTimeout(() => setCopied(false), COPIED_RESET_MS);
    return () => {
      if (timerRef.current !== null) window.clearTimeout(timerRef.current);
      timerRef.current = null;
    };
  }, [copied]);

  const empty = !value;

  const handleCopy = async () => {
    if (!value || copied) return;
    try {
      await copyTextToClipboard(value);
      setCopied(true);
    } catch (error) {
      toast.warning(`复制失败：${error instanceof Error ? error.message : String(error)}`);
    }
  };

  return (
    <Button
      type="button"
      variant="ghost"
      size="icon-sm"
      className={cn("text-muted-foreground", className)}
      disabled={empty}
      aria-label={copied ? "已复制" : label}
      title={copied ? "已复制" : label}
      onClick={() => void handleCopy()}
    >
      {copied ? <Check className="h-3.5 w-3.5" /> : <Copy className="h-3.5 w-3.5" />}
    </Button>
  );
}
