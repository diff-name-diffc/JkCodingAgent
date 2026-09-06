/**
 * Unified diff 文本解析（UI-17 从 GitDiffViewer 迁出的纯函数）。
 *
 * 输入为 `git diff` 输出（后端三模式合一返回的原始文本），输出结构化
 * 文件/hunk/行树；重命名、二进制、新建/删除等扩展元信息以布尔/字段
 * 显式承载，视图层不再解析 meta 行原文。
 */

export interface DiffLineInfo {
  type: "add" | "del" | "ctx";
  /** 行文本（不含行首 +/-/空格标记）。 */
  content: string;
  oldLn: number | null;
  newLn: number | null;
}

export interface DiffHunk {
  header: string;
  lines: DiffLineInfo[];
}

export interface ParsedDiffFile {
  /** 展示路径（diff --git 的 b 侧；扁平 diff 兜底时为空串）。 */
  header: string;
  /** `--- a/…` / `/dev/null` 解析出的旧路径；无信息时为 null。 */
  oldPath: string | null;
  /** `+++ b/…` 解析出的新路径；无信息时为 null。 */
  newPath: string | null;
  isNew: boolean;
  isDeleted: boolean;
  isBinary: boolean;
  renameFrom: string | null;
  renameTo: string | null;
  /** rename/copy 的相似度百分比数值（0–100）。 */
  similarity: number | null;
  hunks: DiffHunk[];
}

const HUNK_HEADER_RE = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@(.*)$/;
const DIFF_GIT_RE = /^diff --git a\/(.+?) b\/(.+)$/;

function stripAbPrefix(line: string): string | null {
  if (line === "/dev/null") {
    return null;
  }
  if (line.startsWith("a/") || line.startsWith("b/")) {
    return line.slice(2);
  }
  return line;
}

export function parseUnifiedDiff(raw: string): ParsedDiffFile[] {
  const lines = raw.split("\n");
  const files: ParsedDiffFile[] = [];
  let currentFile: ParsedDiffFile | null = null;
  let currentHunk: DiffHunk | null = null;
  let oldLn = 0;
  let newLn = 0;

  for (const line of lines) {
    if (line.startsWith("diff --git ")) {
      const match = line.match(DIFF_GIT_RE);
      const filePath = match ? match[2] : line.slice(11);
      currentFile = emptyFile(filePath);
      currentHunk = null;
      files.push(currentFile);
      continue;
    }

    if (currentFile && !currentHunk) {
      if (applyExtendedMeta(currentFile, line)) {
        continue;
      }
    }

    if (line.startsWith("@@")) {
      const match = line.match(HUNK_HEADER_RE);
      if (match) {
        oldLn = parseInt(match[1], 10);
        newLn = parseInt(match[2], 10);
        currentHunk = { header: line, lines: [] };
        if (currentFile) {
          currentFile.hunks.push(currentHunk);
        } else {
          // 无 diff --git 头的扁平 diff：按既有行为兜底建空 header 文件。
          currentFile = emptyFile("");
          files.push(currentFile);
          currentFile.hunks.push(currentHunk);
        }
      }
      continue;
    }

    if (currentHunk) {
      if (line.startsWith("+")) {
        currentHunk.lines.push({ type: "add", content: line.slice(1), oldLn: null, newLn: newLn++ });
      } else if (line.startsWith("-")) {
        currentHunk.lines.push({ type: "del", content: line.slice(1), oldLn: oldLn++, newLn: null });
      } else if (line.startsWith(" ") || line === "") {
        currentHunk.lines.push({
          type: "ctx",
          content: line.startsWith(" ") ? line.slice(1) : line,
          oldLn: oldLn++,
          newLn: newLn++,
        });
      } else if (line.startsWith("\\")) {
        // "\ No newline at end of file"
        continue;
      } else {
        currentHunk = null;
      }
    }

    if (!currentFile && line.trim()) {
      currentFile = emptyFile("");
      files.push(currentFile);
    }
  }

  return files;
}

function emptyFile(header: string): ParsedDiffFile {
  return {
    header,
    oldPath: null,
    newPath: null,
    isNew: false,
    isDeleted: false,
    isBinary: false,
    renameFrom: null,
    renameTo: null,
    similarity: null,
    hunks: [],
  };
}

/**
 * 文件头扩展 meta：命中返回 true（该行已消费）。`--- `/`+++ ` 同时填充
 * oldPath/newPath 供重命名展示；mode 行仅消费不落字段。
 */
function applyExtendedMeta(file: ParsedDiffFile, line: string): boolean {
  if (line.startsWith("--- ")) {
    file.oldPath = stripAbPrefix(line.slice(4));
    return true;
  }
  if (line.startsWith("+++ ")) {
    file.newPath = stripAbPrefix(line.slice(4));
    return true;
  }
  if (line.startsWith("new file")) {
    file.isNew = true;
    return true;
  }
  if (line.startsWith("deleted file")) {
    file.isDeleted = true;
    return true;
  }
  if (line.startsWith("Binary files ") || line === "GIT binary patch") {
    file.isBinary = true;
    return true;
  }
  if (line.startsWith("rename from ")) {
    file.renameFrom = line.slice("rename from ".length);
    return true;
  }
  if (line.startsWith("rename to ")) {
    file.renameTo = line.slice("rename to ".length);
    return true;
  }
  if (line.startsWith("copy from ") || line.startsWith("copy to ")) {
    return true;
  }
  const similarityMatch = line.match(/^similarity index (\d+)%$/);
  if (similarityMatch) {
    file.similarity = parseInt(similarityMatch[1], 10);
    return true;
  }
  if (line.startsWith("index ") || line.startsWith("old mode ") || line.startsWith("new mode ")) {
    return true;
  }
  return false;
}

/** 展示用主路径：重命名优先 renameTo，否则 newPath/header。 */
export function diffFileDisplayPath(file: ParsedDiffFile): string {
  return file.renameTo ?? file.newPath ?? file.header;
}
