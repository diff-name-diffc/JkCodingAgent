import { describe, expect, it } from "vitest";
import {
  HOME_PANE_UNMOUNTED,
  nextHomePaneKeepAlive,
  type HomePaneKeepAlive,
} from "./home-view-state";

describe("nextHomePaneKeepAlive", () => {
  it("激活视图时挂载并可见", () => {
    expect(nextHomePaneKeepAlive(HOME_PANE_UNMOUNTED, true)).toEqual({
      mounted: true,
      visible: true,
    });
  });

  it("已挂载后切走 → 保活隐藏（mounted 保持，visible=false）", () => {
    const mounted: HomePaneKeepAlive = { mounted: true, visible: true };
    expect(nextHomePaneKeepAlive(mounted, false)).toEqual({
      mounted: true,
      visible: false,
    });
  });

  it("隐藏后再切回 → 恢复可见且仍为同一挂载（不重建）", () => {
    const hidden: HomePaneKeepAlive = { mounted: true, visible: false };
    expect(nextHomePaneKeepAlive(hidden, true)).toEqual({
      mounted: true,
      visible: true,
    });
  });

  it("从未挂载且未激活 → 保持 UNMOUNTED（不产生隐藏空转态）", () => {
    expect(nextHomePaneKeepAlive(HOME_PANE_UNMOUNTED, false)).toBe(
      HOME_PANE_UNMOUNTED,
    );
  });

  it("激活态重复推进幂等", () => {
    const active = nextHomePaneKeepAlive(HOME_PANE_UNMOUNTED, true);
    expect(nextHomePaneKeepAlive(active, true)).toEqual(active);
  });

  it("多次来回切换 mounted 单调不回退", () => {
    let s = HOME_PANE_UNMOUNTED;
    for (const active of [true, false, true, false, false, true]) {
      s = nextHomePaneKeepAlive(s, active);
      expect(s.mounted).toBe(true);
      expect(s.visible).toBe(active);
    }
  });
});
