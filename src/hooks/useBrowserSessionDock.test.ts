import { describe, expect, it } from "vitest";
import type { DockedBrowser } from "../components/BrowserDock";
import { reduceDockedBrowsers } from "./useBrowserSessionDock";

describe("reduceDockedBrowsers", () => {
  it.each(["minimized", "page_closed"])("将 %s 会话加入停靠栏", (state) => {
    const result = reduceDockedBrowsers(new Map(), {
      sessionId: "session-1",
      state,
      url: "https://example.com",
    });

    expect(result.get("session-1")).toEqual({
      sessionId: "session-1",
      state,
      url: "https://example.com",
    });
  });

  it("用最新状态覆盖同一会话", () => {
    const previous = new Map<string, DockedBrowser>([
      ["session-1", { sessionId: "session-1", state: "minimized", url: null }],
    ]);

    const result = reduceDockedBrowsers(previous, {
      sessionId: "session-1",
      state: "page_closed",
      url: null,
    });

    expect(result.get("session-1")?.state).toBe("page_closed");
    expect(result).not.toBe(previous);
  });

  it.each(["ready", "closed"])("在 %s 状态移除停靠会话", (state) => {
    const previous = new Map<string, DockedBrowser>([
      ["session-1", { sessionId: "session-1", state: "minimized", url: null }],
    ]);

    expect(reduceDockedBrowsers(previous, { sessionId: "session-1", state }).has("session-1")).toBe(
      false,
    );
  });

  it("无关状态不复制集合", () => {
    const previous = new Map<string, DockedBrowser>();

    expect(reduceDockedBrowsers(previous, { sessionId: "session-1", state: "ready" })).toBe(
      previous,
    );
  });
});
