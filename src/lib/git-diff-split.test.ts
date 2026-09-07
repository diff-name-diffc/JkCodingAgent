import { describe, expect, it } from "vitest";
import { buildSplitRows, EMPTY_SIDE } from "./git-diff-split";
import { MAX_WORD_DIFF_CHARS } from "./word-diff";
import type { DiffHunk, DiffLineInfo } from "./git-diff";

function del(oldLn: number, content: string): DiffLineInfo {
  return { type: "del", content, oldLn, newLn: null };
}
function add(newLn: number, content: string): DiffLineInfo {
  return { type: "add", content, oldLn: null, newLn };
}
function ctx(oldLn: number, newLn: number, content: string): DiffLineInfo {
  return { type: "ctx", content, oldLn, newLn };
}
function hunk(lines: DiffLineInfo[]): DiffHunk {
  return { header: "@@ -1,1 +1,1 @@", lines };
}

describe("buildSplitRows", () => {
  it("对齐的 del/add 变更组 zip 成同一行（左 del、右 add）", () => {
    const rows = buildSplitRows(hunk([del(1, "a=1"), add(1, "a=2")]));
    expect(rows).toHaveLength(1);
    // toMatchObject：配对侧带 word-level segments（专门用例在下方与 word-diff.test.ts）。
    expect(rows[0].left).toMatchObject({ ln: 1, content: "a=1", type: "del" });
    expect(rows[0].right).toMatchObject({ ln: 1, content: "a=2", type: "add" });
  });

  it("ctx 行双侧同内容、各带自身行号", () => {
    const rows = buildSplitRows(hunk([ctx(3, 5, "keep")]));
    expect(rows).toHaveLength(1);
    expect(rows[0].left).toEqual({ ln: 3, content: "keep", type: "ctx" });
    expect(rows[0].right).toEqual({ ln: 5, content: "keep", type: "ctx" });
  });

  it("ctx 边界分组：ctx→del/add→ctx 顺序与分组正确", () => {
    const rows = buildSplitRows(
      hunk([ctx(1, 1, "top"), del(2, "old"), add(2, "new"), ctx(3, 3, "bottom")]),
    );
    expect(rows).toHaveLength(3);
    expect(rows[0].left.type).toBe("ctx");
    expect(rows[1].left).toMatchObject({ ln: 2, content: "old", type: "del" });
    expect(rows[1].right).toMatchObject({ ln: 2, content: "new", type: "add" });
    expect(rows[2].right.type).toBe("ctx");
  });

  it("del 比 add 多：短侧（右）用 empty 占位补齐", () => {
    const rows = buildSplitRows(hunk([del(1, "x"), del(2, "y"), add(1, "z")]));
    expect(rows).toHaveLength(2);
    expect(rows[0]).toMatchObject({
      left: { ln: 1, content: "x", type: "del" },
      right: { ln: 1, content: "z", type: "add" },
    });
    expect(rows[1].left).toEqual({ ln: 2, content: "y", type: "del" });
    expect(rows[1].right).toBe(EMPTY_SIDE);
  });

  it("add 比 del 多：短侧（左）用 empty 占位补齐", () => {
    const rows = buildSplitRows(hunk([del(1, "x"), add(1, "p"), add(2, "q")]));
    expect(rows).toHaveLength(2);
    expect(rows[1].left).toBe(EMPTY_SIDE);
    expect(rows[1].right).toEqual({ ln: 2, content: "q", type: "add" });
  });

  it("纯 add hunk（新建文件语义）：每行左 empty 右 add", () => {
    const rows = buildSplitRows(hunk([add(1, "a"), add(2, "b")]));
    expect(rows).toHaveLength(2);
    expect(rows.every((r) => r.left === EMPTY_SIDE)).toBe(true);
    expect(rows.map((r) => r.right.type)).toEqual(["add", "add"]);
  });

  it("纯 del hunk（删除文件语义）：每行左 del 右 empty", () => {
    const rows = buildSplitRows(hunk([del(1, "a"), del(2, "b")]));
    expect(rows).toHaveLength(2);
    expect(rows.every((r) => r.right === EMPTY_SIDE)).toBe(true);
    expect(rows.map((r) => r.left.type)).toEqual(["del", "del"]);
  });

  it("多个变更组以 ctx 分隔，各自独立 zip 不跨组错位", () => {
    const rows = buildSplitRows(
      hunk([del(1, "d1"), add(1, "a1"), ctx(2, 2, "c"), del(3, "d2"), add(3, "a2")]),
    );
    expect(rows).toHaveLength(3);
    expect(rows[0].left.content).toBe("d1");
    expect(rows[0].right.content).toBe("a1");
    expect(rows[1].left.type).toBe("ctx");
    expect(rows[2].left.content).toBe("d2");
    expect(rows[2].right.content).toBe("a2");
  });

  it("末尾未闭合变更组（无尾随 ctx）也被冲刷", () => {
    const rows = buildSplitRows(hunk([ctx(1, 1, "c"), del(2, "d"), add(2, "a")]));
    expect(rows).toHaveLength(2);
    expect(rows[1].left.content).toBe("d");
    expect(rows[1].right.content).toBe("a");
  });

  it("空 hunk 返回空数组", () => {
    expect(buildSplitRows(hunk([]))).toEqual([]);
  });

  it("极端不等长（100 del + 1 add）产生 100 行，仅首行右侧非空", () => {
    const lines: DiffLineInfo[] = [];
    for (let i = 0; i < 100; i++) lines.push(del(i + 1, `d${i}`));
    lines.push(add(1, "only"));
    const rows = buildSplitRows(hunk(lines));
    expect(rows).toHaveLength(100);
    expect(rows[0].right.content).toBe("only");
    expect(rows.slice(1).every((r) => r.right === EMPTY_SIDE)).toBe(true);
  });

  // ── 行内 word-level 高亮（UI-17 遗留第三批）──────────────────────────────

  it("配对 del/add 行两侧携带 segments，变更词高亮、公共词不高亮", () => {
    const rows = buildSplitRows(hunk([del(1, "the quick fox"), add(1, "the slow fox")]));
    expect(rows).toHaveLength(1);
    const { left, right } = rows[0];
    expect(left.segments).toBeDefined();
    expect(right.segments).toBeDefined();
    expect(left.segments!.filter((s) => s.hl).map((s) => s.text)).toEqual(["quick"]);
    expect(right.segments!.filter((s) => s.hl).map((s) => s.text)).toEqual(["slow"]);
    // 各段 join 恒等于行内容
    expect(left.segments!.map((s) => s.text).join("")).toBe(left.content);
    expect(right.segments!.map((s) => s.text).join("")).toBe(right.content);
  });

  it("未配对侧不带 segments（多 del 单 add 时第二行 del 无高亮）", () => {
    const rows = buildSplitRows(hunk([del(1, "x"), del(2, "y"), add(1, "z")]));
    expect(rows[0].left.segments).toBeDefined();
    expect(rows[1].left.segments).toBeUndefined();
    expect(rows[1].right).toBe(EMPTY_SIDE);
  });

  it("ctx 行双侧不带 segments", () => {
    const rows = buildSplitRows(hunk([ctx(1, 1, "keep me")]));
    expect(rows[0].left.segments).toBeUndefined();
    expect(rows[0].right.segments).toBeUndefined();
  });

  it("超长配对行触发阈值降级：双侧无 segments，行结构不受影响", () => {
    const longOld = `a ${"x".repeat(MAX_WORD_DIFF_CHARS)}`;
    const longNew = `b ${"x".repeat(MAX_WORD_DIFF_CHARS)}`;
    const rows = buildSplitRows(hunk([del(1, longOld), add(1, longNew)]));
    expect(rows).toHaveLength(1);
    expect(rows[0].left.segments).toBeUndefined();
    expect(rows[0].right.segments).toBeUndefined();
    expect(rows[0].left.content).toBe(longOld);
    expect(rows[0].right.content).toBe(longNew);
  });
});
