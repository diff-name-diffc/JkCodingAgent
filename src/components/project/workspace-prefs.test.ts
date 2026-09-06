import { describe, expect, it } from "vitest";
import {
  DEFAULT_WORKSPACE_PREFS,
  sanitizeWorkspacePrefs,
} from "./workspace-prefs";

describe("sanitizeWorkspacePrefs", () => {
  it("空/非法输入回退默认", () => {
    expect(sanitizeWorkspacePrefs(undefined)).toEqual(DEFAULT_WORKSPACE_PREFS);
    expect(sanitizeWorkspacePrefs(null)).toEqual(DEFAULT_WORKSPACE_PREFS);
    expect(sanitizeWorkspacePrefs("garbage")).toEqual(DEFAULT_WORKSPACE_PREFS);
    expect(sanitizeWorkspacePrefs({ contextNavWidth: NaN, editorPaneRatio: "x" })).toEqual(
      DEFAULT_WORKSPACE_PREFS,
    );
  });

  it("越界值夹取到允许区间", () => {
    const out = sanitizeWorkspacePrefs({
      contextNavWidth: 900,
      rightPanelWidth: -50,
      terminalHeight: 9999,
      editorPaneRatio: 3,
    });
    expect(out.contextNavWidth).toBe(320);
    expect(out.rightPanelWidth).toBe(180);
    expect(out.terminalHeight).toBe(600);
    expect(out.editorPaneRatio).toBe(1);
  });

  it("未知页签回退 sessions，合法页签保留", () => {
    expect(sanitizeWorkspacePrefs({ contextTab: "nope" }).contextTab).toBe("sessions");
    expect(sanitizeWorkspacePrefs({ contextTab: "history" }).contextTab).toBe("history");
  });

  it("合法偏好原样保留（窄窗临时适配不污染后的读回）", () => {
    const prefs = { ...DEFAULT_WORKSPACE_PREFS, contextNavWidth: 260, terminalHeight: 300 };
    expect(sanitizeWorkspacePrefs(prefs)).toEqual(prefs);
  });
});
