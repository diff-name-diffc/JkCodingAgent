import { describe, expect, it } from "vitest";
import {
  EMPTY_EDITOR_TABS,
  closeAllTabs,
  closeOtherTabs,
  closeTab,
  closeTabsToRight,
  deleteFileTab,
  diffTabId,
  fileTabId,
  fileTabs,
  graphTabId,
  openDiffTab,
  openFileTab,
  openGraphTab,
  renameFileTab,
  selectTab,
  type OpenDiff,
} from "./main-tabs";

const diffFile = (path: string, staged = false): OpenDiff => ({
  kind: "file",
  filePath: path,
  staged,
  label: path,
});

describe("main-tabs 打开/激活", () => {
  it("同路径文件标签只激活不重复", () => {
    const s1 = openFileTab(EMPTY_EDITOR_TABS, "/a.ts", "a.ts");
    const s2 = openFileTab(s1, "/a.ts", "a.ts");
    expect(s2.tabs).toHaveLength(1);
    expect(s2.activeTabId).toBe(fileTabId("/a.ts"));
  });

  it("diff 与文件标签共存，diff 判别内容相同仅激活", () => {
    const s1 = openFileTab(EMPTY_EDITOR_TABS, "/a.ts", "a.ts");
    const s2 = openDiffTab(s1, diffFile("/b.ts"));
    const s3 = openDiffTab(s2, diffFile("/b.ts"));
    expect(s3.tabs).toHaveLength(2);
    expect(s3.activeTabId).toBe(diffTabId(diffFile("/b.ts")));
    expect(fileTabs(s3)).toHaveLength(1);
  });
});

describe("main-tabs 关闭回退", () => {
  it("关闭活动标签回退右侧邻居，越界取末位", () => {
    let s = EMPTY_EDITOR_TABS;
    s = openFileTab(s, "/a.ts", "a.ts");
    s = openFileTab(s, "/b.ts", "b.ts");
    s = openFileTab(s, "/c.ts", "c.ts");
    const after = closeTab(selectTab(s, fileTabId("/a.ts")), fileTabId("/a.ts"));
    expect(after.activeTabId).toBe(fileTabId("/b.ts"));
    const last = closeTab(selectTab(s, fileTabId("/c.ts")), fileTabId("/c.ts"));
    expect(last.activeTabId).toBe(fileTabId("/b.ts"));
  });

  it("关其他/关右侧/关全部", () => {
    let s = EMPTY_EDITOR_TABS;
    s = openFileTab(s, "/a.ts", "a.ts");
    s = openFileTab(s, "/b.ts", "b.ts");
    s = openDiffTab(s, diffFile("/c.ts"));
    expect(closeOtherTabs(s, fileTabId("/a.ts")).tabs).toHaveLength(1);
    expect(closeTabsToRight(s, fileTabId("/a.ts")).tabs.map((t) => t.id)).toEqual([
      fileTabId("/a.ts"),
    ]);
    expect(closeAllTabs()).toEqual(EMPTY_EDITOR_TABS);
  });
});

describe("main-tabs 文件树同步", () => {
  it("重命名更新文件标签与文件 diff 标签", () => {
    let s = openFileTab(EMPTY_EDITOR_TABS, "/old.ts", "old.ts");
    s = openDiffTab(s, diffFile("/old.ts"));
    const renamed = renameFileTab(s, "/old.ts", "/new.ts", "new.ts");
    expect(renamed.tabs.map((t) => t.id)).toEqual([
      fileTabId("/new.ts"),
      diffTabId(diffFile("/new.ts")),
    ]);
  });

  it("删除移除文件标签与关联 diff 标签并修正活动项", () => {
    let s = openFileTab(EMPTY_EDITOR_TABS, "/a.ts", "a.ts");
    s = openDiffTab(s, diffFile("/a.ts"));
    s = openFileTab(s, "/b.ts", "b.ts");
    const after = deleteFileTab(s, "/a.ts");
    expect(after.tabs.map((t) => t.id)).toEqual([fileTabId("/b.ts")]);
    expect(after.activeTabId).toBe(fileTabId("/b.ts"));
  });
});

describe("main-tabs 执行图标签（UI-13）", () => {
  it("graphTabId 稳定于 planId，新建即激活", () => {
    const s = openGraphTab(EMPTY_EDITOR_TABS, "plan-1", "session-a");
    expect(s.tabs).toHaveLength(1);
    expect(s.tabs[0]).toEqual({
      id: graphTabId("plan-1"),
      kind: "graph",
      planId: "plan-1",
      sessionId: "session-a",
    });
    expect(s.activeTabId).toBe(graphTabId("plan-1"));
  });

  it("同 planId 重复打开幂等只激活（切视图不新建）", () => {
    let s = openGraphTab(EMPTY_EDITOR_TABS, "plan-1", "session-a");
    s = openFileTab(s, "/a.ts", "a.ts");
    s = openGraphTab(s, "plan-1", "session-a");
    expect(s.tabs).toHaveLength(2);
    expect(s.activeTabId).toBe(graphTabId("plan-1"));
  });

  it("同会话换 planId 替换旧图标签，每会话至多一个图视图", () => {
    let s = openGraphTab(EMPTY_EDITOR_TABS, "plan-1", "session-a");
    s = openGraphTab(s, "plan-2", "session-a");
    expect(s.tabs.map((t) => t.id)).toEqual([graphTabId("plan-2")]);
    expect(s.activeTabId).toBe(graphTabId("plan-2"));
  });

  it("不同会话的图标签互不影响", () => {
    let s = openGraphTab(EMPTY_EDITOR_TABS, "plan-1", "session-a");
    s = openGraphTab(s, "plan-2", "session-b");
    expect(s.tabs.map((t) => t.id).sort()).toEqual([
      graphTabId("plan-1"),
      graphTabId("plan-2"),
    ]);
  });

  it("关闭活动图标签回退右邻；混合关闭语义与文件一致", () => {
    let s = openGraphTab(EMPTY_EDITOR_TABS, "plan-1", "session-a");
    s = openFileTab(s, "/a.ts", "a.ts");
    s = openFileTab(s, "/b.ts", "b.ts");
    s = selectTab(s, graphTabId("plan-1"));
    const after = closeTab(s, graphTabId("plan-1"));
    expect(after.activeTabId).toBe(fileTabId("/a.ts"));
    expect(closeOtherTabs(s, graphTabId("plan-1")).tabs).toHaveLength(1);
    expect(closeTabsToRight(s, graphTabId("plan-1")).tabs.map((t) => t.id)).toEqual([
      graphTabId("plan-1"),
    ]);
  });

  it("文件重命名/删除不触碰图标签", () => {
    let s = openFileTab(EMPTY_EDITOR_TABS, "/old.ts", "old.ts");
    s = openGraphTab(s, "plan-1", "session-a");
    const renamed = renameFileTab(s, "/old.ts", "/new.ts", "new.ts");
    expect(renamed.tabs.some((t) => t.kind === "graph" && t.planId === "plan-1")).toBe(true);
    const deleted = deleteFileTab(renamed, "/new.ts");
    expect(deleted.tabs.map((t) => t.id)).toEqual([graphTabId("plan-1")]);
    expect(deleted.activeTabId).toBe(graphTabId("plan-1"));
  });

  it("closeAll 清空图标签", () => {
    const s = openGraphTab(EMPTY_EDITOR_TABS, "plan-1", "session-a");
    expect(closeAllTabs()).toEqual(EMPTY_EDITOR_TABS);
    expect(closeTab(s, graphTabId("plan-1"))).toEqual(EMPTY_EDITOR_TABS);
  });
});
