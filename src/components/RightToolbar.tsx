import type { ReactNode } from "react";
import { IconButton } from "./IconButton";
import { Folder, Search, GitBranch, History, Terminal, MonitorDot } from "lucide-react";
import type { RightPanel } from "../hooks/projectPanelsFileState";

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
  }> = [
    { key: "files", icon: <Folder size={17} />, title: "文件浏览器" },
    { key: "git-changes", icon: <GitBranch size={17} />, title: "Git 变更" },
    { key: "git-history", icon: <History size={17} />, title: "Git 历史" },
    { key: "browser", icon: <MonitorDot size={17} />, title: "CloakBrowser" },
  ];

  const placeholders = [{ icon: <Search size={17} />, title: "搜索（即将上线）" }];

  return (
    <div
      style={{
        width: 44,
        flexShrink: 0,
        background: "var(--bg-sidebar)",
        borderLeft: "1px solid var(--border-dim)",
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        paddingTop: 6,
        paddingBottom: 8,
        gap: 2,
        overflow: "hidden",
      }}
    >
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

      <div style={{ width: 20, height: 1, background: "var(--border-dim)", margin: "4px 0" }} />

      {placeholders.map((p, i) => (
        <IconButton key={i} icon={p.icon} title={p.title} disabled />
      ))}
      <div style={{ flex: 1 }} />
    </div>
  );
}
