import { describe, expect, it } from "vitest";
import {
  focusAfterInsert,
  normalizeSelectionRange,
  selectedCharacterCount,
  selectedText,
} from "./large-file-selection";

describe("large file selection", () => {
  it("标准化反向拖拽选区", () => {
    expect(normalizeSelectionRange({ line: 3, col: 4 }, { line: 1, col: 2 })).toEqual({
      startLine: 1,
      startCol: 2,
      endLine: 3,
      endCol: 4,
    });
  });

  it("提取跨行文本并计算包含换行符的删除长度", () => {
    const range = { startLine: 4, startCol: 2, endLine: 6, endCol: 1 };
    const lines = ["abcd", "middle", "xyz"];

    expect(selectedText(lines, range)).toBe("cd\nmiddle\nx");
    expect(selectedCharacterCount(lines, range)).toBe(11);
  });

  it("计算多行插入后的光标位置", () => {
    expect(focusAfterInsert(8, 3, "one\ntwo\nlast")).toEqual({ line: 10, col: 4 });
    expect(focusAfterInsert(8, 3, "abc")).toEqual({ line: 8, col: 6 });
  });
});
