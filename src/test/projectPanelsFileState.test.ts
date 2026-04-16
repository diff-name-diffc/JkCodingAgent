import { describe, expect, it } from "vitest";
import {
  deleteFromOpenFilesState,
  deleteOpenDiff,
  renameOpenDiff,
  renameOpenFilesState,
  type OpenDiff,
  type OpenFilesState,
} from "../hooks/projectPanelsFileState";

describe("project panel file state helpers", () => {
  it("重命名目录时同步所有受影响标签路径和名称", () => {
    const state: OpenFilesState = {
      tabs: [
        { id: "1", path: "/repo/src/a.ts", name: "a.ts" },
        { id: "2", path: "/repo/src/nested/b.ts", name: "b.ts" },
        { id: "3", path: "/repo/docs/readme.md", name: "readme.md" },
      ],
      activeTabId: "2",
    };

    expect(renameOpenFilesState(state, "/repo/src", "/repo/app")).toEqual({
      tabs: [
        { id: "1", path: "/repo/app/a.ts", name: "a.ts" },
        { id: "2", path: "/repo/app/nested/b.ts", name: "b.ts" },
        { id: "3", path: "/repo/docs/readme.md", name: "readme.md" },
      ],
      activeTabId: "2",
    });
  });

  it("删除目录时关闭受影响标签并回退激活标签", () => {
    const state: OpenFilesState = {
      tabs: [
        { id: "1", path: "/repo/src/a.ts", name: "a.ts" },
        { id: "2", path: "/repo/src/nested/b.ts", name: "b.ts" },
        { id: "3", path: "/repo/docs/readme.md", name: "readme.md" },
      ],
      activeTabId: "2",
    };

    expect(deleteFromOpenFilesState(state, "/repo/src")).toEqual({
      tabs: [{ id: "3", path: "/repo/docs/readme.md", name: "readme.md" }],
      activeTabId: "3",
    });
  });

  it("只在工作区文件 diff 命中时更新或清空 diff 面板", () => {
    const openDiff: OpenDiff = {
      kind: "file",
      filePath: "/repo/src/a.ts",
      staged: false,
      label: "a.ts",
    };

    expect(renameOpenDiff(openDiff, "/repo/src", "/repo/app")).toEqual({
      kind: "file",
      filePath: "/repo/app/a.ts",
      staged: false,
      label: "a.ts",
    });
    expect(deleteOpenDiff(openDiff, "/repo/src")).toBeNull();
    expect(
      renameOpenDiff(
        { kind: "commit", hash: "abc", message: "feat: test" },
        "/repo/src",
        "/repo/app",
      ),
    ).toEqual({
      kind: "commit",
      hash: "abc",
      message: "feat: test",
    });
  });
});
