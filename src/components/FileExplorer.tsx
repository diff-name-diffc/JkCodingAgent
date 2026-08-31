import { useState, useEffect, useCallback, useMemo } from "react";
import type { MouseEvent as ReactMouseEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { confirm } from "@tauri-apps/plugin-dialog";
import { useVirtualizer } from "@tanstack/react-virtual";
import { RotateCcw } from "lucide-react";
import { useToast } from "./Toast";
import { FileGlyph } from "../file-icons";
import { isSystemGroupNode, type TreeNode } from "./file-explorer/tree";
import {
  FileExplorerRenameDialog,
  type RenameTarget,
} from "./file-explorer/FileExplorerRenameDialog";
import { FileExplorerTreeItem, ROW_HEIGHT } from "./file-explorer/FileExplorerTreeItem";
import { FileExplorerContextMenu } from "./file-explorer/FileExplorerContextMenu";
import { buildFileContextActionGroups } from "./file-explorer/fileContextActions";
import { useFileExplorerTree } from "./file-explorer/useFileExplorerTree";
import {
  buildSiblingPath,
  getRelativePathDisplay,
  isSameOrChildPath,
} from "../utils/filePaths";

function resolveErrorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

export function FileExplorer({
  projectPath,
  projectName,
  onFileSelect,
  onFileRename,
  onFileDelete,
  openFilePaths = [],
  active = true,
  width = 240,
}: {
  projectPath: string;
  projectName: string;
  onFileSelect: (path: string, name: string) => void;
  onFileRename?: (currentPath: string, nextPath: string) => void;
  onFileDelete?: (deletedPath: string) => void;
  openFilePaths?: string[];
  active?: boolean;
  width?: number;
}) {
  const { showToast } = useToast();
  const [renameTarget, setRenameTarget] = useState<RenameTarget | null>(null);
  const [renameSaving, setRenameSaving] = useState(false);
  const {
    scrollRef,
    loading,
    flatNodes,
    selectedPath,
    refresh,
    handleToggle,
    handleSelect,
    updateSelectedPath,
  } = useFileExplorerTree({
    projectPath,
    active,
    onFileSelect,
  });

  const virtualizer = useVirtualizer({
    count: flatNodes.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 6,
  });

  useEffect(() => {
    setRenameTarget(null);
  }, [projectPath]);

  const isPathOpenInEditor = useCallback(
    (path: string) => openFilePaths.some((openPath) => isSameOrChildPath(path, openPath)),
    [openFilePaths],
  );

  const confirmOpenFileMutation = useCallback(
    async (path: string, actionLabel: "重命名" | "删除") => {
      if (!isPathOpenInEditor(path)) {
        return true;
      }

      return confirm(
        `该项当前已在编辑器中打开，继续${actionLabel}会刷新相关标签页，未保存的内容可能丢失。确定继续吗？`,
        {
          title: `${actionLabel}已打开项`,
          kind: "warning",
        },
      );
    },
    [isPathOpenInEditor],
  );

  const copyPath = useCallback(
    async (path: string, withMentionPrefix: boolean) => {
      try {
        await navigator.clipboard.writeText(withMentionPrefix ? `@${path}` : path);
      } catch (error) {
        console.error("复制路径失败:", error);
        showToast(`复制路径失败：${resolveErrorMessage(error)}`);
      }
    },
    [showToast],
  );

  const handleDelete = useCallback(
    async (node: TreeNode) => {
      const entryLabel = node.is_dir ? "目录" : "文件";
      const extraWarning = node.is_dir ? "\n目录下的内容也会被一并删除。" : "";
      const allowMutation = await confirmOpenFileMutation(node.path, "删除");
      if (!allowMutation) {
        return;
      }

      const confirmed = await confirm(
        `确定永久删除${entryLabel}“${node.name}”吗？${extraWarning}`,
        {
          title: `删除${entryLabel}`,
          kind: "warning",
        },
      );
      if (!confirmed) {
        return;
      }

      try {
        await invoke("delete_fs_entry", { path: node.path, projectPath });
        updateSelectedPath(node.path, null);
        onFileDelete?.(node.path);
        await refresh();
      } catch (error) {
        console.error("删除文件项失败:", error);
        showToast(`删除失败：${resolveErrorMessage(error)}`);
      }
    },
    [confirmOpenFileMutation, onFileDelete, projectPath, refresh, showToast, updateSelectedPath],
  );

  const handleRename = useCallback(
    async (nextName: string) => {
      if (!renameTarget) {
        return;
      }

      const nextPath = buildSiblingPath(renameTarget.path, nextName);
      if (nextPath === renameTarget.path) {
        setRenameTarget(null);
        return;
      }

      const allowMutation = await confirmOpenFileMutation(renameTarget.path, "重命名");
      if (!allowMutation) {
        return;
      }

      setRenameSaving(true);
      try {
        await invoke("move_fs_entry", {
          sourcePath: renameTarget.path,
          destinationPath: nextPath,
          projectPath,
        });
        updateSelectedPath(renameTarget.path, nextPath);
        onFileRename?.(renameTarget.path, nextPath);
        setRenameTarget(null);
        await refresh();
      } catch (error) {
        console.error("重命名文件项失败:", error);
        showToast(`重命名失败：${resolveErrorMessage(error)}`);
      } finally {
        setRenameSaving(false);
      }
    },
    [
      confirmOpenFileMutation,
      onFileRename,
      projectPath,
      refresh,
      renameTarget,
      showToast,
      updateSelectedPath,
    ],
  );

  const handleCopyPath = useCallback(
    (node: TreeNode) => {
      void copyPath(node.path, false);
    },
    [copyPath],
  );

  const handleCopyMentionPath = useCallback(
    (node: TreeNode) => {
      void copyPath(node.path, true);
    },
    [copyPath],
  );

  const handleRenameRequest = useCallback((node: TreeNode) => {
    setRenameTarget({ path: node.path, name: node.name, isDir: node.is_dir });
  }, []);

  const [menuTarget, setMenuTarget] = useState<TreeNode | null>(null);
  const [menuOpen, setMenuOpen] = useState(false);

  // 菜单关闭即清空目标节点：避免过期节点对象残留（删除/重命名触发 refresh
  // 后，旧引用会指向已不存在的树节点），保证「菜单关闭后无目标残留」的闭环。
  const handleMenuOpenChange = useCallback((nextOpen: boolean) => {
    setMenuOpen(nextOpen);
    if (!nextOpen) setMenuTarget(null);
  }, []);

  const handleTreeContextMenu = useCallback(
    (event: ReactMouseEvent<HTMLDivElement>) => {
      const rowEl = (event.target as HTMLElement).closest("[data-row-path]");
      const path = rowEl?.getAttribute("data-row-path");
      const node = path ? flatNodes.find((item) => item.node.path === path)?.node : undefined;
      if (!node || isSystemGroupNode(node)) {
        // preventDefault 同时是阻止 Radix 打开菜单的手段：Trigger asChild 把本
        // 处理器与 Radix 内部 onContextMenu 合并到同一节点，composeEventHandlers
        // 在 defaultPrevented 时跳过打开逻辑。升级 @radix-ui/react-context-menu
        // 时需确认该检查仍存在，否则系统节点/空白处会意外打开菜单。
        event.preventDefault();
        return;
      }
      // 同一事件内同步设置 target 与 open：「打开」不再依赖 Radix 内部
      // onContextMenu 回调触发 onOpenChange(true)——该回调的执行顺序属于
      // composeEventHandlers 的内部约定，Radix 升级可能变化；显式设置后
      // 即使内部回调顺序改变或缺失，菜单也能带正确目标打开。
      setMenuTarget(node);
      setMenuOpen(true);
    },
    [flatNodes],
  );

  const menuRelativePath = useMemo(
    () => (menuTarget ? getRelativePathDisplay(projectPath, menuTarget.path) : ""),
    [projectPath, menuTarget],
  );

  const menuGroups = useMemo(() => {
    if (!menuTarget) {
      return [];
    }
    return buildFileContextActionGroups({
      node: menuTarget,
      relativePath: menuRelativePath,
      onCopyPath: () => handleCopyPath(menuTarget),
      onCopyMentionPath: () => handleCopyMentionPath(menuTarget),
      onRename: () => handleRenameRequest(menuTarget),
      onDelete: () => handleDelete(menuTarget),
    });
  }, [
    menuTarget,
    menuRelativePath,
    handleCopyPath,
    handleCopyMentionPath,
    handleRenameRequest,
    handleDelete,
  ]);

  return (
    <div className="ai-file-explorer" style={{ width }}>
      <div className="ai-file-explorer-header">
        <span className="ai-file-explorer-title">Files</span>
        <button
          type="button"
          onClick={() => void refresh()}
          title="刷新文件树"
          className="ai-file-explorer-refresh"
        >
          <RotateCcw size={13} />
        </button>
      </div>

      <div className="ai-file-explorer-project">
        <FileGlyph name={projectName} path={projectPath} isDir size={20} />
        <span>{projectName}</span>
      </div>

      <FileExplorerContextMenu
        open={menuOpen}
        onOpenChange={handleMenuOpenChange}
        node={menuTarget}
        relativePath={menuRelativePath}
        groups={menuGroups}
      >
        <div
          ref={scrollRef}
          onContextMenu={handleTreeContextMenu}
          className="ai-file-explorer-tree chat-scroll"
        >
          {loading ? (
            <div className="ai-file-explorer-empty">加载中...</div>
          ) : flatNodes.length === 0 ? (
            <div className="ai-file-explorer-empty">空目录</div>
          ) : (
            <div
              className="ai-file-explorer-virtual"
              style={{ height: virtualizer.getTotalSize() }}
            >
              {virtualizer.getVirtualItems().map((virtualRow) => {
                const { node, depth } = flatNodes[virtualRow.index];

                return (
                  <div
                    key={node.path}
                    data-row-path={node.path}
                    className="ai-file-explorer-virtual-row"
                    style={{ transform: `translateY(${virtualRow.start}px)` }}
                  >
                    <FileExplorerTreeItem
                      node={node}
                      depth={depth}
                      selected={node.path === selectedPath}
                      onNodeSelect={handleSelect}
                      onNodeToggle={handleToggle}
                    />
                  </div>
                );
              })}
            </div>
          )}
        </div>
      </FileExplorerContextMenu>

      <FileExplorerRenameDialog
        projectPath={projectPath}
        target={renameTarget}
        saving={renameSaving}
        onClose={() => {
          if (!renameSaving) {
            setRenameTarget(null);
          }
        }}
        onSubmit={handleRename}
      />
    </div>
  );
}
