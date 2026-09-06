import type { ReactNode } from "react";
import { IconButton } from "./IconButton";
import { Terminal, MonitorDot } from "lucide-react";
import type { RightPanel } from "../hooks/projectPanelsFileState";

/**
 * 过渡态（UI-07 commit 1）：文件/变更/历史入口已移入 ContextNav 页签，
 * 本工具栏仅保留浏览器（右面板，UI-18 迁移前）与终端；
 * commit 2 由 StatusDockBar 取代后整体删除。
 */
export function RightToolbar({
  activePanel,
  onToggle,
  terminalActive,
  onToggleTerminal,
}: {
  activePanel: RightPanel;
  onToggle: (panel: Exclude<RightPanel, null>) => void;
  terminalActive: boolean;
  onToggleTerminal: () => void;
}) {
  const buttons: Array<{
    key: Exclude<RightPanel, null>;
    icon: ReactNode;
    title: string;
  }> = [{ key: "browser", icon: <MonitorDot size={17} />, title: "CloakBrowser" }];

  return (
    <div className="ai-project-right-toolbar">
      {buttons.map((btn) => (
        <IconButton
          key={btn.key}
          icon={btn.icon}
          title={btn.title}
          active={activePanel === btn.key}
          onClick={() => onToggle(btn.key)}
        />
      ))}

      <IconButton
        icon={<Terminal size={17} />}
        title="终端"
        active={terminalActive}
        onClick={onToggleTerminal}
      />

      <div className="ai-project-right-toolbar-spacer" />
    </div>
  );
}
