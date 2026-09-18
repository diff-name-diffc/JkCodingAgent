import {
  ArrowLeft,
  ExternalLink,
  KeyRound,
  Maximize2,
  Minimize2,
  Power,
  Square,
  X,
} from "lucide-react";

export interface BrowserPanelHeaderProps {
  statusText: string;
  connected: boolean;
  busy: boolean;
  hasSession: boolean;
  canOpenCurrentUrl: boolean;
  onGoBack: () => void;
  onStart: () => void;
  onStop: () => void;
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
  canOpenCurrentUrl,
  onGoBack,
  onStart,
  onStop,
  onImportProfile,
  onOpenExternal,
  expanded = false,
  onToggleExpanded,
  onClose,
}: BrowserPanelHeaderProps) {
  return (
    <div className="ai-browser-header">
      <div className="ai-browser-title-block">
        <div className="ai-browser-title">浏览器</div>
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
