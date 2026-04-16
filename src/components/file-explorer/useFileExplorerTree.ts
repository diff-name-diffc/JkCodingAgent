import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useCancellableInvoke } from "../../hooks/useCancellableInvoke";
import { isSameOrChildPath, replacePathPrefix } from "../../utils/filePaths";
import {
  findNode,
  flattenVisible,
  loadTreeNodes,
  updateNode,
  type FsEntry,
  type TreeNode,
} from "./tree";
import { ROW_HEIGHT } from "./FileExplorerTreeItem";

const AUTO_REFRESH_MS = 2500;
const OVERSCAN = 5;

export function useFileExplorerTree({
  projectPath,
  active,
  onFileSelect,
}: {
  projectPath: string;
  active: boolean;
  onFileSelect: (path: string, name: string) => void;
}) {
  const [nodes, setNodes] = useState<TreeNode[]>([]);
  const [selectedPath, setSelectedPath] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewportHeight, setViewportHeight] = useState(500);
  const scrollRef = useRef<HTMLDivElement>(null);
  const nodesRef = useRef<TreeNode[]>([]);
  const refreshIdRef = useRef(0);
  const { safeInvoke, isCancelled } = useCancellableInvoke();

  useEffect(() => {
    nodesRef.current = nodes;
  }, [nodes]);

  const readEntries = useCallback(
    (path: string) => safeInvoke<FsEntry[]>("read_dir_entries", { path, projectPath }),
    [projectPath, safeInvoke],
  );

  const refresh = useCallback(
    async (showLoading = false) => {
      const refreshId = refreshIdRef.current + 1;
      refreshIdRef.current = refreshId;

      if (showLoading) {
        setLoading(true);
      }

      try {
        const nextNodes = await loadTreeNodes({
          path: projectPath,
          rootPath: projectPath,
          previousNodes: nodesRef.current,
          readEntries,
        });
        if (nextNodes === null || refreshId !== refreshIdRef.current) {
          return;
        }

        if (nextNodes !== nodesRef.current) {
          setNodes(nextNodes);
        }

        setLoading(false);
      } catch {
        if (!isCancelled() && refreshId === refreshIdRef.current) {
          setLoading(false);
        }
      }
    },
    [isCancelled, projectPath, readEntries],
  );

  useEffect(() => {
    if (!active) {
      return;
    }

    void refresh(true);
  }, [active, projectPath, refresh]);

  useEffect(() => {
    if (!active) {
      return;
    }

    const handleVisibilityRefresh = () => {
      if (document.visibilityState !== "visible") {
        return;
      }

      void refresh();
    };

    const timer = window.setInterval(() => {
      if (document.visibilityState !== "visible") {
        return;
      }

      void refresh();
    }, AUTO_REFRESH_MS);

    window.addEventListener("focus", handleVisibilityRefresh);
    document.addEventListener("visibilitychange", handleVisibilityRefresh);

    return () => {
      window.clearInterval(timer);
      window.removeEventListener("focus", handleVisibilityRefresh);
      document.removeEventListener("visibilitychange", handleVisibilityRefresh);
    };
  }, [active, refresh]);

  useEffect(() => {
    const element = scrollRef.current;
    if (!element) {
      return;
    }

    setViewportHeight(element.clientHeight);
    const observer = new ResizeObserver(() => setViewportHeight(element.clientHeight));
    observer.observe(element);

    return () => observer.disconnect();
  }, []);

  const flatNodes = useMemo(() => flattenVisible(nodes), [nodes]);
  const startIndex = Math.max(0, Math.floor(scrollTop / ROW_HEIGHT) - OVERSCAN);
  const endIndex = Math.min(
    flatNodes.length - 1,
    Math.ceil((scrollTop + viewportHeight) / ROW_HEIGHT) + OVERSCAN,
  );

  const handleToggle = useCallback(
    (dirPath: string) => {
      const currentNode = findNode(nodesRef.current, dirPath);
      const shouldExpand = !currentNode?.expanded;

      setNodes((previousNodes) =>
        updateNode(previousNodes, dirPath, (node) => {
          const nextChildren = shouldExpand ? (node.children ?? []) : node.children;
          if (node.expanded === shouldExpand && node.children === nextChildren) {
            return node;
          }
          return { ...node, expanded: shouldExpand, children: nextChildren };
        }),
      );

      if (!shouldExpand) {
        return;
      }

      void (async () => {
        const currentChildren = findNode(nodesRef.current, dirPath)?.children ?? [];
        const nextChildren = await loadTreeNodes({
          path: dirPath,
          rootPath: projectPath,
          previousNodes: currentChildren,
          readEntries,
        });

        if (nextChildren === null) {
          return;
        }

        setNodes((previousNodes) =>
          updateNode(previousNodes, dirPath, (node) =>
            node.children === nextChildren ? node : { ...node, children: nextChildren },
          ),
        );
      })();
    },
    [projectPath, readEntries],
  );

  const handleSelect = useCallback(
    (node: TreeNode) => {
      setSelectedPath(node.path);
      onFileSelect(node.path, node.name);
    },
    [onFileSelect],
  );

  const updateSelectedPath = useCallback((currentPath: string, nextPath: string | null) => {
    setSelectedPath((previousPath) => {
      if (!previousPath || !isSameOrChildPath(currentPath, previousPath)) {
        return previousPath;
      }

      return nextPath ? replacePathPrefix(previousPath, currentPath, nextPath) : null;
    });
  }, []);

  return {
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
  };
}
