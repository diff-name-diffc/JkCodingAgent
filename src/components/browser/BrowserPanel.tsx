import { useRef } from "react";
import { BrowserAddressBar } from "./BrowserAddressBar";
import { BrowserLogPane } from "./BrowserLogPane";
import { BrowserPanelHeader } from "./BrowserPanelHeader";
import { BrowserStage } from "./BrowserStage";
import { useBrowserPanelCommands } from "./useBrowserPanelCommands";
import { useBrowserPanelSession } from "./useBrowserPanelSession";

export interface BrowserPanelProps {
  sessionId: string | null;
  projectPath?: string;
  /** 右面板宿主传入的像素宽；主区标签宿主缺省（面板铺满容器）。 */
  width?: number;
  active: boolean;
  expanded?: boolean;
  onToggleExpanded?: () => void;
  onClose?: () => void;
}

/**
 * 会话绑定的内嵌浏览器面板（组装壳，UI-18 拆分自单文件 570 行实现）：
 * - 会话态/事件监听/帧绘制 → useBrowserPanelSession（含切会话清屏）
 * - busy 单飞锁与全部命令 → useBrowserPanelCommands（projectPath 透传权限闸门）
 * - 头部/地址栏/舞台/日志 → 四个展示组件
 */
export function BrowserPanel({
  sessionId,
  projectPath,
  width,
  active,
  expanded = false,
  onToggleExpanded,
  onClose,
}: BrowserPanelProps) {
  const containerRef = useRef<HTMLElement>(null);
  const session = useBrowserPanelSession(sessionId, active, containerRef);
  const commands = useBrowserPanelCommands({ sessionId, projectPath, session });

  const { status, logs, error, canvasRef } = session;
  const { busy, stopping } = commands;

  const state = status?.state ?? "closed";
  const connected = state !== "closed" && state !== "page_closed";
  const pageClosed = state === "page_closed";
  const automating = state === "automating";
  const stateLabel = stopping ? "正在停止" : automating ? "自动化运行中（仅观察）" : state;
  const statusText = sessionId
    ? status?.message
      ? `${stateLabel} · ${status.message}`
      : stateLabel
    : "未选择会话";
  const canOpenCurrentUrl = Boolean(status?.url && status.url !== "about:blank");

  return (
    <aside
      ref={containerRef}
      className="ai-browser-panel"
      style={width != null ? { width } : undefined}
    >
      <BrowserPanelHeader
        statusText={statusText}
        connected={connected}
        busy={busy}
        automating={automating}
        stopping={stopping}
        hasSession={Boolean(sessionId)}
        canOpenCurrentUrl={canOpenCurrentUrl}
        onGoBack={() => void commands.goBack()}
        onStart={() => void commands.startBrowser()}
        onStop={() => void commands.stopBrowser()}
        onImportProfile={() => void commands.importChromeProfile()}
        onOpenExternal={() => void commands.openCurrentUrl()}
        expanded={expanded}
        onToggleExpanded={onToggleExpanded}
        onClose={onClose}
      />
      <BrowserAddressBar
        url={status?.url ?? ""}
        connected={connected}
        busy={busy || automating || stopping}
        hasSession={Boolean(sessionId)}
        onReload={() => void commands.reloadPage()}
        onNavigate={(url) => void commands.navigateTo(url)}
      />
      <BrowserStage
        canvasRef={canvasRef}
        busy={busy || stopping}
        readOnly={automating}
        hasSession={Boolean(sessionId)}
        connected={connected}
        pageClosed={pageClosed}
        onCanvasClick={(event) => void commands.handleCanvasClick(event)}
      />
      <BrowserLogPane error={error} logs={logs} />
    </aside>
  );
}
