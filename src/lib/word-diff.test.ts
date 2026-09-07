import { describe, expect, it } from "vitest";
import {
  diffWords,
  MAX_WORD_DIFF_CELLS,
  MAX_WORD_DIFF_CHARS,
  type WordDiffSegment,
} from "./word-diff";

function join(segments: WordDiffSegment[]): string {
  return segments.map((s) => s.text).join("");
}

function hlTexts(segments: WordDiffSegment[]): string[] {
  return segments.filter((s) => s.hl).map((s) => s.text);
}

describe("diffWords", () => {
  it("相同串：两侧单段全不高亮，join 还原原文", () => {
    const result = diffWords("same line", "same line");
    expect(result).not.toBeNull();
    expect(result!.oldSegments).toEqual([{ text: "same line", hl: false }]);
    expect(result!.newSegments).toEqual([{ text: "same line", hl: false }]);
  });

  it("完全不同：两侧所有实词段高亮，join 还原原文", () => {
    const result = diffWords("aaa bbb", "ccc ddd");
    expect(result).not.toBeNull();
    expect(hlTexts(result!.oldSegments)).toEqual(["aaa", "bbb"]);
    expect(hlTexts(result!.newSegments)).toEqual(["ccc", "ddd"]);
    expect(join(result!.oldSegments)).toBe("aaa bbb");
    expect(join(result!.newSegments)).toBe("ccc ddd");
  });

  it("单侧空串返回 null（行级背景已足够，空串双空同样 null）", () => {
    expect(diffWords("", "foo")).toBeNull();
    expect(diffWords("foo", "")).toBeNull();
    expect(diffWords("", "")).toBeNull();
  });

  it("中英文混排：CJK 逐字、拉丁按词，公共部分不高亮", () => {
    const result = diffWords("修复bug了", "修复feature了");
    expect(result).not.toBeNull();
    expect(hlTexts(result!.oldSegments)).toEqual(["bug"]);
    expect(hlTexts(result!.newSegments)).toEqual(["feature"]);
    expect(join(result!.oldSegments)).toBe("修复bug了");
    expect(join(result!.newSegments)).toBe("修复feature了");
    // 「修复」「了」逐字匹配消亮
    expect(result!.oldSegments.some((s) => !s.hl && s.text.includes("修"))).toBe(true);
  });

  it("空白差异不高亮（噪声抑制），join 仍还原各自原文", () => {
    const result = diffWords("a  b", "a b");
    expect(result).not.toBeNull();
    expect(hlTexts(result!.oldSegments)).toEqual([]);
    expect(hlTexts(result!.newSegments)).toEqual([]);
    expect(join(result!.oldSegments)).toBe("a  b");
    expect(join(result!.newSegments)).toBe("a b");
  });

  it("超过单侧字符阈值返回 null", () => {
    const long = "x".repeat(MAX_WORD_DIFF_CHARS + 1);
    expect(diffWords(long, "short")).toBeNull();
    expect(diffWords("short", long)).toBeNull();
    expect(MAX_WORD_DIFF_CHARS).toBe(4000);
  });

  it("超过 DP cells 复杂度阈值返回 null", () => {
    // 两行各 ~600 token（601×601 = 361201 > 250000），字符数远低于 char 阈值。
    const a = Array.from({ length: 600 }, (_, i) => `a${i}`).join(" ");
    const b = Array.from({ length: 600 }, (_, i) => `b${i}`).join(" ");
    expect(a.length).toBeLessThan(MAX_WORD_DIFF_CHARS);
    expect((600 + 1) * (600 + 1)).toBeGreaterThan(MAX_WORD_DIFF_CELLS);
    expect(diffWords(a, b)).toBeNull();
  });

  it("中间单词变更：仅变更词高亮，前后公共词与空白不高亮", () => {
    const result = diffWords("the quick fox", "the slow fox");
    expect(result).not.toBeNull();
    expect(result!.oldSegments).toEqual([
      { text: "the ", hl: false },
      { text: "quick", hl: true },
      { text: " fox", hl: false },
    ]);
    expect(result!.newSegments).toEqual([
      { text: "the ", hl: false },
      { text: "slow", hl: true },
      { text: " fox", hl: false },
    ]);
  });
});
