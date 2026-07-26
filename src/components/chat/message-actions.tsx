import * as React from "react";
import { Check, Copy, RefreshCw, ThumbsDown, ThumbsUp } from "lucide-react";
import { cn } from "../../lib/cn";
import { Button } from "../ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "../ui/tooltip";

type Feedback = "up" | "down" | null;

export interface MessageActionsProps {
  tokenLabel?: string;
  onCopy: () => void;
  onRegenerate?: () => void;
  className?: string;
}

export function MessageActions({
  tokenLabel,
  onCopy,
  onRegenerate,
  className,
}: MessageActionsProps) {
  const [copied, setCopied] = React.useState(false);
  const [feedback, setFeedback] = React.useState<Feedback>(null);
  const resetTimerRef = React.useRef<number | null>(null);

  React.useEffect(
    () => () => {
      if (resetTimerRef.current !== null) window.clearTimeout(resetTimerRef.current);
    },
    [],
  );

  const handleCopy = () => {
    onCopy();
    setCopied(true);
    if (resetTimerRef.current !== null) window.clearTimeout(resetTimerRef.current);
    resetTimerRef.current = window.setTimeout(() => setCopied(false), 1500);
  };

  return (
    <div className={cn("ai-message-actions", className)}>
      {tokenLabel && <span className="mr-1 text-[11px] text-muted-foreground">{tokenLabel}</span>}
      <ActionButton label={copied ? "已复制" : "复制"} onClick={handleCopy}>
        {copied ? <Check className="h-4 w-4" /> : <Copy className="h-4 w-4" />}
      </ActionButton>
      {onRegenerate && (
        <ActionButton label="重新生成" onClick={onRegenerate}>
          <RefreshCw className="h-4 w-4" />
        </ActionButton>
      )}
      <ActionButton
        label="赞"
        pressed={feedback === "up"}
        onClick={() => setFeedback((value) => (value === "up" ? null : "up"))}
      >
        <ThumbsUp className="h-4 w-4" />
      </ActionButton>
      <ActionButton
        label="踩"
        pressed={feedback === "down"}
        onClick={() => setFeedback((value) => (value === "down" ? null : "down"))}
      >
        <ThumbsDown className="h-4 w-4" />
      </ActionButton>
    </div>
  );
}

export function ActionButton({
  label,
  pressed,
  onClick,
  children,
}: {
  label: string;
  pressed?: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          className={cn("ai-message-action", pressed && "ai-message-action-active")}
          aria-label={label}
          aria-pressed={pressed}
          onClick={onClick}
        >
          {children}
        </Button>
      </TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  );
}
