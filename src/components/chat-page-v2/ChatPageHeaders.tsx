import { Waypoints, X } from "lucide-react";
import type { McpStatus } from "../../types";
import { getMcpIndicatorState } from "../../hooks/use-mcp-status";
import { Button } from "../ui/button";

interface CommonHeaderProps {
  isLoading: boolean;
  hasMessages: boolean;
  mcpStatus: McpStatus | null;
  mcpChecking: boolean;
  onOpenMcpStatus?: () => void;
  onClearMessages: () => void;
  onOpenSettings: () => void;
}

export function PlainChatHeader({
  title,
  isLoading,
  hasMessages,
  mcpStatus,
  mcpChecking,
  onOpenMcpStatus,
  onClearMessages,
  onOpenSettings,
}: CommonHeaderProps & { title: string | null }) {
  const mcpIndicator = getMcpIndicatorState(mcpStatus, mcpChecking);
  return (
    <div className="flex min-h-12 items-center gap-3 px-5">
      <div className="flex min-w-0 flex-1 items-center gap-2">
        <span className="truncate text-[15px] font-semibold">{title?.trim() || "新对话"}</span>
        {isLoading && <span className="h-1.5 w-1.5 animate-pulse rounded-full bg-primary" />}
      </div>
      {onOpenMcpStatus && (
        <Button variant="outline" size="sm" onClick={onOpenMcpStatus} title="查看全局 MCP 状态">
          <span className="h-2 w-2 rounded-full" style={{ background: mcpIndicator.color }} />
          MCP <span className="font-normal text-muted-foreground">{mcpIndicator.label}</span>
        </Button>
      )}
      {hasMessages && (
        <Button variant="ghost" size="sm" onClick={onClearMessages}>
          清空
        </Button>
      )}
      <Button variant="ghost" size="sm" onClick={onOpenSettings}>
        设置
      </Button>
    </div>
  );
}

export function ProjectChatHeader({
  isLoading,
  hasMessages,
  mcpStatus,
  mcpChecking,
  graphAvailable,
  onOpenGraphPanel,
  onOpenMcpStatus,
  onClearMessages,
  onOpenSettings,
  onClosePanel,
}: CommonHeaderProps & {
  graphAvailable: boolean;
  onOpenGraphPanel: () => void;
  onClosePanel?: () => void;
}) {
  const mcpIndicator = getMcpIndicatorState(mcpStatus, mcpChecking);
  return (
    <div className="flex min-h-12 items-center gap-2 px-4">
      <div className="flex min-w-0 flex-1 items-center gap-2">
        <span className="text-[15px] font-semibold">调度智能体</span>
        {isLoading && <span className="h-1.5 w-1.5 animate-pulse rounded-full bg-primary" />}
      </div>
      <Button
        variant="outline"
        size="sm"
        onClick={onOpenGraphPanel}
        disabled={!graphAvailable}
        title={graphAvailable ? "查看最近的执行图" : "当前会话还没有执行图"}
      >
        <Waypoints size={13} strokeWidth={2} />
        执行图
      </Button>
      <Button variant="outline" size="sm" onClick={onOpenMcpStatus} title="查看 MCP 状态">
        <span className="h-2 w-2 rounded-full" style={{ background: mcpIndicator.color }} />
        MCP <span className="font-normal text-muted-foreground">{mcpIndicator.label}</span>
      </Button>
      {hasMessages && (
        <Button variant="ghost" size="sm" onClick={onClearMessages}>
          清空
        </Button>
      )}
      <Button variant="ghost" size="sm" onClick={onOpenSettings}>
        设置
      </Button>
      {onClosePanel && (
        <Button variant="ghost" size="icon-sm" aria-label="关闭会话面板" onClick={onClosePanel}>
          <X size={14} strokeWidth={2} />
        </Button>
      )}
    </div>
  );
}
