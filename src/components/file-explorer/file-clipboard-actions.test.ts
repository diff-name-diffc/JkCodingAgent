import { describe, expect, it } from "vitest";
import {
  clipboardEntryName,
  describeCopiedEntries,
  describePastedEntries,
  resolveClipboardShortcut,
  resolvePasteDirectory,
} from "./file-clipboard-actions";

describe("resolveClipboardShortcut", () => {
  it("识别 ⌘/Ctrl + C/V", () => {
    expect(resolveClipboardShortcut({ key: "c", metaKey: true, ctrlKey: false })).toBe("copy");
    expect(resolveClipboardShortcut({ key: "V", metaKey: false, ctrlKey: true })).toBe("paste");
  });

  it("带 Shift/Alt 或没有修饰键时让路给其他功能", () => {
    expect(
      resolveClipboardShortcut({ key: "c", metaKey: true, ctrlKey: false, shiftKey: true }),
    ).toBeNull();
    expect(
      resolveClipboardShortcut({ key: "v", metaKey: true, ctrlKey: false, altKey: true }),
    ).toBeNull();
    expect(resolveClipboardShortcut({ key: "c", metaKey: false, ctrlKey: false })).toBeNull();
  });

  it("其他按键不拦截", () => {
    expect(resolveClipboardShortcut({ key: "x", metaKey: true, ctrlKey: false })).toBeNull();
    expect(resolveClipboardShortcut({ key: "a", metaKey: true, ctrlKey: false })).toBeNull();
  });
});

describe("resolvePasteDirectory", () => {
  const root = "/Users/jk/project";

  it("选中目录时粘贴进该目录", () => {
    expect(resolvePasteDirectory({ path: `${root}/docs`, is_dir: true }, root)).toBe(
      `${root}/docs`,
    );
  });

  it("选中文件时粘贴进其父目录", () => {
    expect(resolvePasteDirectory({ path: `${root}/docs/a.md`, is_dir: false }, root)).toBe(
      `${root}/docs`,
    );
  });

  it("未选中任何条目时回退到树根", () => {
    expect(resolvePasteDirectory(null, root)).toBe(root);
  });

  it("文件直接位于根下时回退到根", () => {
    expect(resolvePasteDirectory({ path: "a.md", is_dir: false }, root)).toBe(root);
  });
});

describe("clipboardEntryName", () => {
  it("取最后一段，兼容结尾分隔符与 Windows 分隔符", () => {
    expect(clipboardEntryName("/a/b/c.txt")).toBe("c.txt");
    expect(clipboardEntryName("/a/b/docs/")).toBe("docs");
    expect(clipboardEntryName("C:\\a\\b.txt")).toBe("b.txt");
    expect(clipboardEntryName("plain.txt")).toBe("plain.txt");
  });
});

describe("提示文案", () => {
  it("复制提示区分文件与目录，并说明可粘贴到访达", () => {
    expect(describeCopiedEntries([{ path: "/a/b.txt", is_dir: false }])).toContain("b.txt");
    expect(describeCopiedEntries([{ path: "/a/docs", is_dir: true }])).toContain("目录「docs」");
    expect(describeCopiedEntries([{ path: "/a/b.txt", is_dir: false }])).toContain("访达");
  });

  it("多选时只报数量", () => {
    expect(
      describeCopiedEntries([
        { path: "/a/1.txt", is_dir: false },
        { path: "/a/2.txt", is_dir: false },
      ]),
    ).toContain("2 项");
  });

  it("粘贴提示使用后端返回的真实落盘路径（可能是副本名）", () => {
    expect(describePastedEntries(["/a/b 副本.txt"])).toBe("已粘贴「b 副本.txt」");
    expect(describePastedEntries(["/a/1.txt", "/a/2.txt"])).toBe("已粘贴 2 项");
  });
});

describe("resolvePasteDirectory 盘符根", () => {
  it("Windows 盘符根不退化为 C:（C 盘当前目录的相对引用）", () => {
    // 选中 C 盘根下的文件：父目录应保留盘符根的分隔符形态。
    expect(resolvePasteDirectory({ path: "C:\\a.md", is_dir: false }, "C:\\")).toBe("C:\\");
    expect(resolvePasteDirectory({ path: "C:/a.md", is_dir: false }, "C:/")).toBe("C:/");
  });
});
