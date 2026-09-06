import { GitBranch, Loader2, MoreHorizontal, Settings, Trash2, Waypoints, X } from "lucide-react";
import type { McpStatus } from "../../types";
import { getMcpIndicatorState } from "../../hooks/use-mcp-status";
import { Button } from "../ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "../ui/dropdown-menu";

interface CommonHeaderProps {
  isLoading: boolean;
  /** 停止请求进行中（UI-11：运行/停止状态双编码可辨）。 */
  isStopping?: boolean;
  hasMessages: boolean;
  mcpStatus: McpStatus | null;
  mcpChecking: boolean;
  onOpenMcpStatus?: () => void;
  onClearMessages: () => void;
  onOpenSettings: () => void;
}

/**
 * 运行状态双编码（图标/点 + 文字），收敛「只靠彩点」的实现
 * （tokens.md §5 评审结论 4；空闲时不渲染，不占头部空间）。
 */
function RunStateIndicator({
  isLoading,
  isStopping,
}: {
  isLoading: boolean;
  isStopping?: boolean;
}) {
  if (isStopping) {
    return (
      <span className="ai-chat-header-runstate is-stopping" role="status">
        <Loader2 size={12} strokeWidth={2} className="animate-spin" aria-hidden="true" />
        停止中
      </span>
    );
  }
  if (isLoading) {
    return (
      <span className="ai-chat-header-runstate is-running" role="status">
        <span className="ai-chat-header-runstate-dot" aria-hidden="true" />
        运行中
      </span>
    );
  }
  return null;
}

/**
 * 更多菜单（UI-11）：清空为破坏性动作，仅在已有消息时可用；
 * 设置一并收入，头部只保留当前任务最重要的动作。
 */
function HeaderMoreMenu({
  hasMessages,
  onClearMessages,
  onOpenSettings,
}: {
  hasMessages: boolean;
  onClearMessages: () => void;
  onOpenSettings: () => void;
}) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" size="icon-sm" aria-label="更多操作">
          <MoreHorizontal size={14} strokeWidth={2} />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuItem
          disabled={!hasMessages}
          onClick={onClearMessages}
          className="text-destructive focus:text-destructive"
        >
          <Trash2 aria-hidden="true" />
          清空对话
        </DropdownMenuItem>
        <DropdownMenuItem onClick={onOpenSettings}>
          <Settings aria-hidden="true" />
          设置
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

export function PlainChatHeader({
  title,
  isLoading,
  isStopping,
  hasMessages,
  mcpStatus,
  mcpChecking,
  onOpenMcpStatus,
  onClearMessages,
  onOpenSettings,
}: CommonHeaderProps & { title: string | null }) {
  const mcpIndicator = getMcpIndicatorState(mcpStatus, mcpChecking);
  const displayTitle = title?.trim() || "新对话";
  return (
    <div className="ai-chat-header">
      <div className="ai-chat-header-main">
        <div className="ai-chat-header-title-row">
          <span className="ai-chat-header-title" title={displayTitle}>
            {displayTitle}
          </span>
          <RunStateIndicator isLoading={isLoading} isStopping={isStopping} />
        </div>
      </div>
      <div className="ai-chat-header-actions">
        {onOpenMcpStatus && (
          <Button variant="outline" size="sm" onClick={onOpenMcpStatus} title="查看全局 MCP 状态">
            <span className="h-2 w-2 rounded-full" style={{ background: mcpIndicator.color }} />
            MCP <span className="font-normal text-muted-foreground">{mcpIndicator.label}</span>
          </Button>
        )}
        <HeaderMoreMenu
          hasMessages={hasMessages}
          onClearMessages={onClearMessages}
          onOpenSettings={onOpenSettings}
        />
      </div>
    </div>
  );
}

export function ProjectChatHeader({
  sessionTitle,
  projectName,
  branchName,
  isLoading,
  isStopping,
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
  /** 当前会话任务标题（UI-11：任务身份优先，替代旧硬编码「调度智能体」）。 */
  sessionTitle: string | null;
  projectName?: string | null;
  branchName?: string | null;
  graphAvailable: boolean;
  onOpenGraphPanel: () => void;
  onClosePanel?: () => void;
}) {
  const mcpIndicator = getMcpIndicatorState(mcpStatus, mcpChecking);
  const displayTitle = sessionTitle?.trim() || "新会话";
  return (
    <div className="ai-chat-header">
      <div className="ai-chat-header-main">
        <div className="ai-chat-header-title-row">
          <span className="ai-chat-header-title" title={displayTitle}>
            {displayTitle}
          </span>
          <RunStateIndicator isLoading={isLoading} isStopping={isStopping} />
        </div>
        {(projectName || branchName) && (
          <div className="ai-chat-header-context-row">
            {projectName && <span className="truncate">{projectName}</span>}
            {branchName && (
              <span className="ai-chat-header-branch-pill" title={`当前分支：${branchName}`}>
                <GitBranch size={11} strokeWidth={2} aria-hidden="true" />
                <span className="truncate">{branchName}</span>
              </span>
            )}
          </div>
        )}
      </div>
      <div className="ai-chat-header-actions">
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
        <HeaderMoreMenu
          hasMessages={hasMessages}
          onClearMessages={onClearMessages}
          onOpenSettings={onOpenSettings}
        />
        {onClosePanel && (
          <Button variant="ghost" size="icon-sm" aria-label="关闭会话面板" onClick={onClosePanel}>
            <X size={14} strokeWidth={2} />
          </Button>
        )}
      </div>
    </div>
  );
}
