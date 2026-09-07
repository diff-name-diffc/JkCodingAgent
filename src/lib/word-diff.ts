/**
 * 词级（token 级）行内 diff —— split 并排 diff 中配对 del/add 行的
 * word-level 高亮纯函数（UI-17 遗留，登记见 docs/ui-redesign-2026-09-06/
 * 03-tasks.md 第 18 节）。
 *
 * 零依赖自写 token 级 LCS：tokenize → DP → 回溯标记 → 合并相邻段。
 * token 粒度 CJK 感知：Han/假名逐字成 token（中文整段若按 \S+ 会退化为
 * 单 token，高亮无意义）、拉丁词按连续非空白成 token、空白独立成 token；
 * join(tokens) 恒等于原文，段文本可精确重建行内容。
 *
 * 降噪与保护：
 * - 空白 token 永不高亮（缩进/连排空白差异不产生噪声）；
 * - 单侧字符数超 MAX_WORD_DIFF_CHARS 或 DP cells 超 MAX_WORD_DIFF_CELLS
 *   时返回 null，渲染层优雅降级为纯文本（行级 add/del 背景仍在）；
 * - 任一侧 tokenize 为空（空行配对）返回 null，行级背景已足够。
 */

export interface WordDiffSegment {
  text: string;
  /** 该段是否为本次行内变更（相对另一侧未匹配到的 token）。 */
  hl: boolean;
}

export interface WordDiffResult {
  oldSegments: WordDiffSegment[];
  newSegments: WordDiffSegment[];
}

/** 单侧字符数上限：超长行（minified 产物等）跳过高亮。 */
export const MAX_WORD_DIFF_CHARS = 4000;

/** LCS DP 单元数上限（≈500×500 token），防大行 O(m·n) 爆炸。 */
export const MAX_WORD_DIFF_CELLS = 250_000;

const WHITESPACE_TOKEN_RE = /^\s+$/;

// CJK 感知分词：Han/平假名/片假名逐字、其余非空白连续成词、空白独立成段。
const TOKEN_RE =
  /\p{Script=Han}|\p{Script=Hiragana}|\p{Script=Katakana}|[^\s\p{Script=Han}\p{Script=Hiragana}\p{Script=Katakana}]+|\s+/gu;

function tokenize(text: string): string[] {
  return text.match(TOKEN_RE) ?? [];
}

function mergeSegments(tokens: string[], hlFlags: boolean[]): WordDiffSegment[] {
  const segments: WordDiffSegment[] = [];
  for (let i = 0; i < tokens.length; i++) {
    const last = segments[segments.length - 1];
    if (last && last.hl === hlFlags[i]) {
      last.text += tokens[i];
    } else {
      segments.push({ text: tokens[i], hl: hlFlags[i] });
    }
  }
  return segments;
}

/**
 * 对一对已配对的 del/add 行内容做词级 diff。
 * 返回两侧的高亮分段（各自 join 恒等于入参原文）；不适合/不值得高亮时
 * 返回 null（调用方降级为纯文本渲染）。
 */
export function diffWords(oldText: string, newText: string): WordDiffResult | null {
  if (oldText.length > MAX_WORD_DIFF_CHARS || newText.length > MAX_WORD_DIFF_CHARS) {
    return null;
  }
  if (oldText === newText) {
    // 完全相同（含双空）：无变更可强调，空串交给渲染层的占位兜底。
    return oldText === ""
      ? null
      : {
          oldSegments: [{ text: oldText, hl: false }],
          newSegments: [{ text: newText, hl: false }],
        };
  }

  const oldTokens = tokenize(oldText);
  const newTokens = tokenize(newText);
  if (oldTokens.length === 0 || newTokens.length === 0) return null;

  const m = oldTokens.length;
  const n = newTokens.length;
  if ((m + 1) * (n + 1) > MAX_WORD_DIFF_CELLS) return null;

  // LCS DP：dp[i][j] = oldTokens[i..] 与 newTokens[j..] 的 LCS 长度。
  const stride = n + 1;
  const dp = new Uint32Array((m + 1) * stride);
  for (let i = m - 1; i >= 0; i--) {
    for (let j = n - 1; j >= 0; j--) {
      dp[i * stride + j] =
        oldTokens[i] === newTokens[j]
          ? dp[(i + 1) * stride + j + 1] + 1
          : Math.max(dp[(i + 1) * stride + j], dp[i * stride + j + 1]);
    }
  }

  // 回溯：匹配 token 双侧消亮，未匹配 token 标记高亮。
  const oldHl = new Array<boolean>(m).fill(true);
  const newHl = new Array<boolean>(n).fill(true);
  let i = 0;
  let j = 0;
  while (i < m && j < n) {
    if (oldTokens[i] === newTokens[j]) {
      oldHl[i] = false;
      newHl[j] = false;
      i++;
      j++;
    } else if (dp[(i + 1) * stride + j] >= dp[i * stride + j + 1]) {
      i++;
    } else {
      j++;
    }
  }

  // 空白 token 永不高亮（噪声抑制）。
  for (let k = 0; k < m; k++) if (WHITESPACE_TOKEN_RE.test(oldTokens[k])) oldHl[k] = false;
  for (let k = 0; k < n; k++) if (WHITESPACE_TOKEN_RE.test(newTokens[k])) newHl[k] = false;

  return {
    oldSegments: mergeSegments(oldTokens, oldHl),
    newSegments: mergeSegments(newTokens, newHl),
  };
}
