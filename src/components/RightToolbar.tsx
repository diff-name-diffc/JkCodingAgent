import type { ReactNode } from "react";
import { IconButton } from "./IconButton";
import { Folder, GitBranch, History, Terminal, MonitorDot } from "lucide-react";
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
