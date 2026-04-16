import { getPathBasename, isSameOrChildPath, replacePathPrefix } from "../utils/filePaths";

type RightPanel = "files" | "git-changes" | "git-history" | null;

type OpenFileTab = { id: string; path: string; name: string };

type OpenDiff =
  | { kind: "file"; filePath: string; staged: boolean; label: string }
  | { kind: "commit"; hash: string; message: string }
  | { kind: "commit-file"; hash: string; filePath: string; label: string };

type OpenFilesState = {
  tabs: OpenFileTab[];
  activeTabId: string | null;
};

export function renameOpenFilesState(
  state: OpenFilesState,
  currentPath: string,
  nextPath: string,
): OpenFilesState {
  let changed = false;
  const nextTabs = state.tabs.map((tab) => {
    const resolvedPath = replacePathPrefix(tab.path, currentPath, nextPath);
    if (resolvedPath === tab.path) {
      return tab;
    }

    changed = true;
    return {
      ...tab,
      path: resolvedPath,
      name: getPathBasename(resolvedPath),
    };
  });

  return changed
    ? {
        tabs: nextTabs,
        activeTabId: state.activeTabId,
      }
    : state;
}

export function deleteFromOpenFilesState(
  state: OpenFilesState,
  deletedPath: string,
): OpenFilesState {
  const removedIndexes = state.tabs
    .map((tab, index) => (isSameOrChildPath(deletedPath, tab.path) ? index : -1))
    .filter((index) => index !== -1);

  if (removedIndexes.length === 0) {
    return state;
  }

  const nextTabs = state.tabs.filter((tab) => !isSameOrChildPath(deletedPath, tab.path));
  const removedIndex = removedIndexes[0];
  const nextActiveTabId = nextTabs.some((tab) => tab.id === state.activeTabId)
    ? state.activeTabId
    : nextTabs[Math.min(removedIndex, nextTabs.length - 1)]?.id ?? null;

  return {
    tabs: nextTabs,
    activeTabId: nextActiveTabId,
  };
}

export function renameOpenDiff(openDiff: OpenDiff | null, currentPath: string, nextPath: string) {
  if (!openDiff || openDiff.kind !== "file") {
    return openDiff;
  }

  const resolvedPath = replacePathPrefix(openDiff.filePath, currentPath, nextPath);
  if (resolvedPath === openDiff.filePath) {
    return openDiff;
  }

  return {
    ...openDiff,
    filePath: resolvedPath,
    label: getPathBasename(resolvedPath),
  };
}

export function deleteOpenDiff(openDiff: OpenDiff | null, deletedPath: string) {
  if (!openDiff || openDiff.kind !== "file") {
    return openDiff;
  }

  return isSameOrChildPath(deletedPath, openDiff.filePath) ? null : openDiff;
}

export type { OpenDiff, OpenFileTab, OpenFilesState, RightPanel };
