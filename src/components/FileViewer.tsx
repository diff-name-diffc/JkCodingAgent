import { forwardRef, useImperativeHandle, useMemo, useRef, useState } from "react";
import * as Popover from "@radix-ui/react-popover";
import { MoreHorizontal, X } from "lucide-react";
import { FileGlyph, resolveFilePresentation } from "../file-icons";
import { FileTabPane, type FileTabPaneHandle } from "./file-viewer/FileTabPane";
import type { OpenFileTab } from "../hooks/useProjectPanels";

export interface FileViewerHandle {
  flushFile: (path: string) => Promise<string | null>;
}

export const FileViewer = forwardRef<FileViewerHandle, {
  tabs: OpenFileTab[];
  activeTabId: string | null;
  projectPath: string;
  onSelectTab: (tabId: string) => void;
  onCloseTab: (tabId: string) => void;
  onCloseOtherTabs: (tabId: string) => void;
  onCloseTabsToRight: (tabId: string) => void;
  onCloseAllTabs: () => void;
  onHide: () => void;
}>(function FileViewer({
  tabs,
  activeTabId,
  projectPath,
  onSelectTab,
  onCloseTab,
  onCloseOtherTabs,
  onCloseTabsToRight,
  onCloseAllTabs,
  onHide,
}, ref) {
  const [menuOpen, setMenuOpen] = useState(false);
  const tabRefs = useRef<Map<string, FileTabPaneHandle | null>>(new Map());

  const activeTab = useMemo(
    () => tabs.find((tab) => tab.id === activeTabId) ?? tabs[tabs.length - 1] ?? null,
    [activeTabId, tabs],
  );

  useImperativeHandle(
    ref,
    () => ({
      async flushFile(path: string) {
        const tab = tabs.find((item) => item.path === path);
        if (!tab) {
          return null;
        }
        return (await tabRefs.current.get(tab.id)?.flushPendingSave()) ?? null;
      },
    }),
    [tabs],
  );

  if (!activeTab) {
    return null;
  }

  const canCloseOtherTabs = tabs.length > 1;
  const activeTabIndex = tabs.findIndex((tab) => tab.id === activeTab.id);
  const canCloseTabsToRight = activeTabIndex !== -1 && activeTabIndex < tabs.length - 1;

  return (
    <div className="ai-file-viewer ai-migrated-file-viewer">
      <div className="ai-file-viewer-header">
        <div className="ai-file-viewer-tab-strip file-viewer-tab-strip chat-scroll">
          {tabs.map((tab) => {
            const isActive = tab.id === activeTab.id;
            const presentation = resolveFilePresentation({ name: tab.name, path: tab.path });

            return (
              <button
                key={tab.id}
                type="button"
                onClick={() => onSelectTab(tab.id)}
                title={tab.path}
                className={isActive ? "ai-file-viewer-tab is-active" : "ai-file-viewer-tab"}
              >
                <FileGlyph presentation={presentation} size={20} />
                <span className="ai-file-viewer-tab-label">
                  {tab.name}
                </span>
                <span
                  onClick={(event) => {
                    event.stopPropagation();
                    onCloseTab(tab.id);
                  }}
                  className="ai-file-viewer-tab-close"
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
              className="ai-file-viewer-tool-button"
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
          className="ai-file-viewer-tool-button"
        >
          <X size={15} />
        </button>
      </div>

      <div className="ai-file-viewer-stage">
        {tabs.map((tab) => {
          const isActive = tab.id === activeTab.id;

          return (
            <div
              key={tab.id}
              className="ai-file-viewer-pane-slot"
              style={{
                display: isActive ? "flex" : "none",
              }}
            >
              <FileTabPane
                ref={(handle) => {
                  tabRefs.current.set(tab.id, handle);
                }}
                active={isActive}
                tab={tab}
                projectPath={projectPath}
              />
            </div>
          );
        })}
      </div>
    </div>
  );
});
