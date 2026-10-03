/**
 * Git 分支上下文纯函数（UI-11）。
 * 数据形状 `GitBranchInfo` 见 types/infrastructure.ts（与后端 `git_list_branches` 返回一致）。
 */
import type { GitBranchInfo } from "../types/infrastructure";

/** 从分支列表中挑出当前分支名；空列表或无 current 时返回 null（不猜测）。 */
export function pickCurrentBranchName(branches: GitBranchInfo[]): string | null {
  return branches.find((branch) => branch.current)?.name ?? null;
}
