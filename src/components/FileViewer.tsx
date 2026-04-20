import { useMemo, useState } from "react";
import * as Popover from "@radix-ui/react-popover";
import { MoreHorizontal, X } from "lucide-react";
import { FileGlyph, resolveFilePresentation } from "../file-icons";
import { FileTabPane } from "./file-viewer/FileTabPane";
import type { OpenFileTab } from "../hooks/useProjectPanels";

export function FileViewer({
  tabs,
  activeTabId,
  projectPath,
  onSelectTab,
  onCloseTab,
  onCloseOtherTabs,
  onCloseTabsToRight,
  onCloseAllTabs,
  onHide,
  isDark,
  onRunMakeTarget: _onRunMakeTarget,
}: {
  tabs: OpenFileTab[];
  activeTabId: string | null;
  projectPath: string;
  onSelectTab: (tabId: string) => void;
  onCloseTab: (tabId: string) => void;
  onCloseOtherTabs: (tabId: string) => void;
  onCloseTabsToRight: (tabId: string) => void;
  onCloseAllTabs: () => void;
  onHide: () => void;
  isDark: boolean;
  onRunMakeTarget?: (target: string) => void;
}) {
  const [menuOpen, setMenuOpen] = useState(false);

  const activeTab = useMemo(
    () => tabs.find((tab) => tab.id === activeTabId) ?? tabs[tabs.length - 1] ?? null,
    [activeTabId, tabs],
  );

  if (!activeTab) {
    return null;
  }

  const canCloseOtherTabs = tabs.length > 1;
  const activeTabIndex = tabs.findIndex((tab) => tab.id === activeTab.id);
  const canCloseTabsToRight = activeTabIndex !== -1 && activeTabIndex < tabs.length - 1;

  return (
    <div
      style={{
        flex: 1,
        display: "flex",
        flexDirection: "column",
        overflow: "hidden",
        minWidth: 0,
        minHeight: 0,
        background:
          "linear-gradient(180deg, color-mix(in srgb, var(--bg-sidebar) 64%, transparent), var(--bg-panel))",
      }}
    >
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 10,
          minWidth: 0,
          padding: "10px 12px",
          borderBottom: "1px solid var(--border-dim)",
          background: "color-mix(in srgb, var(--bg-card) 74%, transparent)",
          backdropFilter: "blur(14px)",
          WebkitBackdropFilter: "blur(14px)",
        }}
      >
        <div
          className="file-viewer-tab-strip"
          style={{
            flex: 1,
            minWidth: 0,
            display: "flex",
            alignItems: "center",
            gap: 6,
            overflowX: "auto",
            overflowY: "hidden",
            paddingBottom: 2,
          }}
        >
          {tabs.map((tab) => {
            const isActive = tab.id === activeTab.id;
            const presentation = resolveFilePresentation({ name: tab.name, path: tab.path });

            return (
              <button
                key={tab.id}
                type="button"
                onClick={() => onSelectTab(tab.id)}
                title={tab.path}
                style={{
                  minWidth: 0,
                  maxWidth: 260,
                  display: "flex",
                  alignItems: "center",
                  gap: 8,
                  padding: "10px 12px",
                  borderRadius: 16,
                  border: isActive
                    ? "1px solid color-mix(in srgb, var(--accent) 24%, var(--border-dim))"
                    : "1px solid transparent",
                  background: isActive
                    ? "linear-gradient(135deg, color-mix(in srgb, var(--accent) 9%, var(--bg-card)), color-mix(in srgb, var(--bg-card) 88%, transparent))"
                    : "transparent",
                  color: isActive ? "var(--text-primary)" : "var(--text-secondary)",
                  cursor: "pointer",
                  flexShrink: 0,
                  boxShadow: isActive ? "0 10px 24px rgba(15, 23, 42, 0.05)" : "none",
                }}
              >
                <FileGlyph presentation={presentation} size={20} />
                <span
                  style={{
                    overflow: "hidden",
                    textOverflow: "ellipsis",
                    whiteSpace: "nowrap",
                    fontSize: 12.5,
                    fontWeight: isActive ? 600 : 500,
                  }}
                >
                  {tab.name}
                </span>
                <span
                  onClick={(event) => {
                    event.stopPropagation();
                    onCloseTab(tab.id);
                  }}
                  style={{
                    display: "flex",
                    alignItems: "center",
                    justifyContent: "center",
                    width: 20,
                    height: 20,
                    borderRadius: 999,
                    color: "var(--text-hint)",
                    flexShrink: 0,
                  }}
                  role="button"
                  aria-label={`关闭 ${tab.name}`}
                >
                  <X size={12} />
                </span>
              </button>
            );
          })}
        </div>

        <Popover.Root open={menuOpen} onOpenChange={setMenuOpen}>
          <Popover.Trigger asChild>
            <button
              type="button"
              title="标签操作"
              aria-label="标签操作"
              style={{
                width: 36,
                height: 36,
                borderRadius: 12,
                border: "1px solid var(--border-dim)",
                background: "color-mix(in srgb, var(--bg-card) 88%, transparent)",
                color: "var(--text-secondary)",
                cursor: "pointer",
                display: "flex",
                alignItems: "center",
                justifyContent: "center",
                flexShrink: 0,
              }}
            >
              <MoreHorizontal size={15} />
            </button>
          </Popover.Trigger>
          <Popover.Portal>
            <Popover.Content
              sideOffset={6}
              align="end"
              onOpenAutoFocus={(event) => event.preventDefault()}
              className="file-viewer-tab-menu"
            >
              <button
                type="button"
                disabled={!canCloseOtherTabs}
                onClick={() => {
                  onCloseOtherTabs(activeTab.id);
                  setMenuOpen(false);
                }}
                className="file-viewer-tab-menu-item"
              >
                关闭其他标签
              </button>
              <button
                type="button"
                disabled={!canCloseTabsToRight}
                onClick={() => {
                  onCloseTabsToRight(activeTab.id);
                  setMenuOpen(false);
                }}
                className="file-viewer-tab-menu-item"
              >
                关闭右侧标签
              </button>
              <button
                type="button"
                disabled={tabs.length === 0}
                onClick={() => {
                  onCloseAllTabs();
                  setMenuOpen(false);
                }}
                className="file-viewer-tab-menu-item"
              >
                关闭全部标签
              </button>
            </Popover.Content>
          </Popover.Portal>
        </Popover.Root>

        <button
          type="button"
          title="隐藏文件编辑器"
          aria-label="隐藏文件编辑器"
          onClick={onHide}
          style={{
            width: 36,
            height: 36,
            borderRadius: 12,
            border: "1px solid var(--border-dim)",
            background: "color-mix(in srgb, var(--bg-card) 88%, transparent)",
            color: "var(--text-secondary)",
            cursor: "pointer",
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            flexShrink: 0,
          }}
        >
          <X size={15} />
        </button>
      </div>

      <div
        style={{
          flex: 1,
          position: "relative",
          minWidth: 0,
          minHeight: 0,
        }}
      >
        {tabs.map((tab) => {
          const isActive = tab.id === activeTab.id;

          return (
            <div
              key={tab.id}
              style={{
                position: "absolute",
                inset: 0,
                display: isActive ? "flex" : "none",
                minWidth: 0,
                minHeight: 0,
              }}
            >
              <FileTabPane active={isActive} tab={tab} projectPath={projectPath} isDark={isDark} />
            </div>
          );
        })}
      </div>
    </div>
  );
}
