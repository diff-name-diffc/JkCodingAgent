import { describe, expect, it } from "vitest";
import {
  BROWSER_TAB_ID,
  EMPTY_EDITOR_TABS,
  closeAllTabs,
  closeOtherTabs,
  closeTab,
  closeTabsToRight,
  deleteFileTab,
  diffTabId,
  fileTabId,
  fileTabs,
  workflowTabId,
  openBrowserTab,
  openDiffTab,
  openFileTab,
  openWorkflowTab,
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

describe("main-tabs 工作流标签（UI-13）", () => {
  it("workflowTabId 稳定于 sessionId，新建即激活", () => {
    const s = openWorkflowTab(EMPTY_EDITOR_TABS, "session-a", "plan-1");
    expect(s.tabs).toHaveLength(1);
    expect(s.tabs[0]).toEqual({
      id: workflowTabId("session-a"),
      kind: "workflow",
      planId: "plan-1",
      sessionId: "session-a",
      view: "canvas",
    });
    expect(s.activeTabId).toBe(workflowTabId("session-a"));
  });

  it("planId 传 null 进入列表态（会话工作流列表）", () => {
    const s = openWorkflowTab(EMPTY_EDITOR_TABS, "session-a", null);
    expect(s.tabs[0]).toEqual({
      id: workflowTabId("session-a"),
      kind: "workflow",
      planId: null,
      sessionId: "session-a",
      view: "canvas",
    });
  });

  it("view 直达详情态一级视图（列表行「结果」入口传 result）", () => {
    const s = openWorkflowTab(EMPTY_EDITOR_TABS, "session-a", "plan-1", "result");
    expect(s.tabs[0].kind === "workflow" && s.tabs[0].view).toBe("result");
  });

  it("同 planId 不同 view 视为外部导航更新 view；完全相同才幂等", () => {
    let s = openWorkflowTab(EMPTY_EDITOR_TABS, "session-a", "plan-1", "result");
    s = openWorkflowTab(s, "session-a", "plan-1", "canvas");
    expect(s.tabs[0].kind === "workflow" && s.tabs[0].view).toBe("canvas");
    expect(openWorkflowTab(s, "session-a", "plan-1", "canvas")).toBe(s);
  });

  it("同会话同 planId 重复打开幂等只激活（切视图不新建）", () => {
    let s = openWorkflowTab(EMPTY_EDITOR_TABS, "session-a", "plan-1");
    s = openFileTab(s, "/a.ts", "a.ts");
    s = openWorkflowTab(s, "session-a", "plan-1");
    expect(s.tabs).toHaveLength(2);
    expect(s.activeTabId).toBe(workflowTabId("session-a"));
  });

  it("同会话列表⇄详情切换只更新 planId，不新建标签", () => {
    let s = openWorkflowTab(EMPTY_EDITOR_TABS, "session-a", null);
    s = openWorkflowTab(s, "session-a", "plan-1");
    s = openWorkflowTab(s, "session-a", "plan-2");
    expect(s.tabs).toHaveLength(1);
    expect(s.tabs[0].kind === "workflow" && s.tabs[0].planId).toBe("plan-2");
    s = openWorkflowTab(s, "session-a", null);
    expect(s.tabs[0].kind === "workflow" && s.tabs[0].planId).toBeNull();
    expect(s.activeTabId).toBe(workflowTabId("session-a"));
  });

  it("不同会话的工作流标签互不影响", () => {
    let s = openWorkflowTab(EMPTY_EDITOR_TABS, "session-a", "plan-1");
    s = openWorkflowTab(s, "session-b", "plan-2");
    expect(s.tabs.map((t) => t.id).sort()).toEqual([
      workflowTabId("session-a"),
      workflowTabId("session-b"),
    ]);
  });

  it("关闭活动工作流标签回退右邻；混合关闭语义与文件一致", () => {
    let s = openWorkflowTab(EMPTY_EDITOR_TABS, "session-a", "plan-1");
    s = openFileTab(s, "/a.ts", "a.ts");
    s = openFileTab(s, "/b.ts", "b.ts");
    s = selectTab(s, workflowTabId("session-a"));
    const after = closeTab(s, workflowTabId("session-a"));
    expect(after.activeTabId).toBe(fileTabId("/a.ts"));
    expect(closeOtherTabs(s, workflowTabId("session-a")).tabs).toHaveLength(1);
    expect(closeTabsToRight(s, workflowTabId("session-a")).tabs.map((t) => t.id)).toEqual([
      workflowTabId("session-a"),
    ]);
  });

  it("文件重命名/删除不触碰工作流标签", () => {
    let s = openFileTab(EMPTY_EDITOR_TABS, "/old.ts", "old.ts");
    s = openWorkflowTab(s, "session-a", "plan-1");
    const renamed = renameFileTab(s, "/old.ts", "/new.ts", "new.ts");
    expect(renamed.tabs.some((t) => t.kind === "workflow" && t.planId === "plan-1")).toBe(true);
    const deleted = deleteFileTab(renamed, "/new.ts");
    expect(deleted.tabs.map((t) => t.id)).toEqual([workflowTabId("session-a")]);
    expect(deleted.activeTabId).toBe(workflowTabId("session-a"));
  });

  it("closeAll 清空工作流标签", () => {
    const s = openWorkflowTab(EMPTY_EDITOR_TABS, "session-a", "plan-1");
    expect(closeAllTabs()).toEqual(EMPTY_EDITOR_TABS);
    expect(closeTab(s, workflowTabId("session-a"))).toEqual(EMPTY_EDITOR_TABS);
  });
});

describe("main-tabs 浏览器标签（UI-18）", () => {
  it("工作区单例：id 恒为 browser，重复打开幂等激活", () => {
    const s1 = openBrowserTab(EMPTY_EDITOR_TABS);
    expect(s1.tabs).toHaveLength(1);
    expect(s1.tabs[0]).toEqual({ id: BROWSER_TAB_ID, kind: "browser", title: "浏览器" });
    expect(s1.activeTabId).toBe(BROWSER_TAB_ID);
    const s2 = openFileTab(s1, "/a.ts", "a.ts");
    const s3 = openBrowserTab(s2);
    expect(s3.tabs).toHaveLength(2);
    expect(s3.activeTabId).toBe(BROWSER_TAB_ID);
    // 已激活时原样返回（意图同步不触发无谓渲染）
    expect(openBrowserTab(s3)).toBe(s3);
  });

  it("与文件/diff/工作流标签混合：关闭回退与 closeOther 语义一致", () => {
    let s = openFileTab(EMPTY_EDITOR_TABS, "/a.ts", "a.ts");
    s = openBrowserTab(s);
    s = openWorkflowTab(s, "session-a", "plan-1");
    const closed = closeTab(s, BROWSER_TAB_ID);
    expect(closed.tabs.map((t) => t.id)).toEqual([fileTabId("/a.ts"), workflowTabId("session-a")]);
    // 关闭活动工作流标签回退右邻——browser 已不在，回退到末位
    const afterWorkflowClose = closeTab(s, workflowTabId("session-a"));
    expect(afterWorkflowClose.activeTabId).toBe(BROWSER_TAB_ID);
    expect(closeOtherTabs(s, BROWSER_TAB_ID).tabs).toHaveLength(1);
    expect(closeTabsToRight(s, BROWSER_TAB_ID).tabs.map((t) => t.id)).toEqual([
      fileTabId("/a.ts"),
      BROWSER_TAB_ID,
    ]);
  });

  it("文件重命名/删除不触碰浏览器标签", () => {
    let s = openFileTab(EMPTY_EDITOR_TABS, "/old.ts", "old.ts");
    s = openBrowserTab(s);
    const renamed = renameFileTab(s, "/old.ts", "/new.ts", "new.ts");
    expect(renamed.tabs.some((t) => t.id === BROWSER_TAB_ID)).toBe(true);
    const deleted = deleteFileTab(renamed, "/new.ts");
    expect(deleted.tabs.map((t) => t.id)).toEqual([BROWSER_TAB_ID]);
  });

  it("closeAll 清空浏览器标签", () => {
    const s = openBrowserTab(EMPTY_EDITOR_TABS);
    expect(closeAllTabs()).toEqual(EMPTY_EDITOR_TABS);
    expect(closeTab(s, BROWSER_TAB_ID)).toEqual(EMPTY_EDITOR_TABS);
  });
});
