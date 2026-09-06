import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { RefreshCw, GitCommit, Sparkles, RotateCcw } from "lucide-react";
import { useCancellableInvoke } from "../hooks/useCancellableInvoke";
import { fileName, isImeComposing } from "../utils";
import {
  ErrorRow,
  FileRow,
  SectionHeader,
  TopSectionHeader,
  type GitFileChange,
} from "./git/GitChangesParts";

/** 活动的主区文件 diff 标签描述（UI-17 导航列表 ↔ diff 对应）。 */
interface ActiveFileDiff {
  path: string;
  staged: boolean;
}

interface ActionError {
  message: string;
  retry: () => void;
}

interface Props {
  projectPath: string;
  onFileSelect: (filePath: string, staged: boolean, label: string) => void;
  activeFileDiff?: ActiveFileDiff | null;
}

export function GitChanges({
  projectPath,
  onFileSelect,
  activeFileDiff = null,
}: Props) {
  const [changes, setChanges] = useState<GitFileChange[]>([]);
  const [loading, setLoading] = useState(false);
  const [commitMsg, setCommitMsg] = useState("");
  const [committing, setCommitting] = useState(false);
  const [generatingMsg, setGeneratingMsg] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [stageError, setStageError] = useState<ActionError | null>(null);
  const [commitError, setCommitError] = useState<ActionError | null>(null);
  const [commitMsgError, setCommitMsgError] = useState(false);
  const [trackedCollapsed, setTrackedCollapsed] = useState(false);
  const [untrackedCollapsed, setUntrackedCollapsed] = useState(false);

  const { safeInvoke, isCancelled } = useCancellableInvoke();

  const refresh = useCallback(async () => {
    setLoading(true);
    setLoadError(null);
    try {
      const result = await safeInvoke<GitFileChange[]>("git_status", { projectPath });
      if (result === null) return; // Component unmounted
      setChanges(result);
    } catch (e) {
      if (!isCancelled()) setLoadError(String(e));
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
  const nothingStaged = stagedFiles.length === 0;

  const stageToggle = async (c: GitFileChange) => {
    setStageError(null);
    try {
      await safeInvoke<void>("git_stage", {
        projectPath,
        files: [c.path],
        unstage: c.staged,
      });
      refresh();
    } catch (err) {
      if (!isCancelled()) {
        setStageError({
          message: String(err),
          retry: () => void stageToggle(c),
        });
      }
    }
  };

  const handleStageToggle = (c: GitFileChange, e: React.MouseEvent) => {
    e.stopPropagation();
    void stageToggle(c);
  };

  /** 全部暂存只作用于「已修改」区列出的未暂存文件（不波及未跟踪区）。 */
  const handleStageAll = async () => {
    setStageError(null);
    try {
      await safeInvoke<void>("git_stage", {
        projectPath,
        files: unstagedFiles.map((c) => c.path),
      });
      refresh();
    } catch (err) {
      if (!isCancelled()) {
        setStageError({ message: String(err), retry: () => void handleStageAll() });
      }
    }
  };

  const handleUnstageAll = async () => {
    setStageError(null);
    try {
      await safeInvoke<void>("git_stage", {
        projectPath,
        files: stagedFiles.map((c) => c.path),
        unstage: true,
      });
      refresh();
    } catch (err) {
      if (!isCancelled()) {
        setStageError({ message: String(err), retry: () => void handleUnstageAll() });
      }
    }
  };

  const handleGenerateMsg = async () => {
    setGeneratingMsg(true);
    setCommitError(null);
    try {
      const msg = await safeInvoke<string>("generate_commit_message", { projectPath });
      if (msg === null) return; // Component unmounted
      setCommitMsg(msg);
      if (commitMsgError) setCommitMsgError(false);
    } catch (err) {
      if (!isCancelled()) {
        setCommitError({
          message: String(err),
          retry: () => void handleGenerateMsg(),
        });
      }
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
    setCommitError(null);
    try {
      await invoke("git_commit", { projectPath, message: commitMsg.trim() });
      setCommitMsg("");
      refresh();
    } catch (err) {
      // 提交文本在失败路径保留，便于修改后重试。
      setCommitError({ message: String(err), retry: () => void handleCommit() });
    } finally {
      setCommitting(false);
    }
  };

  const commitDisabled = committing || generatingMsg || nothingStaged;

  return (
    <div className="ai-git-changes">
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

      {/* File list */}
      <div className="ai-git-change-list chat-scroll">
        {loadError && (
          <ErrorRow message={loadError} retry={refresh} onDismiss={() => setLoadError(null)} />
        )}
        {stageError && (
          <ErrorRow
            message={stageError.message}
            retry={stageError.retry}
            onDismiss={() => setStageError(null)}
          />
        )}

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
                        isActive={
                          activeFileDiff !== null &&
                          activeFileDiff.path === c.path &&
                          activeFileDiff.staged
                        }
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
                        isActive={
                          activeFileDiff !== null &&
                          activeFileDiff.path === c.path &&
                          !activeFileDiff.staged
                        }
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
        <div className="ai-git-commit-scope">
          {nothingStaged ? "没有已暂存的变更" : `提交暂存的 ${stagedFiles.length} 个文件`}
        </div>
        <div className="ai-git-commit-input-wrap">
          <textarea
            value={commitMsg}
            onChange={(e) => {
              setCommitMsg(e.target.value);
              if (commitMsgError) setCommitMsgError(false);
            }}
            placeholder={nothingStaged ? "先暂存要提交的变更…" : "提交信息…"}
            rows={3}
            className={commitMsgError ? "ai-git-commit-textarea is-error" : "ai-git-commit-textarea"}
            onKeyDown={(e) => {
              if (!isImeComposing(e) && e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
                if (!commitDisabled) handleCommit();
              }
            }}
          />
          <button
            onClick={handleGenerateMsg}
            disabled={generatingMsg || nothingStaged}
            title={nothingStaged ? "没有已暂存的变更" : "用 AI 生成提交信息"}
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
        {commitError && (
          <div className="ai-git-commit-failure">
            <div className="ai-git-commit-failure-message">{commitError.message}</div>
            <div className="ai-git-commit-failure-actions">
              <button type="button" onClick={commitError.retry} className="ai-git-commit-retry">
                <RotateCcw size={12} />
                重试
              </button>
              <button
                type="button"
                onClick={() => setCommitError(null)}
                className="ai-git-commit-dismiss"
              >
                知道了
              </button>
            </div>
          </div>
        )}
        <div className="ai-git-commit-actions">
          <button
            onClick={handleCommit}
            disabled={commitDisabled}
            title={nothingStaged ? "没有已暂存的变更" : "提交已暂存的变更"}
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
