import { MonitorDot, Terminal } from "lucide-react";
import { cn } from "../../lib/cn";

interface StatusDockBarProps {
  terminalActive: boolean;
  onToggleTerminal: () => void;
  browserActive: boolean;
  onToggleBrowser: () => void;
  /** 左侧简洁状态文本（分支/运行提示等由调用方提供）。 */
  statusText?: string;
}

/**
 * 底部状态/dock 条（UI-07）：24px 高，终端与浏览器的稳定 toggle 入口。
 * 取代旧 48px 右工具栏的纵向入口（文件/变更已归 ContextNav 页签）。
 */
export function StatusDockBar({
  terminalActive,
  onToggleTerminal,
  browserActive,
  onToggleBrowser,
  statusText,
}: StatusDockBarProps) {
  return (
    <div className="ai-status-dock-bar">
      <span className="min-w-0 flex-1 truncate">{statusText}</span>
      <button
        type="button"
        className={cn("ai-status-dock-toggle", terminalActive && "is-active")}
        onClick={onToggleTerminal}
        aria-pressed={terminalActive}
      >
        <Terminal size={12} strokeWidth={2} />
        终端
      </button>
      <button
        type="button"
        className={cn("ai-status-dock-toggle", browserActive && "is-active")}
        onClick={onToggleBrowser}
        aria-pressed={browserActive}
      >
        <MonitorDot size={12} strokeWidth={2} />
        浏览器
      </button>
    </div>
  );
}
