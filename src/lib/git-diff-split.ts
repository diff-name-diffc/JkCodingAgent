/**
 * Split（并排）diff 行构建（UI-17 遗留：split 模式）。
 *
 * 输入为 `parseUnifiedDiff` 产出的单个 hunk（每行已带双侧独立行号
 * oldLn/newLn），输出按行对齐的左右两栏结构：左栏 = 旧文件（del/ctx），
 * 右栏 = 新文件（add/ctx）。纯函数、零后端依赖——split 所需的一切
 * （行类型、双侧行号、内容）unified 解析已完整建模。
 *
 * 对齐算法（O(n)，每 hunk 独立）：顺序扫描，缓冲连续的 del-run 与 add-run；
 * 遇到 ctx 行或 hunk 结束时，把两个 run 按位 zip 成行（左=del、右=add），
 * 短侧用 empty 占位补齐；ctx 行左右同内容、各带自身行号。git unified diff
 * 的变更组恒为「全部 `-` 行后跟全部 `+` 行」且以 ctx/结尾分隔，故按 ctx
 * 边界分组 zip 与真实语义一致。
 */

import type { DiffHunk, DiffLineInfo } from "./git-diff";
import { diffWords, type WordDiffSegment } from "./word-diff";

export type SplitSideType = "add" | "del" | "ctx" | "empty";

export interface SplitSide {
  /** 该侧行号（左=oldLn，右=newLn）；empty 侧为 null。 */
  ln: number | null;
  content: string;
  type: SplitSideType;
  /**
   * 行内 word-level 高亮分段（UI-17 遗留）：仅配对成功的 del/add 侧携带，
   * ctx/empty/未配对侧与超阈值降级（diffWords 返回 null）不带此字段，
   * 渲染层回退纯文本。各段 text join 恒等于 content。
   */
  segments?: WordDiffSegment[];
}

export interface SplitRow {
  left: SplitSide;
  right: SplitSide;
}

/** 占位空侧：del/add run 不等长时补齐短侧，渲染为灰底无内容。 */
export const EMPTY_SIDE: SplitSide = Object.freeze({
  ln: null,
  content: "",
  type: "empty",
});

function delSide(line: DiffLineInfo): SplitSide {
  return { ln: line.oldLn, content: line.content, type: "del" };
}

function addSide(line: DiffLineInfo): SplitSide {
  return { ln: line.newLn, content: line.content, type: "add" };
}

function ctxSides(line: DiffLineInfo): SplitRow {
  return {
    left: { ln: line.oldLn, content: line.content, type: "ctx" },
    right: { ln: line.newLn, content: line.content, type: "ctx" },
  };
}

/** 把一个 hunk 的 unified 行序列构建为按行对齐的 split 行。 */
export function buildSplitRows(hunk: DiffHunk): SplitRow[] {
  const rows: SplitRow[] = [];
  let delRun: DiffLineInfo[] = [];
  let addRun: DiffLineInfo[] = [];

  const flushRuns = () => {
    const n = Math.max(delRun.length, addRun.length);
    for (let i = 0; i < n; i++) {
      const d = delRun[i];
      const a = addRun[i];
      const left = d ? delSide(d) : EMPTY_SIDE;
      const right = a ? addSide(a) : EMPTY_SIDE;
      // 真配对（del↔add 同位）才算行内词级高亮；未配对侧/占位侧不带 segments。
      if (d && a) {
        const wordDiff = diffWords(d.content, a.content);
        if (wordDiff) {
          left.segments = wordDiff.oldSegments;
          right.segments = wordDiff.newSegments;
        }
      }
      rows.push({ left, right });
    }
    delRun = [];
    addRun = [];
  };

  for (const line of hunk.lines) {
    if (line.type === "del") {
      delRun.push(line);
    } else if (line.type === "add") {
      addRun.push(line);
    } else {
      // ctx：先冲刷在前的变更组，再双侧同行渲染上下文。
      flushRuns();
      rows.push(ctxSides(line));
    }
  }
  // 冲刷 hunk 末尾未闭合的变更组。
  flushRuns();

  return rows;
}
