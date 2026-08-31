import { useState, useEffect, useMemo } from "react";
import { invoke } from "@tauri-apps/api/core";
import { X, FileCode, FileText, FilePlus2, FileMinus2 } from "lucide-react";
import { FileGlyph } from "../file-icons";

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

// ── Unified diff parser ──────────────────────────────────────────────────────

interface DiffFile {
  header: string;      // e.g. "src/components/Foo.tsx"
  meta: string[];      // index, --- , +++ lines
  hunks: DiffHunk[];
}

interface DiffHunk {
  header: string;       // @@ -1,5 +1,7 @@ optional context
  lines: DiffLineInfo[];
}

interface DiffLineInfo {
  type: "add" | "del" | "ctx";   // + / - / context
  content: string;                // line text WITHOUT the leading +/-/space
  oldLn: number | null;
  newLn: number | null;
}

function parseDiff(raw: string): DiffFile[] {
  const lines = raw.split("\n");
  const files: DiffFile[] = [];
  let currentFile: DiffFile | null = null;
  let currentHunk: DiffHunk | null = null;
  let oldLn = 0;
  let newLn = 0;

  for (const line of lines) {
    // ── File header ──
    if (line.startsWith("diff --git ")) {
      // Extract file path from "diff --git a/path b/path"
      const match = line.match(/^diff --git a\/(.+?) b\/(.+)$/);
      const filePath = match ? match[2] : line.slice(11);
      currentFile = { header: filePath, meta: [], hunks: [] };
      currentHunk = null;
      files.push(currentFile);
      continue;
    }

    // ── Meta lines (index, --- , +++) ──
    if (
      currentFile &&
      !currentHunk &&
      (line.startsWith("index ") ||
        line.startsWith("--- ") ||
        line.startsWith("+++ ") ||
        line.startsWith("old mode ") ||
        line.startsWith("new mode ") ||
        line.startsWith("new file ") ||
        line.startsWith("deleted file ") ||
        line.startsWith("similarity index ") ||
        line.startsWith("rename from ") ||
        line.startsWith("rename to ") ||
        line.startsWith("Binary files "))
    ) {
      currentFile.meta.push(line);
      continue;
    }

    // ── Hunk header ──
    if (line.startsWith("@@")) {
      const match = line.match(/^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@(.*)$/);
      if (match) {
        oldLn = parseInt(match[1], 10);
        newLn = parseInt(match[2], 10);
        currentHunk = {
          header: line,
          lines: [],
        };
        currentFile?.hunks.push(currentHunk);
      }
      continue;
    }

    // ── Diff content lines ──
    if (currentHunk) {
      if (line.startsWith("+")) {
        currentHunk.lines.push({
          type: "add",
          content: line.slice(1),
          oldLn: null,
          newLn: newLn++,
        });
      } else if (line.startsWith("-")) {
        currentHunk.lines.push({
          type: "del",
          content: line.slice(1),
          oldLn: oldLn++,
          newLn: null,
        });
      } else if (line.startsWith(" ") || line === "") {
        // Context line or empty
        currentHunk.lines.push({
          type: "ctx",
          content: line.startsWith(" ") ? line.slice(1) : line,
          oldLn: oldLn++,
          newLn: newLn++,
        });
      } else if (line.startsWith("\\")) {
        // "\ No newline at end of file" — skip
        continue;
      } else {
        // Unknown line outside hunk — reset hunk
        currentHunk = null;
      }
    }

    // If we don't have a file yet, create one for flat diffs
    if (!currentFile && line.trim()) {
      currentFile = { header: "", meta: [], hunks: [] };
      files.push(currentFile);
    }
  }

  return files;
}

// ── File status icon helper ──
function FileStatusIcon({ meta }: { meta: string[] }) {
  const isNew = meta.some((m) => m.startsWith("new file"));
  const isDeleted = meta.some((m) => m.startsWith("deleted file"));

  if (isNew) return <FilePlus2 size={13} color="var(--success)" />;
  if (isDeleted) return <FileMinus2 size={13} color="var(--danger)" />;
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

  const parsedFiles = useMemo(() => parseDiff(diff), [diff]);

  return (
    <div className="ai-git-diff-shell">
      {/* Header */}
      <div className="ai-git-diff-header">
        {filePath ? <FileGlyph path={filePath} size={20} /> : <FileCode size={14} color="var(--text-muted)" />}
        <span className="ai-git-diff-title">
          {title}
        </span>
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
              <DiffFileSection key={fi} file={file} />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

// ── File section ────────────────────────────────────────────────────────────

function DiffFileSection({ file }: { file: DiffFile }) {
  return (
    <div>
      {/* File header — only show if we have a real file header */}
      {file.header && (
        <div className="git-diff-file-header">
          <FileStatusIcon meta={file.meta} />
          <FileGlyph path={file.header} size={20} />
          <span className="git-diff-file-path">{file.header}</span>
        </div>
      )}

      {/* Meta lines (index, ---, +++) */}
      {file.meta
        .filter((m) => !m.startsWith("--- ") && !m.startsWith("+++ ") && !m.startsWith("index "))
        .map((m, i) => (
          <div key={i} className="git-diff-meta-line">
            {m}
          </div>
        ))}

      {/* Hunks */}
      {file.hunks.map((hunk, hi) => (
        <div key={hi}>
          <div className="git-diff-hunk-header">{hunk.header}</div>
          {hunk.lines.map((line, li) => (
            <DiffLineRow key={li} line={line} />
          ))}
        </div>
      ))}
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
