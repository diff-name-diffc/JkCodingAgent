import { useState, useEffect, useCallback, useRef } from "react";
import { useCancellableInvoke } from "../hooks/useCancellableInvoke";
import {
  Search,
  RefreshCw,
  Filter,
  GitCommit as GitCommitIcon,
  GitBranch as GitBranchIcon,
  Loader2,
  ChevronDown,
  Check,
} from "lucide-react";
import { getGitStatusColor, fileName, fileDir } from "../utils";
import { FileGlyph } from "../file-icons";

interface GitCommit {
  hash: string;
  short_hash: string;
  author: string;
  date: string;
  message: string;
  refs: string[];
}

interface GitCommitFile {
  path: string;
  status: string;
  additions: number;
  deletions: number;
}

interface GitCommitDetail {
  hash: string;
  short_hash: string;
  author: string;
  date: string;
  message: string;
  files: GitCommitFile[];
  total_additions: number;
  total_deletions: number;
}

interface GitRemoteCounts {
  ahead: number;
  behind: number;
  branch: string;
}

interface GitBranchInfo {
  name: string;
  current: boolean;
}

interface Props {
  projectPath: string;
  onCommitSelect: (hash: string, message: string) => void;
  onFileClick?: (hash: string, filePath: string, label: string) => void;
  width?: number;
}

export function GitHistory({ projectPath, onCommitSelect, onFileClick, width = 280 }: Props) {
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
  const [error, setError] = useState<string | null>(null);
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
        if (!isCancelled()) setError(String(e));
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
      if (!isCancelled()) setError(String(e));
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
      if (!isCancelled()) setError(String(e));
    } finally {
      if (!isCancelled()) setPushing(false);
    }
  };

  return (
    <div className="ai-git-history ai-migrated-git-history" style={{ width }}>
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

      {/* Error */}
      {error && <div className="ai-git-error">{error}</div>}

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
          return (
            <CommitRow
              key={commit.hash}
              commit={commit}
              isSelected={isSelected}
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

function CommitRow({
  commit,
  isSelected,
  onClick,
}: {
  commit: GitCommit;
  isSelected: boolean;
  onClick: () => void;
}) {
  const hasBranch = commit.refs.some((r) => !r.startsWith("tag:") && !r.includes("HEAD"));
  const branchNames = commit.refs
    .filter((r) => !r.startsWith("tag:") && !r.includes("HEAD ->"))
    .map((r) => r.trim());

  return (
    <div
      onClick={onClick}
      className={isSelected ? "ai-git-commit-row is-selected" : "ai-git-commit-row"}
    >
      {/* Dot indicator */}
      <div className="ai-git-commit-dot-wrap">
        <div
          className={[
            "ai-git-commit-dot",
            isSelected ? "is-selected" : "",
            hasBranch ? "has-ref" : "",
          ]
            .filter(Boolean)
            .join(" ")}
        />
      </div>

      <div className="ai-git-commit-main">
        <div className="ai-git-commit-head">
          <span className="ai-git-commit-message">{commit.message}</span>
          {branchNames.map((ref) => (
            <span key={ref} className="ai-git-ref-pill">
              {ref}
            </span>
          ))}
        </div>
        <div className="ai-git-commit-meta">
          <span>{commit.short_hash}</span>
          <span>{commit.author}</span>
          <span>{commit.date}</span>
        </div>
      </div>
    </div>
  );
}

function BranchOption({
  name,
  current,
  active,
  onClick,
}: {
  name: string;
  current: boolean;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <div
      onClick={onClick}
      className={active ? "ai-git-branch-option is-active" : "ai-git-branch-option"}
    >
      <GitBranchIcon size={11} />
      <span className="ai-git-branch-option-name">{name}</span>
      {current && <span className="ai-git-current-pill">当前</span>}
      {active && <Check size={11} />}
    </div>
  );
}

function CommitDetailPanel({
  detail,
  loading,
  onFileClick,
}: {
  detail: GitCommitDetail;
  loading: boolean;
  onFileClick?: (path: string) => void;
}) {
  if (loading) {
    return <div className="ai-git-empty">加载中…</div>;
  }

  return (
    <div className="ai-git-detail-scroll chat-scroll">
      {/* Commit meta */}
      <div className="ai-git-detail-meta">
        <div className="ai-git-detail-meta-row">
          <GitCommitIcon size={12} />
          <span className="ai-git-detail-hash">{detail.short_hash}</span>
          <span>{detail.author}</span>
          <span className="ai-git-detail-date">{detail.date}</span>
        </div>
        <div className="ai-git-detail-message">{detail.message}</div>
        <div className="ai-git-detail-summary">
          共变更 {detail.files.length} 个文件{" "}
          <span className="ai-git-detail-add">+{detail.total_additions}</span>{" "}
          <span className="ai-git-detail-del">-{detail.total_deletions}</span>
        </div>
      </div>

      {/* File list */}
      {detail.files.map((f) => {
        const color = getGitStatusColor(f.status);
        const name = fileName(f.path);
        const dir = fileDir(f.path);
        const clickable = !!onFileClick;
        return (
          <div
            key={f.path}
            onClick={clickable ? () => onFileClick(f.path) : undefined}
            className={
              clickable ? "ai-git-detail-file-row is-clickable" : "ai-git-detail-file-row"
            }
          >
            <span
              className="ai-git-detail-status"
              style={{
                color,
              }}
            >
              {f.status}
            </span>
            <FileGlyph path={f.path} size={20} />
            <span className="ai-git-detail-file-name-wrap">
              <span className="ai-git-detail-file-name">{name}</span>
              {dir && <span className="ai-git-detail-file-dir">{dir}</span>}
            </span>
            <span className="ai-git-detail-add">+{f.additions}</span>
            <span className="ai-git-detail-del">-{f.deletions}</span>
          </div>
        );
      })}
    </div>
  );
}
