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

const SYSTEM_CONFIG_GROUP_LABEL = "系统配置";
const SYSTEM_CONFIG_GROUP_ICON_NAME = ".config";

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

/**
 * 目录读取并发上限：一次刷新会递归并行读取整棵已展开子树，大型项目
 * （展开的 node_modules、monorepo）可能瞬间扇出数百个 read_dir_entries
 * IPC。限制器在整个递归刷新间共享，全局生效；取 8 保留并行收益的同时
 * 避免请求洪峰压垮后端。
 */
const REFRESH_READ_CONCURRENCY = 8;

type ConcurrencyLimiter = <T>(task: () => Promise<T>) => Promise<T>;

/** 简单信号量：活跃任务达到上限时排队等待，任务完成把名额让给队首。 */
function createConcurrencyLimiter(max: number): ConcurrencyLimiter {
  let active = 0;
  const queue: Array<() => void> = [];

  function release() {
    const resume = queue.shift();
    if (resume) {
      resume(); // 名额直接移交给排队任务，active 不变
    } else {
      active -= 1;
    }
  }

  return async function run<T>(task: () => Promise<T>): Promise<T> {
    if (active >= max) {
      await new Promise<void>((resolve) => queue.push(resolve));
    } else {
      active += 1;
    }
    try {
      return await task();
    } finally {
      release();
    }
  };
}

export async function loadTreeNodes({
  path,
  rootPath,
  previousNodes,
  readEntries,
  limiter,
}: {
  path: string;
  rootPath: string;
  previousNodes: TreeNode[];
  readEntries: (path: string) => Promise<FsEntry[] | null>;
  /** 本次刷新共享的并发限制器；顶层调用缺省时自动创建。 */
  limiter?: ConcurrencyLimiter;
}): Promise<TreeNode[] | null> {
  const run = limiter ?? createConcurrencyLimiter(REFRESH_READ_CONCURRENCY);
  const entries = await run(() => readEntries(path));
  if (entries === null) return null;

  const comparablePreviousNodes = path === rootPath ? unwrapRootNodes(previousNodes) : previousNodes;
  const previousByPath = new Map(comparablePreviousNodes.map((node) => [node.path, node]));
  let changed = entries.length !== comparablePreviousNodes.length;

  // 已展开子目录并行读取，避免逐目录串行 IPC 的延迟叠加；
  // 并发数由共享限制器封顶，防止大树扇出海量 IPC。
  const expandedDirs = entries.filter(
    (entry) => entry.is_dir && previousByPath.get(entry.path)?.expanded,
  );
  const childResults = await Promise.all(
    expandedDirs.map((entry) =>
      loadTreeNodes({
        path: entry.path,
        rootPath,
        previousNodes: previousByPath.get(entry.path)?.children ?? [],
        readEntries,
        limiter: run,
      }),
    ),
  );
  if (!childResults.every((result): result is TreeNode[] => result !== null)) return null;
  const resolvedChildren = childResults;
  const childrenByPath = new Map<string, TreeNode[]>();
  expandedDirs.forEach((entry, index) => {
    childrenByPath.set(entry.path, resolvedChildren[index]);
  });

  const nextNodes: TreeNode[] = [];

  for (const [index, entry] of entries.entries()) {
    const previous = previousByPath.get(entry.path);
    const expanded = previous?.expanded ?? false;
    let children: TreeNode[] | null = null;

    if (entry.is_dir) {
      if (expanded) {
        children = childrenByPath.get(entry.path) ?? null;
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
