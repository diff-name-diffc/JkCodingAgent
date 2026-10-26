/**
 * 普通聊天「分类文件」面板：区分展示分类共享工程目录与会话临时目录——
 * 左栏目录分区导航 + 当前根的文件树（FileExplorer），右栏多标签查看/编辑
 * （FileViewer：Monaco 编辑、防抖自动保存、大文件/图片只读）。
 * 两侧目录均由 chat_workspace_overview 给出，作为授权根复用项目页文件命令，
 * 本面板不新增读写通道。
 */

import * as React from "react";
import * as DialogPrimitive from "@radix-ui/react-dialog";
import { FolderOpen, RefreshCw } from "lucide-react";
import { useChatCategoriesQuery, useChatSessionsQuery } from "../../hooks/use-chat-queries";
import { useChatWorkspaceOverviewQuery } from "../../hooks/use-chat-workspace-overview";
import {
  closeAllTabs,
  closeOtherTabs,
  closeTab,
  closeTabsToRight,
  deleteFileTab,
  EMPTY_EDITOR_TABS,
  fileTabs,
  openFileTab,
  renameFileTab,
  selectTab,
  type EditorTabsState,
} from "../project/main-tabs";
import { FileExplorer } from "../FileExplorer";
import { FileViewer } from "../FileViewer";
import { ChatFilesDirectoryNav } from "./ChatFilesDirectoryNav";
import {
  buildChatFileRoots,
  pathBaseName,
  pickDefaultSessionId,
  pickInitialRoot,
} from "./chat-files-model";

export interface ChatCategoryFilesDialogProps {
  categoryId: string;
  /** 当前活跃会话（属于该分类时作为临时目录缺省选择）。 */
  activeSessionId: string | null;
  onClose: () => void;
}

export function ChatCategoryFilesDialog({
  categoryId,
  activeSessionId,
  onClose,
}: ChatCategoryFilesDialogProps) {
  const categoriesQuery = useChatCategoriesQuery();
  const category = categoriesQuery.data?.find((item) => item.id === categoryId) ?? null;
  // 会话下拉数据面：单页 100 条（按更新时间倒序），覆盖绝大多数分类；
  // 与侧栏的懒加载分页 hook 互不影响。
  const sessionsQuery = useChatSessionsQuery(categoryId, true);
  const sessions = React.useMemo(() => sessionsQuery.data ?? [], [sessionsQuery.data]);

  // 临时目录的会话选择：活跃会话属于该分类时缺省选中，否则取首个；失效自动纠正。
  const [selectedSessionId, setSelectedSessionId] = React.useState<string | null>(null);
  React.useEffect(() => {
    const ids = new Set(sessions.map((session) => session.id));
    if ((selectedSessionId && ids.has(selectedSessionId)) || ids.size === 0) {
      return;
    }
    setSelectedSessionId(pickDefaultSessionId(activeSessionId, sessions));
  }, [activeSessionId, selectedSessionId, sessions]);

  const overviewQuery = useChatWorkspaceOverviewQuery(categoryId, selectedSessionId);
  const roots = React.useMemo(() => buildChatFileRoots(overviewQuery.data), [overviewQuery.data]);

  // 当前浏览根：失效/消失的根自动回退到默认根。
  const [selectedRoot, setSelectedRoot] = React.useState<string | null>(null);
  React.useEffect(() => {
    if (roots.length === 0) {
      if (selectedRoot !== null) setSelectedRoot(null);
      return;
    }
    if (selectedRoot && roots.some((root) => root.path === selectedRoot && !root.error)) {
      return;
    }
    setSelectedRoot(pickInitialRoot(roots));
  }, [roots, selectedRoot]);

  const [tabsState, setTabsState] = React.useState<EditorTabsState>(EMPTY_EDITOR_TABS);
  const openFileTabFromTree = React.useCallback(
    (path: string, name: string) => {
      setTabsState((state) => openFileTab(state, path, name, selectedRoot ?? undefined));
    },
    [selectedRoot],
  );

  const sessionRoot = roots.find((root) => root.kind === "session") ?? null;
  const selectedRootIsMissingSessionDir =
    sessionRoot !== null &&
    selectedRoot === sessionRoot.path &&
    !overviewQuery.data?.sessionDirExists;
  const openFilePaths = React.useMemo(
    () => fileTabs(tabsState).map((tab) => tab.path),
    [tabsState],
  );
  const tabs = React.useMemo(() => fileTabs(tabsState), [tabsState]);

  const categoryName = category?.name ?? categoryId;

  return (
    <DialogPrimitive.Root
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <DialogPrimitive.Portal>
        <DialogPrimitive.Overlay className="ai-dialog-overlay ai-chat-files-overlay" />
        <DialogPrimitive.Content className="ai-chat-files-shell" aria-describedby={undefined}>
          <DialogPrimitive.Title className="sr-only">
            分类文件 · {categoryName}
          </DialogPrimitive.Title>

          <header className="ai-chat-files-header">
            <div className="ai-chat-files-title">
              <FolderOpen size={16} strokeWidth={1.6} aria-hidden="true" />
              <span>分类文件</span>
              <span className="ai-chat-files-category">{categoryName}</span>
            </div>
            <div className="ai-chat-files-actions">
              <button
                type="button"
                className="ai-chat-files-action"
                title="刷新目录信息"
                aria-label="刷新目录信息"
                onClick={() => void overviewQuery.refetch()}
              >
                <RefreshCw size={14} aria-hidden="true" />
              </button>
              <DialogPrimitive.Close className="ai-chat-files-action" aria-label="关闭">
                ✕
              </DialogPrimitive.Close>
            </div>
          </header>

          <div className="ai-chat-files-body">
            <aside className="ai-chat-files-side">
              <ChatFilesDirectoryNav
                roots={roots}
                selectedRoot={selectedRoot}
                onSelectRoot={setSelectedRoot}
                sessions={sessions}
                selectedSessionId={selectedSessionId}
                onSelectSession={setSelectedSessionId}
                sessionDirExists={overviewQuery.data?.sessionDirExists ?? false}
              />
              <div className="ai-chat-files-tree">
                {overviewQuery.isLoading ? (
                  <p className="ai-chat-files-hint">正在读取目录…</p>
                ) : !selectedRoot ? (
                  <p className="ai-chat-files-hint">暂无可浏览的目录。</p>
                ) : selectedRootIsMissingSessionDir ? (
                  <p className="ai-chat-files-hint">
                    会话临时目录尚未创建；发送首条消息后会随运行自动生成。
                  </p>
                ) : (
                  <FileExplorer
                    key={selectedRoot}
                    projectPath={selectedRoot}
                    onFileSelect={openFileTabFromTree}
                    onFileRename={(currentPath, nextPath) =>
                      setTabsState((state) =>
                        renameFileTab(state, currentPath, nextPath, pathBaseName(nextPath)),
                      )
                    }
                    onFileDelete={(deletedPath) =>
                      setTabsState((state) => deleteFileTab(state, deletedPath))
                    }
                    openFilePaths={openFilePaths}
                  />
                )}
              </div>
            </aside>

            <section className="ai-chat-files-viewer">
              {tabs.length > 0 ? (
                <FileViewer
                  tabs={tabs}
                  activeTabId={tabsState.activeTabId}
                  projectPath={selectedRoot ?? ""}
                  onSelectTab={(tabId) => setTabsState((state) => selectTab(state, tabId))}
                  onCloseTab={(tabId) => setTabsState((state) => closeTab(state, tabId))}
                  onCloseOtherTabs={(tabId) =>
                    setTabsState((state) => closeOtherTabs(state, tabId))
                  }
                  onCloseTabsToRight={(tabId) =>
                    setTabsState((state) => closeTabsToRight(state, tabId))
                  }
                  onCloseAllTabs={() => setTabsState(closeAllTabs())}
                  onHide={() => setTabsState(closeAllTabs())}
                />
              ) : (
                <div className="ai-chat-files-empty">
                  <FolderOpen size={26} strokeWidth={1.4} aria-hidden="true" />
                  <p>在左侧选择文件即可查看与编辑；共享工程目录存正式产物，</p>
                  <p>会话临时目录存放脚本、下载与中间产物。</p>
                </div>
              )}
            </section>
          </div>
        </DialogPrimitive.Content>
      </DialogPrimitive.Portal>
    </DialogPrimitive.Root>
  );
}
