import {
  ArrowLeft,
  ExternalLink,
  KeyRound,
  Maximize2,
  Minimize2,
  Monitor,
  MonitorDown,
  Power,
  Square,
  X,
} from "lucide-react";
import type { BrowserStatus } from "../../types";

export interface BrowserPanelHeaderProps {
  statusText: string;
  connected: boolean;
  busy: boolean;
  hasSession: boolean;
  status: BrowserStatus | null;
  canOpenCurrentUrl: boolean;
  onGoBack: () => void;
  onStart: () => void;
  onStop: () => void;
  onMinimize: () => void;
  onReopen: () => void;
  onImportProfile: () => void;
  onOpenExternal: () => void;
  expanded?: boolean;
  onToggleExpanded?: () => void;
  onClose?: () => void;
}

/**
 * 浏览器面板头部（UI-18 拆分）：标题/连接状态 + 动作按钮组。
 * 语义区分（验收条款「隐藏与关闭语义明确」）：
 * - Square「关闭浏览器」= browser_stop，真结束进程；
 * - X「关闭面板」= 仅隐藏容器，不碰进程。
 */
export function BrowserPanelHeader({
  statusText,
  connected,
  busy,
  hasSession,
  status,
  canOpenCurrentUrl,
  onGoBack,
  onStart,
  onStop,
  onMinimize,
  onReopen,
  onImportProfile,
  onOpenExternal,
  expanded = false,
  onToggleExpanded,
  onClose,
}: BrowserPanelHeaderProps) {
  return (
    <div className="ai-browser-header">
      <div className="ai-browser-title-block">
        <div className="ai-browser-title">CloakBrowser</div>
        <div className={connected ? "ai-browser-status is-connected" : "ai-browser-status"}>
          {statusText}
        </div>
      </div>
      <div className="ai-browser-actions">
        <button
          type="button"
          title="返回上一页"
          onClick={onGoBack}
          disabled={!hasSession || !connected || busy}
          className="ai-browser-icon-button"
        >
          <ArrowLeft size={14} />
        </button>
        <button
          type="button"
          title="启动浏览器"
          onClick={onStart}
          disabled={!hasSession || busy}
          className="ai-browser-icon-button"
        >
          <Power size={14} />
        </button>
        {status?.hasHeadedWindow && !status?.minimized ? (
          <button
            type="button"
            title="最小化窗口"
            onClick={onMinimize}
            disabled={!hasSession || busy || !connected}
            className="ai-browser-icon-button"
          >
            <MonitorDown size={14} />
          </button>
        ) : (
          <button
            type="button"
            title={status?.hasHeadedWindow ? "恢复窗口" : "打开独立窗口"}
            onClick={onReopen}
            disabled={!hasSession || busy || !connected}
            className="ai-browser-icon-button"
          >
            <Monitor size={14} />
          </button>
        )}
        <button
          type="button"
          title="关闭浏览器"
          onClick={onStop}
          disabled={!hasSession || busy}
          className="ai-browser-icon-button"
        >
          <Square size={14} />
        </button>
        <button
          type="button"
          title="导入 Chrome 登录态"
          onClick={onImportProfile}
          disabled={!hasSession || busy}
          className="ai-browser-icon-button"
        >
          <KeyRound size={14} />
        </button>
        <button
          type="button"
          title="外部浏览器打开"
          onClick={onOpenExternal}
          disabled={!canOpenCurrentUrl}
          className="ai-browser-icon-button"
        >
          <ExternalLink size={14} />
        </button>
        {onToggleExpanded && (
          <button
            type="button"
            title={expanded ? "还原宽度" : "展开面板"}
            onClick={onToggleExpanded}
            className="ai-browser-icon-button"
          >
            {expanded ? <Minimize2 size={14} /> : <Maximize2 size={14} />}
          </button>
        )}
        {onClose && (
          <button type="button" title="关闭面板" onClick={onClose} className="ai-browser-icon-button">
            <X size={14} />
          </button>
        )}
      </div>
    </div>
  );
}
