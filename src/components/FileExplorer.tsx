import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { confirm } from "@tauri-apps/plugin-dialog";
import { RotateCcw } from "lucide-react";
import { useToast } from "./Toast";
import { FileGlyph } from "../file-icons";
import { isSystemGroupNode, type TreeNode } from "./file-explorer/tree";
import { FileExplorerContextMenu } from "./file-explorer/FileExplorerContextMenu";
import {
  FileExplorerRenameDialog,
  type RenameTarget,
} from "./file-explorer/FileExplorerRenameDialog";
import { FileExplorerTreeItem, ROW_HEIGHT } from "./file-explorer/FileExplorerTreeItem";
import { buildFileContextActionGroups } from "./file-explorer/fileContextActions";
import { useFileExplorerTree } from "./file-explorer/useFileExplorerTree";
import {
  buildSiblingPath,
  getRelativePathDisplay,
  isSameOrChildPath,
} from "../utils/filePaths";
import s from "../styles";

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
  isDark: _isDark,
  active = true,
  width = 240,
}: {
  projectPath: string;
  projectName: string;
  onFileSelect: (path: string, name: string) => void;
  onFileRename?: (currentPath: string, nextPath: string) => void;
  onFileDelete?: (deletedPath: string) => void;
  openFilePaths?: string[];
  isDark: boolean;
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
    startIndex,
    endIndex,
    selectedPath,
    refresh,
    setScrollTop,
    handleToggle,
    handleSelect,
    updateSelectedPath,
  } = useFileExplorerTree({
    projectPath,
    active,
    onFileSelect,
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

  return (
    <div style={{ ...s.fileExplorerPanel, width }}>
      <div style={s.fileExplorerHeader}>
        <span style={s.fileExplorerHeaderTitle}>Files</span>
        <button
          type="button"
          onClick={() => void refresh()}
          title="刷新文件树"
          style={s.fileExplorerRefreshButton}
          onMouseEnter={(event) => {
            event.currentTarget.style.color = "var(--text-primary)";
            event.currentTarget.style.background = "var(--bg-hover)";
          }}
          onMouseLeave={(event) => {
            event.currentTarget.style.color = "var(--text-hint)";
            event.currentTarget.style.background = "none";
          }}
        >
          <RotateCcw size={13} />
        </button>
      </div>

      <div style={s.fileExplorerProjectLabel}>
        <FileGlyph name={projectName} path={projectPath} isDir size={20} />
        {projectName}
      </div>

      <div
        ref={scrollRef}
        onScroll={(event) => setScrollTop(event.currentTarget.scrollTop)}
        style={s.fileExplorerTreeViewport}
      >
        {loading ? (
          <div style={s.fileExplorerEmptyState}>加载中...</div>
        ) : flatNodes.length === 0 ? (
          <div style={s.fileExplorerEmptyState}>空目录</div>
        ) : (
          <div
            style={{
              ...s.fileExplorerVirtualInner,
              height: flatNodes.length * ROW_HEIGHT + 12,
            }}
          >
            {flatNodes.slice(startIndex, endIndex + 1).map(({ node, depth }, index) => {
              const item = (
                <FileExplorerTreeItem
                  node={node}
                  depth={depth}
                  selectedPath={selectedPath}
                  onNodeSelect={handleSelect}
                  onNodeToggle={handleToggle}
                />
              );

              return (
                <div
                  key={node.path}
                  style={{
                    position: "absolute",
                    top: (startIndex + index) * ROW_HEIGHT + 2,
                    width: "100%",
                  }}
                >
                  {isSystemGroupNode(node) ? (
                    item
                  ) : (
                    <FileExplorerContextMenu
                      node={node}
                      relativePath={getRelativePathDisplay(projectPath, node.path)}
                      groups={buildFileContextActionGroups({
                        node,
                        relativePath: getRelativePathDisplay(projectPath, node.path),
                        onCopyPath: () => copyPath(node.path, false),
                        onCopyMentionPath: () => copyPath(node.path, true),
                        onRename: () =>
                          setRenameTarget({
                            path: node.path,
                            name: node.name,
                            isDir: node.is_dir,
                          }),
                        onDelete: () => handleDelete(node),
                      })}
                    >
                      {item}
                    </FileExplorerContextMenu>
                  )}
                </div>
              );
            })}
          </div>
        )}
      </div>

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
