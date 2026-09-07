import { useState, useEffect, useMemo } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  X,
  FileCode,
  FileText,
  FilePlus2,
  FileMinus2,
  ArrowRightLeft,
  Binary,
  Columns2,
  Rows2,
} from "lucide-react";
import { FileGlyph } from "../file-icons";
import { cn } from "../lib/cn";
import {
  diffFileDisplayPath,
  parseUnifiedDiff,
  type DiffHunk,
  type DiffLineInfo,
  type ParsedDiffFile,
} from "../lib/git-diff";
import { buildSplitRows, type SplitRow, type SplitSide } from "../lib/git-diff-split";
import {
  loadDiffViewMode,
  saveDiffViewMode,
  type DiffViewMode,
} from "../lib/diff-view-prefs";

interface Props {
  projectPath: string;
  // "commit" = full commit diff, "file" = working-tree file diff, "commit-file" = single file in a commit
  mode: "commit" | "file" | "commit-file";
  commitHash?: string;
  filePath?: string;
  staged?: boolean;
  title: string;
  onClose: () => void;
}

// ── File status icon (structured meta, UI-17) ────────────────────────────────

function FileStatusIcon({ file }: { file: ParsedDiffFile }) {
  if (file.isNew) return <FilePlus2 size={13} color="var(--success)" />;
  if (file.isDeleted) return <FileMinus2 size={13} color="var(--danger)" />;
  if (file.renameFrom && file.renameTo) {
    return <ArrowRightLeft size={13} color="var(--text-secondary)" />;
  }
  if (file.isBinary) return <Binary size={13} color="var(--text-hint)" />;
  return <FileText size={13} color="var(--text-hint)" />;
}

// ── Main component ──────────────────────────────────────────────────────────

export function GitDiffViewer({
  projectPath,
  mode,
  commitHash,
  filePath,
  staged,
  title,
  onClose,
}: Props) {
  const [diff, setDiff] = useState<string>("");
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  // unified / split 视图模式（UI-17 遗留）：全局 UI 偏好，跨会话持久化。
  const [viewMode, setViewMode] = useState<DiffViewMode>(() => loadDiffViewMode());

  const toggleViewMode = () => {
    setViewMode((prev) => {
      const next: DiffViewMode = prev === "split" ? "unified" : "split";
      saveDiffViewMode(next);
      return next;
    });
  };

  useEffect(() => {
    setLoading(true);
    setError(null);

    const load = async () => {
      try {
        // 参数组合不满足所选模式时不发请求（与历史行为一致：显示空态）。
        const canLoad =
          (mode === "commit" && !!commitHash) ||
          (mode === "commit-file" && !!commitHash && filePath !== undefined) ||
          (mode === "file" && filePath !== undefined);
        if (!canLoad) {
          setDiff("");
          return;
        }
        const result = await invoke<string>("git_diff", {
          projectPath,
          mode,
          commitHash: commitHash ?? null,
          filePath: filePath ?? null,
          staged: staged ?? false,
        });
        setDiff(result);
      } catch (e) {
        setError(String(e));
      } finally {
        setLoading(false);
      }
    };

    load();
  }, [projectPath, mode, commitHash, filePath, staged]);

  const parsedFiles = useMemo(() => parseUnifiedDiff(diff), [diff]);

  return (
    <div className="ai-git-diff-shell">
      {/* Header */}
      <div className="ai-git-diff-header">
        {filePath ? <FileGlyph path={filePath} size={20} /> : <FileCode size={14} color="var(--text-muted)" />}
        <span className="ai-git-diff-title">
          {title}
        </span>
        <button
          onClick={toggleViewMode}
          className="ai-git-icon-button"
          aria-label={viewMode === "split" ? "切换到单栏视图" : "切换到并排视图"}
          aria-pressed={viewMode === "split"}
          title={viewMode === "split" ? "切换到单栏视图" : "切换到并排视图"}
        >
          {viewMode === "split" ? <Rows2 size={14} /> : <Columns2 size={14} />}
        </button>
        <button
          onClick={onClose}
          className="ai-git-icon-button"
          aria-label="关闭差异视图"
        >
          <X size={14} />
        </button>
      </div>

      {/* Content */}
      <div className="ai-git-diff-scroll chat-scroll">
        {loading ? (
          <div className="git-diff-empty">正在加载差异…</div>
        ) : error ? (
          <div className="ai-git-diff-error">{error}</div>
        ) : diff.trim() === "" ? (
          <div className="git-diff-empty">没有变更</div>
        ) : (
          <div className="git-diff-viewer">
            {parsedFiles.map((file, fi) => (
              <DiffFileSection key={fi} file={file} viewMode={viewMode} />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

// ── File section ────────────────────────────────────────────────────────────

function DiffFileSection({
  file,
  viewMode,
}: {
  file: ParsedDiffFile;
  viewMode: DiffViewMode;
}) {
  const isRename = file.renameFrom !== null && file.renameTo !== null;
  const displayPath = diffFileDisplayPath(file);

  return (
    <div>
      {file.header && (
        <div className="git-diff-file-header">
          <FileStatusIcon file={file} />
          <FileGlyph path={displayPath} size={20} />
          {isRename ? (
            <span className="git-diff-file-path" title={`${file.renameFrom} → ${file.renameTo}`}>
              <span className="git-diff-rename-old">{file.renameFrom}</span>
              <span className="git-diff-rename-arrow"> → </span>
              <span className="git-diff-rename-new">{file.renameTo}</span>
              {file.similarity !== null && (
                <span className="git-diff-similarity">相似度 {file.similarity}%</span>
              )}
            </span>
          ) : (
            <span className="git-diff-file-path" title={displayPath}>
              {displayPath}
            </span>
          )}
        </div>
      )}

      {file.isBinary ? (
        <div className="git-diff-binary">二进制文件，不支持文本差异展示</div>
      ) : (
        file.hunks.map((hunk, hi) => (
          <div key={hi}>
            <div className="git-diff-hunk-header">{hunk.header}</div>
            {viewMode === "split"
              ? <SplitHunkRows hunk={hunk} />
              : hunk.lines.map((line, li) => <DiffLineRow key={li} line={line} />)}
          </div>
        ))
      )}
    </div>
  );
}

// ── Individual line ─────────────────────────────────────────────────────────

function DiffLineRow({ line }: { line: DiffLineInfo }) {
  const cls =
    line.type === "add"
      ? "git-diff-line git-diff-line--add"
      : line.type === "del"
        ? "git-diff-line git-diff-line--del"
        : "git-diff-line git-diff-line--ctx";

  const signCls =
    line.type === "add"
      ? "git-diff-sign git-diff-sign--add"
      : line.type === "del"
        ? "git-diff-sign git-diff-sign--del"
        : "git-diff-sign";

  const signChar = line.type === "add" ? "+" : line.type === "del" ? "−" : " ";

  return (
    <div className={cls}>
      <span className="git-diff-ln">{line.oldLn ?? ""}</span>
      <span className="git-diff-ln">{line.newLn ?? ""}</span>
      <span className={signCls}>{signChar}</span>
      <span className="git-diff-content">{line.content || " "}</span>
    </div>
  );
}

// ── Split (side-by-side) row ──────────────────────────────────────────────────

/**
 * 单个 hunk 的 split 行：buildSplitRows 含配对行的词级 diff 计算（O(m·n)），
 * 以 hunk 身份 memo——parsedFiles 已由上层 useMemo 稳定，viewMode 切换等
 * 重渲染命中缓存不重算。
 */
function SplitHunkRows({ hunk }: { hunk: DiffHunk }) {
  const rows = useMemo(() => buildSplitRows(hunk), [hunk]);
  return (
    <>
      {rows.map((row, ri) => (
        <SplitRowView key={ri} row={row} />
      ))}
    </>
  );
}

function SplitRowView({ row }: { row: SplitRow }) {
  return (
    <div className="git-diff-split-row">
      <SplitSideView side={row.left} />
      <SplitSideView side={row.right} />
    </div>
  );
}

function SplitSideView({ side }: { side: SplitSide }) {
  // empty 占位侧（del/add run 不等长时补齐）：灰底无内容，读屏跳过。
  if (side.type === "empty") {
    return (
      <div className="git-diff-split-side git-diff-split-side--empty" aria-hidden="true" />
    );
  }

  const signCls =
    side.type === "add"
      ? "git-diff-sign git-diff-sign--add"
      : side.type === "del"
        ? "git-diff-sign git-diff-sign--del"
        : "git-diff-sign";
  const signChar = side.type === "add" ? "+" : side.type === "del" ? "−" : " ";

  return (
    <div className={cn("git-diff-split-side", `git-diff-split-side--${side.type}`)}>
      <span className="git-diff-ln">{side.ln ?? ""}</span>
      <span className={signCls}>{signChar}</span>
      <span className="git-diff-content">
        {side.segments
          ? side.segments.map((seg, si) =>
              seg.hl ? (
                <span
                  key={si}
                  className={cn("git-diff-word-hl", `git-diff-word-hl--${side.type}`)}
                >
                  {seg.text}
                </span>
              ) : (
                seg.text
              ),
            )
          : side.content || " "}
      </span>
    </div>
  );
}
