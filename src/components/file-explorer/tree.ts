export interface FsEntry {
  name: string;
  path: string;
  is_dir: boolean;
  extension?: string;
}

export interface TreeNode extends FsEntry {
  children: TreeNode[] | null;
  expanded: boolean;
  kind?: "entry" | "system-group";
  iconName?: string;
}

export const SYSTEM_CONFIG_GROUP_LABEL = "系统配置";
export const SYSTEM_CONFIG_GROUP_ICON_NAME = ".config";

const SYSTEM_CONFIG_GROUP_PATH_PREFIX = "__nezha_system_config__:";

function isSameEntry(a: FsEntry, b: FsEntry) {
  return (
    a.path === b.path &&
    a.name === b.name &&
    a.is_dir === b.is_dir &&
    a.extension === b.extension
  );
}

function isRootSystemConfigDir(entry: FsEntry) {
  return entry.is_dir && entry.name.startsWith(".");
}

function createSystemConfigGroupPath(rootPath: string) {
  return `${SYSTEM_CONFIG_GROUP_PATH_PREFIX}${rootPath}`;
}

function hasSameNodeRefs(a: TreeNode[], b: TreeNode[]) {
  return a.length === b.length && a.every((node, index) => node === b[index]);
}

function unwrapRootNodes(nodes: TreeNode[]): TreeNode[] {
  const result: TreeNode[] = [];
  for (const node of nodes) {
    if (node.kind === "system-group") {
      result.push(...(node.children ?? []));
      continue;
    }
    result.push(node);
  }
  return result;
}

function groupRootNodes(rootPath: string, nextNodes: TreeNode[], previousNodes: TreeNode[]): TreeNode[] {
  const systemNodes = nextNodes.filter(isRootSystemConfigDir);
  if (systemNodes.length === 0) {
    return hasSameNodeRefs(nextNodes, previousNodes) ? previousNodes : nextNodes;
  }

  const regularNodes = nextNodes.filter((node) => !isRootSystemConfigDir(node));
  const previousGroup = previousNodes.find((node) => node.kind === "system-group");
  const nextGroupChildren =
    previousGroup?.children && hasSameNodeRefs(systemNodes, previousGroup.children)
      ? previousGroup.children
      : systemNodes;
  const nextGroupExpanded = previousGroup?.expanded ?? false;
  const nextGroup =
    previousGroup &&
    previousGroup.children === nextGroupChildren &&
    previousGroup.expanded === nextGroupExpanded
      ? previousGroup
      : {
          name: SYSTEM_CONFIG_GROUP_LABEL,
          path: createSystemConfigGroupPath(rootPath),
          is_dir: true,
          extension: undefined,
          children: nextGroupChildren,
          expanded: nextGroupExpanded,
          kind: "system-group" as const,
          iconName: SYSTEM_CONFIG_GROUP_ICON_NAME,
        };

  const groupedNodes = [...regularNodes, nextGroup];
  return hasSameNodeRefs(groupedNodes, previousNodes) ? previousNodes : groupedNodes;
}

export function isSystemGroupNode(node: TreeNode) {
  return node.kind === "system-group";
}

export function flattenVisible(nodes: TreeNode[]): Array<{ node: TreeNode; depth: number }> {
  const result: Array<{ node: TreeNode; depth: number }> = [];

  function walk(items: TreeNode[], depth: number) {
    for (const node of items) {
      result.push({ node, depth });
      if (node.is_dir && node.expanded && node.children) {
        walk(node.children, depth + 1);
      }
    }
  }

  walk(nodes, 0);
  return result;
}

export function findNode(items: TreeNode[], path: string): TreeNode | null {
  for (const item of items) {
    if (item.path === path) return item;
    if (item.children) {
      const found = findNode(item.children, path);
      if (found) return found;
    }
  }
  return null;
}

export function updateNode(
  items: TreeNode[],
  path: string,
  updater: (node: TreeNode) => TreeNode,
): TreeNode[] {
  let changed = false;
  const nextItems = items.map((item) => {
    if (item.path === path) {
      const nextItem = updater(item);
      if (nextItem !== item) changed = true;
      return nextItem;
    }

    if (!item.children) return item;

    const nextChildren = updateNode(item.children, path, updater);
    if (nextChildren === item.children) return item;

    changed = true;
    return { ...item, children: nextChildren };
  });

  return changed ? nextItems : items;
}

export async function loadTreeNodes({
  path,
  rootPath,
  previousNodes,
  readEntries,
}: {
  path: string;
  rootPath: string;
  previousNodes: TreeNode[];
  readEntries: (path: string) => Promise<FsEntry[] | null>;
}): Promise<TreeNode[] | null> {
  const entries = await readEntries(path);
  if (entries === null) return null;

  const comparablePreviousNodes = path === rootPath ? unwrapRootNodes(previousNodes) : previousNodes;
  const previousByPath = new Map(comparablePreviousNodes.map((node) => [node.path, node]));
  let changed = entries.length !== comparablePreviousNodes.length;
  const nextNodes: TreeNode[] = [];

  for (const [index, entry] of entries.entries()) {
    const previous = previousByPath.get(entry.path);
    const expanded = previous?.expanded ?? false;
    let children: TreeNode[] | null = null;

    if (entry.is_dir) {
      if (expanded) {
        const nextChildren = await loadTreeNodes({
          path: entry.path,
          rootPath,
          previousNodes: previous?.children ?? [],
          readEntries,
        });
        if (nextChildren === null) return null;
        children = nextChildren;
      } else {
        children = previous?.children ?? null;
      }
    }

    const previousAtIndex = comparablePreviousNodes[index];
    if (!previousAtIndex || previousAtIndex.path !== entry.path) {
      changed = true;
    }

    if (
      previous &&
      isSameEntry(previous, entry) &&
      previous.children === children &&
      previous.kind === "entry"
    ) {
      nextNodes.push(previous);
      continue;
    }

    changed = true;
    nextNodes.push({
      ...entry,
      children,
      expanded,
      kind: "entry",
    });
  }

  if (path === rootPath) {
    const resolvedNodes = changed ? nextNodes : comparablePreviousNodes;
    return groupRootNodes(rootPath, resolvedNodes, previousNodes);
  }

  return changed ? nextNodes : previousNodes;
}
