import { Check, GitBranch as GitBranchIcon, GitCommit as GitCommitIcon } from "lucide-react";
import { getGitStatusColor, fileName, fileDir } from "../../utils";
import { FileGlyph } from "../../file-icons";

export interface GitCommit {
  hash: string;
  short_hash: string;
  author: string;
  date: string;
  message: string;
  refs: string[];
}

export interface GitCommitDetail {
  hash: string;
  short_hash: string;
  author: string;
  date: string;
  message: string;
  files: GitCommitFile[];
  total_additions: number;
  total_deletions: number;
}

export interface GitCommitFile {
  path: string;
  status: string;
  additions: number;
  deletions: number;
}

export interface GitBranchInfo {
  name: string;
  current: boolean;
}

export function CommitRow({
  commit,
  isSelected,
  isMainActive = false,
  onClick,
}: {
  commit: GitCommit;
  isSelected: boolean;
  /** 该提交的 diff 标签当前为主区活动标签（UI-17 对应高亮）。 */
  isMainActive?: boolean;
  onClick: () => void;
}) {
  const hasBranch = commit.refs.some((r) => !r.startsWith("tag:") && !r.includes("HEAD"));
  const branchNames = commit.refs
    .filter((r) => !r.startsWith("tag:") && !r.includes("HEAD ->"))
    .map((r) => r.trim());

  const rowClass = [
    "ai-git-commit-row",
    isSelected ? "is-selected" : "",
    isMainActive ? "is-active" : "",
  ]
    .filter(Boolean)
    .join(" ");

  return (
    <div onClick={onClick} className={rowClass}>
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

export function BranchOption({
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

export function CommitDetailPanel({
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
