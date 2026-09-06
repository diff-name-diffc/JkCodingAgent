import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { pickCurrentBranchName, type GitBranchInfo } from "../lib/git-branch";

/**
 * 头部上下文用的当前 Git 分支（UI-11）。
 *
 * 拉取时机：mount / projectPath 变化 / 窗口重新获焦。刻意**不做轮询**——
 * 该 hook 服务于常显头部，轮询开销不值（与 BranchBar 的 10s 兜底不同，
 * 分支切换属低频动作，获焦刷新已覆盖外部切换的主要场景）。
 * 非 git 仓库 / git 不可用时静默返回 null，头部不渲染分支 pill。
 */
export function useCurrentGitBranch(projectPath: string | null, enabled = true): string | null {
  const [branch, setBranch] = useState<string | null>(null);

  const fetchBranch = useCallback(async () => {
    if (!enabled || !projectPath) {
      setBranch(null);
      return;
    }
    try {
      const branches = await invoke<GitBranchInfo[]>("git_list_branches", { projectPath });
      setBranch(pickCurrentBranchName(branches));
    } catch {
      // 非 git 仓库或 git 不可用：无分支上下文，不打扰用户。
      setBranch(null);
    }
  }, [enabled, projectPath]);

  useEffect(() => {
    void fetchBranch();
  }, [fetchBranch]);

  useEffect(() => {
    if (!enabled || !projectPath) return;
    const onFocus = () => void fetchBranch();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [enabled, fetchBranch, projectPath]);

  return branch;
}
