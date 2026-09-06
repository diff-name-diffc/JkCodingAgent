import { useState, useEffect, useCallback, useRef } from "react";
import { useCancellableInvoke } from "../hooks/useCancellableInvoke";
import {
  ChevronDown,
  GitBranch as GitBranchIcon,
  Loader2,
  RefreshCw,
  RotateCcw,
  Search,
  Filter,
} from "lucide-react";
import {
  BranchOption,
  CommitDetailPanel,
  CommitRow,
  type GitBranchInfo,
  type GitCommit,
  type GitCommitDetail,
} from "./git/GitHistoryParts";

interface GitRemoteCounts {
  ahead: number;
  behind: number;
  branch: string;
}

interface Props {
  projectPath: string;
  onCommitSelect: (hash: string, message: string) => void;
  onFileClick?: (hash: string, filePath: string, label: string) => void;
  /** 活动的主区提交 diff 标签 hash（UI-17 导航列表 ↔ diff 对应）。 */
  activeCommitHash?: string | null;
}

export function GitHistory({
  projectPath,
  onCommitSelect,
  onFileClick,
  activeCommitHash = null,
}: Props) {
  const [commits, setCommits] = useState<GitCommit[]>([]);
  const [remoteCounts, setRemoteCounts] = useState<GitRemoteCounts>({
    ahead: 0,
    behind: 0,
    branch: "",
  });
  const [branches, setBranches] = useState<GitBranchInfo[]>([]);
  const [selectedBranch, setSelectedBranch] = useState<string>("");
  const [selectedHash, setSelectedHash] = useState<string | null>(null);
  const [selectedDetail, setSelectedDetail] = useState<GitCommitDetail | null>(null);
  const [searchQuery, setSearchQuery] = useState("");
  const [loading, setLoading] = useState(false);
  const [loadingDetail, setLoadingDetail] = useState(false);
  const [error, setError] = useState<{ message: string; retry: () => void } | null>(null);
  const [pushing, setPushing] = useState(false);
  const [pulling, setPulling] = useState(false);
  const [branchOpen, setBranchOpen] = useState(false);
  const branchDropRef = useRef<HTMLDivElement>(null);

  const { safeInvoke, isCancelled } = useCancellableInvoke();

  useEffect(() => {
    if (!branchOpen) return;
    const handleClick = (e: MouseEvent) => {
      if (!branchDropRef.current?.contains(e.target as Node)) {
        setBranchOpen(false);
      }
    };
    document.addEventListener("mousedown", handleClick);
    return () => document.removeEventListener("mousedown", handleClick);
  }, [branchOpen]);

  const loadBranches = useCallback(async () => {
    try {
      const list = await safeInvoke<GitBranchInfo[]>("git_list_branches", { projectPath });
      if (list === null) return; // Component unmounted
      setBranches(list);
      // Set initial branch to current if not yet set
      setSelectedBranch((prev) => {
        if (prev) return prev;
        return list.find((b) => b.current)?.name ?? "";
      });
    } catch {
      // ignore
    }
  }, [projectPath, safeInvoke]);

  const refresh = useCallback(
    async (query?: string, branch?: string) => {
      setLoading(true);
      setError(null);
      const activeBranch = branch ?? selectedBranch;
      try {
        const [log, remote] = await Promise.all([
          safeInvoke<GitCommit[]>("git_log", {
            projectPath,
            limit: 50,
            search: query ?? searchQuery,
            branch: activeBranch || null,
          }),
          safeInvoke<GitRemoteCounts>("git_remote_counts", {
            projectPath,
            branch: activeBranch || null,
          }).catch(() => ({ ahead: 0, behind: 0, branch: "" })),
        ]);
        if (log === null) return; // Component unmounted
        setCommits(log);
        setRemoteCounts((remote as GitRemoteCounts) ?? { ahead: 0, behind: 0, branch: "" });
      } catch (e) {
        if (!isCancelled()) {
          setError({ message: String(e), retry: () => void refresh() });
        }
      } finally {
        if (!isCancelled()) setLoading(false);
      }
    },
    [projectPath, searchQuery, selectedBranch, safeInvoke, isCancelled],
  );

  useEffect(() => {
    setSelectedBranch("");
    loadBranches();
    setSelectedHash(null);
    setSelectedDetail(null);
  }, [projectPath, loadBranches]);

  useEffect(() => {
    if (selectedBranch !== "") {
      refresh(undefined, selectedBranch);
    }
    // refresh 依赖 searchQuery，若加入 deps 会在搜索变化时触发此 effect（不预期的行为）
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedBranch]);

  const handleSearch = useCallback(
    (q: string) => {
      setSearchQuery(q);
      refresh(q);
    },
    [refresh],
  );

  const handleSelectCommit = useCallback(
    async (commit: GitCommit) => {
      setSelectedHash(commit.hash);
      onCommitSelect(commit.hash, commit.message);
      setLoadingDetail(true);
      try {
        const detail = await safeInvoke<GitCommitDetail>("git_commit_detail", {
          projectPath,
          commitHash: commit.hash,
        });
        if (detail === null) return; // Component unmounted
        setSelectedDetail(detail);
      } catch {
        if (!isCancelled()) setSelectedDetail(null);
      } finally {
        if (!isCancelled()) setLoadingDetail(false);
      }
    },
    [projectPath, onCommitSelect, safeInvoke, isCancelled],
  );

  const handlePull = async () => {
    setPulling(true);
    setError(null);
    try {
      await safeInvoke("git_pull", { projectPath });
      if (!isCancelled()) refresh();
    } catch (e) {
      if (!isCancelled()) {
        setError({ message: String(e), retry: () => void handlePull() });
      }
    } finally {
      if (!isCancelled()) setPulling(false);
    }
  };

  const handlePush = async () => {
    setPushing(true);
    setError(null);
    try {
      await safeInvoke("git_push", { projectPath, branch: selectedBranch || null });
      if (!isCancelled()) {
        refresh();
        await loadBranches();
      }
    } catch (e) {
      if (!isCancelled()) {
        setError({ message: String(e), retry: () => void handlePush() });
      }
    } finally {
      if (!isCancelled()) setPushing(false);
    }
  };

  return (
    <div className="ai-git-history">
      {/* Header */}
      <div className="ai-git-history-header">
        <div className="ai-git-history-title-row">
          <span className="ai-git-history-title">历史</span>

          <button
            onClick={handlePull}
            disabled={pulling}
            title="拉取"
            className="ai-git-sync-button"
          >
            拉取 ↓{remoteCounts.behind}
          </button>
          <button
            onClick={handlePush}
            disabled={pushing}
            title="推送"
            className={pushing ? "ai-git-sync-button is-active" : "ai-git-sync-button"}
          >
            {pushing ? (
              <>
                <Loader2 size={11} className="spin" />
                推送中…
              </>
            ) : (
              <>推送 ↑{remoteCounts.ahead}</>
            )}
          </button>
          <button
            onClick={() => refresh()}
            title="刷新"
            className="ai-git-icon-button"
          >
            <RefreshCw size={13} />
          </button>
        </div>

        {/* Branch selector */}
        <div ref={branchDropRef} className="ai-git-branch-wrap">
          <button
            onClick={() => setBranchOpen((o) => !o)}
            className={branchOpen ? "ai-git-branch-trigger is-open" : "ai-git-branch-trigger"}
          >
            <GitBranchIcon size={11} className="ai-git-branch-trigger-icon" />
            <span>{selectedBranch || "…"}</span>
            <ChevronDown size={11} className="ai-git-branch-chevron" />
          </button>

          {branchOpen && (
            <div className="ai-git-branch-menu chat-scroll">
              {branches.map((b) => {
                const active = selectedBranch === b.name;
                return (
                  <BranchOption
                    key={b.name}
                    name={b.name}
                    current={b.current}
                    active={active}
                    onClick={() => {
                      setSelectedBranch(b.name);
                      setBranchOpen(false);
                    }}
                  />
                );
              })}
            </div>
          )}
        </div>
      </div>

      {/* Error: 就近反馈（推送/拉取/刷新失败，带重试） */}
      {error && (
        <div className="ai-git-inline-error">
          <span className="ai-git-inline-error-message">{error.message}</span>
          <button
            type="button"
            onClick={error.retry}
            className="ai-git-inline-error-action"
            title="重试"
          >
            <RotateCcw size={12} />
          </button>
          <button
            type="button"
            onClick={() => setError(null)}
            className="ai-git-inline-error-action"
            title="关闭"
          >
            ×
          </button>
        </div>
      )}

      {/* Search */}
      <div className="ai-git-history-search">
        <div className="ai-git-history-search-box">
          <Search size={12} />
          <input
            value={searchQuery}
            onChange={(e) => handleSearch(e.target.value)}
            placeholder="搜索提交"
            className="ai-git-history-search-input"
          />
          <Filter size={12} />
        </div>
      </div>

      {/* Commit list */}
      <div
        className="ai-git-commit-list chat-scroll"
        style={{
          flex: selectedDetail ? "0 0 auto" : 1,
          maxHeight: selectedDetail ? "50%" : undefined,
        }}
      >
        {loading && commits.length === 0 && <div className="ai-git-empty">加载中…</div>}
        {commits.map((commit) => {
          const isSelected = commit.hash === selectedHash;
          const isMainActive = commit.hash === activeCommitHash;
          return (
            <CommitRow
              key={commit.hash}
              commit={commit}
              isSelected={isSelected}
              isMainActive={isMainActive}
              onClick={() => handleSelectCommit(commit)}
            />
          );
        })}
        {!loading && commits.length === 0 && <div className="ai-git-empty">没有找到提交记录</div>}
      </div>

      {/* Commit detail */}
      {selectedDetail && (
        <div className="ai-git-detail-shell">
          <CommitDetailPanel
            detail={selectedDetail}
            loading={loadingDetail}
            onFileClick={
              onFileClick
                ? (path) =>
                    onFileClick(selectedDetail.hash, path, `${path} @ ${selectedDetail.short_hash}`)
                : undefined
            }
          />
        </div>
      )}
    </div>
  );
}
