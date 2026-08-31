import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { RefreshCw, GitCommit, Sparkles, ChevronRight, ChevronDown } from "lucide-react";
import { useCancellableInvoke } from "../hooks/useCancellableInvoke";
import {
  fileDir,
  fileName,
  getGitStatusColor,
  getGitStatusLabel,
  isImeComposing,
} from "../utils";
import { FileGlyph } from "../file-icons";

interface GitFileChange {
  path: string;
  status: string;
  staged: boolean;
}

interface Props {
  projectPath: string;
  onFileSelect: (filePath: string, staged: boolean, label: string) => void;
  width?: number;
}

export function GitChanges({
  projectPath,
  onFileSelect,
  width = 280,
}: Props) {
  const [changes, setChanges] = useState<GitFileChange[]>([]);
  const [loading, setLoading] = useState(false);
  const [commitMsg, setCommitMsg] = useState("");
  const [committing, setCommitting] = useState(false);
  const [generatingMsg, setGeneratingMsg] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [commitMsgError, setCommitMsgError] = useState(false);
  const [trackedCollapsed, setTrackedCollapsed] = useState(false);
  const [untrackedCollapsed, setUntrackedCollapsed] = useState(false);

  const { safeInvoke, isCancelled } = useCancellableInvoke();

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const result = await safeInvoke<GitFileChange[]>("git_status", { projectPath });
      if (result === null) return; // Component unmounted
      setChanges(result);
    } catch (e) {
      if (!isCancelled()) setError(String(e));
    } finally {
      if (!isCancelled()) setLoading(false);
    }
  }, [projectPath, safeInvoke, isCancelled]);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const trackedFiles = changes.filter((c) => c.status !== "?");
  const untrackedFiles = changes.filter((c) => c.status === "?");
  const stagedFiles = trackedFiles.filter((c) => c.staged);
  const unstagedFiles = trackedFiles.filter((c) => !c.staged);

  const handleStageToggle = async (c: GitFileChange, e: React.MouseEvent) => {
    e.stopPropagation();
    try {
      if (c.staged) {
        await invoke("git_unstage", { projectPath, filePath: c.path });
      } else {
        await invoke("git_stage", { projectPath, filePath: c.path });
      }
      refresh();
    } catch (err) {
      setError(String(err));
    }
  };

  const handleStageAll = async () => {
    try {
      setError(null);
      await invoke("git_stage_all", { projectPath });
      refresh();
    } catch (err) {
      setError(String(err));
    }
  };

  const handleUnstageAll = async () => {
    try {
      setError(null);
      await invoke("git_unstage_all", { projectPath });
      refresh();
    } catch (err) {
      setError(String(err));
    }
  };

  const handleGenerateMsg = async () => {
    setGeneratingMsg(true);
    setError(null);
    try {
      const msg = await safeInvoke<string>("generate_commit_message", { projectPath });
      if (msg === null) return; // Component unmounted
      setCommitMsg(msg);
      if (commitMsgError) setCommitMsgError(false);
    } catch (err) {
      if (!isCancelled()) setError(String(err));
    } finally {
      if (!isCancelled()) setGeneratingMsg(false);
    }
  };

  const handleCommit = async () => {
    if (!commitMsg.trim()) {
      setCommitMsgError(true);
      return;
    }
    setCommitMsgError(false);
    setCommitting(true);
    setError(null);
    try {
      await invoke("git_commit", { projectPath, message: commitMsg.trim() });
      setCommitMsg("");
      refresh();
    } catch (err) {
      setError(String(err));
    } finally {
      setCommitting(false);
    }
  };

  return (
    <div className="ai-git-changes" style={{ width }}>
      {/* Header */}
      <div className="ai-git-header">
        <span className="ai-git-title">变更</span>
        <button
          onClick={refresh}
          title="刷新"
          className="ai-git-icon-button"
        >
          <RefreshCw size={13} className={loading ? "spin" : ""} />
        </button>
      </div>

      {/* Error */}
      {error && (
        <div className="ai-git-error">
          {error}
        </div>
      )}

      {/* File list */}
      <div className="ai-git-change-list chat-scroll">
        {changes.length === 0 && !loading && (
          <div className="ai-git-empty">
            暂无变更
          </div>
        )}

        {/* ── Tracked changes section ── */}
        {trackedFiles.length > 0 && (
          <>
            <TopSectionHeader
              label="变更"
              count={trackedFiles.length}
              collapsed={trackedCollapsed}
              onToggleCollapse={() => setTrackedCollapsed((v) => !v)}
            />
            {!trackedCollapsed && (
              <>
                {stagedFiles.length > 0 && (
                  <>
                    <SectionHeader
                      label="已暂存"
                      count={stagedFiles.length}
                      actionIcon="−"
                      actionTitle="全部取消暂存"
                      onAction={handleUnstageAll}
                    />
                    {stagedFiles.map((c) => (
                      <FileRow
                        key={`staged-${c.path}`}
                        change={c}
                        onFileClick={() =>
                          onFileSelect(c.path, true, `${fileName(c.path)}（已暂存）`)
                        }
                        onToggle={(e) => handleStageToggle(c, e)}
                      />
                    ))}
                  </>
                )}
                {unstagedFiles.length > 0 && (
                  <>
                    <SectionHeader
                      label="已修改"
                      count={unstagedFiles.length}
                      actionIcon="+"
                      actionTitle="全部暂存"
                      onAction={handleStageAll}
                    />
                    {unstagedFiles.map((c) => (
                      <FileRow
                        key={`unstaged-${c.path}`}
                        change={c}
                        onFileClick={() =>
                          onFileSelect(c.path, false, `${fileName(c.path)}（未暂存）`)
                        }
                        onToggle={(e) => handleStageToggle(c, e)}
                      />
                    ))}
                  </>
                )}
              </>
            )}
          </>
        )}

        {/* ── Untracked files section ── */}
        {untrackedFiles.length > 0 && (
          <>
            <TopSectionHeader
              label="未跟踪文件"
              count={untrackedFiles.length}
              collapsed={untrackedCollapsed}
              onToggleCollapse={() => setUntrackedCollapsed((v) => !v)}
            />
            {!untrackedCollapsed &&
              untrackedFiles.map((c) => (
                <FileRow
                  key={`untracked-${c.path}`}
                  change={c}
                  onFileClick={() => onFileSelect(c.path, false, `${fileName(c.path)}（未跟踪）`)}
                  onToggle={(e) => handleStageToggle(c, e)}
                />
              ))}
          </>
        )}
      </div>

      {/* Commit area */}
      <div className="ai-git-commit-panel">
        <div className="ai-git-commit-input-wrap">
          <textarea
            value={commitMsg}
            onChange={(e) => {
              setCommitMsg(e.target.value);
              if (commitMsgError) setCommitMsgError(false);
            }}
            placeholder="提交信息…"
            rows={3}
            className={commitMsgError ? "ai-git-commit-textarea is-error" : "ai-git-commit-textarea"}
            onKeyDown={(e) => {
              if (!isImeComposing(e) && e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
                handleCommit();
              }
            }}
          />
          <button
            onClick={handleGenerateMsg}
            disabled={generatingMsg}
            title="用 AI 生成提交信息"
            className={generatingMsg ? "ai-git-commit-generate is-active" : "ai-git-commit-generate"}
          >
            <Sparkles size={14} className={generatingMsg ? "spin" : ""} />
          </button>
        </div>
        {commitMsgError && (
          <div className="ai-git-commit-error">
            请输入提交信息
          </div>
        )}
        <div className="ai-git-commit-actions">
          <button
            onClick={handleCommit}
            disabled={committing || generatingMsg}
            className="ai-git-commit-button"
          >
            <GitCommit size={13} />
            {committing ? "提交中…" : "提交"}
          </button>
        </div>
      </div>
    </div>
  );
}

function TopSectionHeader({
  label,
  count,
  collapsed,
  onToggleCollapse,
}: {
  label: string;
  count: number;
  collapsed: boolean;
  onToggleCollapse: () => void;
}) {
  return (
    <div
      onClick={onToggleCollapse}
      className="ai-git-top-section"
    >
      <span className="ai-git-section-chevron">
        {collapsed ? <ChevronRight size={13} /> : <ChevronDown size={13} />}
      </span>
      <span className="ai-git-top-section-label">
        {label}
      </span>
      <span className="ai-git-count-pill">
        {count}
      </span>
    </div>
  );
}

function SectionHeader({
  label,
  count,
  actionIcon,
  actionTitle,
  onAction,
}: {
  label: string;
  count: number;
  actionIcon?: string;
  actionTitle?: string;
  onAction?: () => void;
}) {
  return (
    <div className="ai-git-section-header">
      <span className="ai-git-section-label">{label}</span>
      <span className="ai-git-section-count">
        {count}
      </span>
      {onAction && (
        <button
          onClick={(e) => {
            e.stopPropagation();
            onAction();
          }}
          title={actionTitle}
          className="ai-git-section-action"
        >
          {actionIcon}
        </button>
      )}
    </div>
  );
}

function FileRow({
  change,
  onFileClick,
  onToggle,
}: {
  change: GitFileChange;
  onFileClick: () => void;
  onToggle: (e: React.MouseEvent) => void;
}) {
  const name = fileName(change.path);
  const dir = fileDir(change.path);
  const color = getGitStatusColor(change.status);
  const label = getGitStatusLabel(change.status);

  return (
    <div
      onClick={onFileClick}
      className="ai-git-file-row"
    >
      {/* Status dot */}
      <span
        className="ai-git-status-dot"
        style={{ background: color }}
      />

      {/* Status letter */}
      <span
        className="ai-git-status-label"
        style={{
          color,
        }}
      >
        {label}
      </span>
      <FileGlyph path={change.path} size={20} />

      {/* Filename + dir */}
      <span className="ai-git-file-name-wrap">
        <span className="ai-git-file-name">
          {name}
        </span>
        {dir && (
          <span className="ai-git-file-dir">{dir}</span>
        )}
      </span>

      {/* Stage/unstage toggle on hover */}
      <button
        onClick={onToggle}
        title={change.staged ? "取消暂存" : "暂存"}
        className="ai-git-file-stage-button"
      >
        {change.staged ? "−" : "+"}
      </button>
    </div>
  );
}
