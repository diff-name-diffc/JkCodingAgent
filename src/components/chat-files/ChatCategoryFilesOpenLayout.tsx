import * as React from "react";
import {
  AlertTriangle,
  Folder,
  FolderClock,
  FolderOpen,
  Loader2,
  PanelLeftOpen,
  RefreshCw,
  X,
} from "lucide-react";
import { useChatCategoriesQuery, useChatSessionsQuery } from "../../hooks/use-chat-queries";
import { useChatWorkspaceOverviewQuery } from "../../hooks/use-chat-workspace-overview";
import { isMacPlatform } from "../../lib/keyboard-bindings";
import { useChatFilesStore } from "../../stores/chat-files-store";
import { useUIStore } from "../../stores/ui-store";
import { AppLayout, type AppLayoutProps } from "../layout/app-layout";
import { Button } from "../ui/button";
import { FileExplorer } from "../FileExplorer";
import { FileViewer } from "../FileViewer";
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
import { ChatFilesDirectoryNav } from "./ChatFilesDirectoryNav";
import {
  buildChatFileRoots,
  inferHomeDir,
  pathBaseName,
  pickDefaultSessionId,
  pickInitialRoot,
  splitPathDisplay,
} from "./chat-files-model";

export interface ChatCategoryFilesOpenLayoutProps extends AppLayoutProps {
  categoryId: string;
  sessionId: string;
}

/** 树区空/加载/错误态的统一版式：居中留白 + 小图标 + 一句说明（可带操作）。 */
function FilesPlaceholder({
  icon,
  text,
  action,
}: {
  icon: React.ReactNode;
  text: string;
  action?: React.ReactNode;
}) {
  return (
    <div className="ai-chat-files-empty">
      {icon}
      <p className="ai-chat-files-empty-text">{text}</p>
      {action}
    </div>
  );
}

/** 键盘复制/粘贴提示的 Mod 键写法（macOS 用 ⌘，其余 Ctrl+）。 */
const MOD_KEY = isMacPlatform() ? "⌘" : "Ctrl+";

export function ChatCategoryFilesOpenLayout({
  categoryId,
  sessionId,
  ...layoutProps
}: ChatCategoryFilesOpenLayoutProps) {
  const closeChatFiles = useChatFilesStore((state) => state.close);
  const sidebarCollapsed = useUIStore((state) => state.sidebarCollapsed);
  const toggleSidebar = useUIStore((state) => state.toggleSidebar);
  const categoriesQuery = useChatCategoriesQuery();
  const category = categoriesQuery.data?.find((item) => item.id === categoryId) ?? null;
  const sessionsQuery = useChatSessionsQuery(categoryId, true);
  const sessions = React.useMemo(() => sessionsQuery.data ?? [], [sessionsQuery.data]);

  const [selectedSessionId, setSelectedSessionId] = React.useState<string | null>(sessionId);
  React.useEffect(() => {
    if (!sessionsQuery.data) return;
    if (selectedSessionId && sessions.some((session) => session.id === selectedSessionId)) return;
    setSelectedSessionId(pickDefaultSessionId(sessionId, sessions));
  }, [selectedSessionId, sessionId, sessions, sessionsQuery.data]);

  const overviewQuery = useChatWorkspaceOverviewQuery(categoryId, selectedSessionId);
  const roots = React.useMemo(() => buildChatFileRoots(overviewQuery.data), [overviewQuery.data]);
  const [selectedRoot, setSelectedRoot] = React.useState<string | null>(null);
  React.useEffect(() => {
    if (roots.length === 0) {
      if (selectedRoot !== null) setSelectedRoot(null);
      return;
    }
    if (selectedRoot && roots.some((root) => root.path === selectedRoot && !root.error)) return;
    setSelectedRoot(pickInitialRoot(roots));
  }, [roots, selectedRoot]);

  const [tabsState, setTabsState] = React.useState<EditorTabsState>(EMPTY_EDITOR_TABS);
  const [previewVisible, setPreviewVisible] = React.useState(false);
  const openFileTabFromTree = React.useCallback(
    (path: string, name: string) => {
      setTabsState((state) => openFileTab(state, path, name, selectedRoot ?? undefined));
      setPreviewVisible(true);
    },
    [selectedRoot],
  );

  const sessionRoot = roots.find((root) => root.kind === "session") ?? null;
  const selectedRootIsMissingSessionDir =
    sessionRoot !== null &&
    selectedRoot === sessionRoot.path &&
    !overviewQuery.data?.sessionDirExists;
  const tabs = React.useMemo(() => fileTabs(tabsState), [tabsState]);
  const openFilePaths = React.useMemo(() => tabs.map((tab) => tab.path), [tabs]);
  const showPreview = previewVisible && tabs.length > 0;
  const categoryName = category?.name ?? categoryId;

  // 前端没有 home 来源（path 插件未授权 home_dir），从会话临时目录的
  // `~/.jkcodingagent/…` 结构反推，用于把绝对路径缩成 `~/…`。
  const homeDir = React.useMemo(
    () => inferHomeDir([overviewQuery.data?.sessionDir, ...roots.map((root) => root.path)]),
    [overviewQuery.data?.sessionDir, roots],
  );
  const crumb = React.useMemo(
    () => (selectedRoot ? splitPathDisplay(selectedRoot, { homeDir, maxChars: 26 }) : null),
    [homeDir, selectedRoot],
  );

  // 唯一的刷新入口：目录信息（是否失效/是否已创建）与文件树一起刷新。
  // 树侧不再用 key 重挂载（那会把展开态清空），改为递增 refreshSignal ——
  // FileExplorer 收到信号后调自身 refresh()，它会按已展开节点递归重读，
  // 展开态与选中态都原样保留。
  const [treeReloadToken, setTreeReloadToken] = React.useState(0);
  const refetchOverview = overviewQuery.refetch;
  const refreshAll = React.useCallback(() => {
    setTreeReloadToken((token) => token + 1);
    void refetchOverview();
  }, [refetchOverview]);

  const showTree =
    !overviewQuery.isLoading &&
    !overviewQuery.isError &&
    selectedRoot !== null &&
    !selectedRootIsMissingSessionDir;

  const sidebar = sidebarCollapsed ? (
    <div className="ai-sidebar-panel ai-sidebar-rail flex h-full w-full flex-col items-center gap-2 py-3">
      <Button
        variant="ghost"
        size="icon"
        aria-label="展开文件侧栏"
        title="展开文件侧栏"
        onClick={toggleSidebar}
      >
        <PanelLeftOpen className="h-4 w-4" />
      </Button>
    </div>
  ) : (
    <div className="ai-chat-files-inline-sidebar">
      <header className="ai-chat-files-header">
        <div className="ai-chat-files-head-main">
          <FolderOpen className="ai-chat-files-head-icon" size={15} aria-hidden="true" />
          <span className="ai-chat-files-head-title">文件</span>
          <span className="ai-chat-files-head-chip" title={`分类 · ${categoryName}`}>
            {categoryName}
          </span>
        </div>
        <button
          type="button"
          className="ai-chat-files-icon-button"
          title="关闭文件侧栏"
          aria-label="关闭文件侧栏"
          onClick={closeChatFiles}
        >
          <X size={14} aria-hidden="true" />
        </button>
      </header>

      <ChatFilesDirectoryNav
        roots={roots}
        selectedRoot={selectedRoot}
        onSelectRoot={setSelectedRoot}
        sessions={sessions}
        selectedSessionId={selectedSessionId}
        onSelectSession={setSelectedSessionId}
        sessionDirExists={overviewQuery.data?.sessionDirExists ?? false}
        loading={overviewQuery.isLoading}
        homeDir={homeDir}
      />

      <div className="ai-chat-files-tree">
        <div className="ai-chat-files-tree-head" title={selectedRoot ?? undefined}>
          <Folder className="ai-chat-files-tree-head-icon" size={13} aria-hidden="true" />
          {/* 当前根只以「路径条」出现（父目录 muted + 末段稍强）：名称在位置列表
              已有强调，这里再放一个标题会变成紧邻的重复信息。 */}
          <span className="ai-chat-files-tree-crumb">
            {crumb ? (
              <>
                {crumb.parent && (
                  <span className="ai-chat-files-tree-crumb-path">{crumb.parent}/</span>
                )}
                <span className="ai-chat-files-tree-crumb-name">{crumb.name}</span>
              </>
            ) : (
              <span className="ai-chat-files-tree-crumb-name">未选择位置</span>
            )}
          </span>
          <button
            type="button"
            className="ai-chat-files-icon-button ai-chat-files-tree-refresh"
            title="刷新目录与文件树"
            aria-label="刷新目录与文件树"
            onClick={refreshAll}
          >
            <RefreshCw
              size={13}
              className={overviewQuery.isFetching ? "animate-spin" : undefined}
              aria-hidden="true"
            />
          </button>
        </div>

        <div className="ai-chat-files-tree-body">
          {overviewQuery.isLoading ? (
            <FilesPlaceholder
              icon={
                <Loader2
                  className="ai-chat-files-empty-icon animate-spin"
                  size={18}
                  aria-hidden="true"
                />
              }
              text="正在读取目录…"
            />
          ) : overviewQuery.isError ? (
            <FilesPlaceholder
              icon={
                <AlertTriangle
                  className="ai-chat-files-empty-icon is-danger"
                  size={18}
                  aria-hidden="true"
                />
              }
              text={`目录读取失败：${String(overviewQuery.error)}`}
              action={
                <button type="button" className="ai-chat-files-empty-action" onClick={refreshAll}>
                  重新读取
                </button>
              }
            />
          ) : !selectedRoot ? (
            <FilesPlaceholder
              icon={
                <FolderOpen className="ai-chat-files-empty-icon" size={18} aria-hidden="true" />
              }
              text="暂无可用位置；绑定共享工程目录后即可浏览文件。"
            />
          ) : selectedRootIsMissingSessionDir ? (
            <FilesPlaceholder
              icon={
                <FolderClock className="ai-chat-files-empty-icon" size={18} aria-hidden="true" />
              }
              text="会话临时目录尚未创建；发送首条消息后会随运行自动生成。"
            />
          ) : (
            <FileExplorer
              key={selectedRoot}
              refreshSignal={treeReloadToken}
              projectPath={selectedRoot}
              showToolbar={false}
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

        {showTree && (
          <div
            className="ai-chat-files-hintbar"
            title="选中文件后复制，可直接粘贴到访达或其它应用；粘贴会把剪贴板文件复制进选中目录"
          >
            <span>{MOD_KEY}C 复制</span>
            <span className="ai-chat-files-hintbar-sep" aria-hidden="true">
              ·
            </span>
            <span>{MOD_KEY}V 粘贴</span>
          </div>
        )}
      </div>
    </div>
  );

  return (
    <AppLayout
      {...layoutProps}
      sidebar={sidebar}
      chatHeader={showPreview ? undefined : layoutProps.chatHeader}
      chatFooter={showPreview ? undefined : layoutProps.chatFooter}
      artifactPanel={showPreview ? undefined : layoutProps.artifactPanel}
    >
      {!showPreview && layoutProps.children}
      {tabs.length > 0 && (
        <section
          key="chat-file-viewer"
          className="ai-chat-files-inline-viewer"
          style={{ display: showPreview ? "flex" : "none" }}
          aria-label="分类文件预览与编辑"
        >
          <FileViewer
            tabs={tabs}
            activeTabId={tabsState.activeTabId}
            projectPath={selectedRoot ?? ""}
            onSelectTab={(tabId) => setTabsState((state) => selectTab(state, tabId))}
            onCloseTab={(tabId) => setTabsState((state) => closeTab(state, tabId))}
            onCloseOtherTabs={(tabId) => setTabsState((state) => closeOtherTabs(state, tabId))}
            onCloseTabsToRight={(tabId) => setTabsState((state) => closeTabsToRight(state, tabId))}
            onCloseAllTabs={() => setTabsState(closeAllTabs())}
            onHide={() => setPreviewVisible(false)}
            hideLabel="关闭文件预览并返回会话"
          />
        </section>
      )}
    </AppLayout>
  );
}
