import { describe, expect, it } from "vitest";
import { createRowUiStateStore } from "./row-ui-state";

describe("createRowUiStateStore — 行级瞬时 UI 状态存储（UI-24b-1）", () => {
  it("未写入的 key 返回 undefined（调用方回退默认值）", () => {
    const store = createRowUiStateStore();
    expect(store.get("tools:turn-1")).toBeUndefined();
  });

  it("写入后读取一致，覆盖写以最后一次为准", () => {
    const store = createRowUiStateStore();
    store.set("card:call-1", true);
    expect(store.get("card:call-1")).toBe(true);
    store.set("card:call-1", false);
    expect(store.get("card:call-1")).toBe(false);
  });

  it("false 与缺失可区分：写入 false 后 get 返回 false 而非 undefined", () => {
    const store = createRowUiStateStore();
    store.set("card:call-2", false);
    expect(store.get("card:call-2")).toBe(false);
    expect(store.get("card:call-2") ?? true).toBe(false);
  });

  it("key 相互隔离：不同行/不同卡互不串扰", () => {
    const store = createRowUiStateStore();
    store.set("tools:turn-1", true);
    store.set("tools:turn-2", false);
    expect(store.get("tools:turn-1")).toBe(true);
    expect(store.get("tools:turn-2")).toBe(false);
    expect(store.get("tools:turn-3")).toBeUndefined();
  });

  it("实例隔离：两个 store 不共享状态（多 MessageList 保活场景）", () => {
    const a = createRowUiStateStore();
    const b = createRowUiStateStore();
    a.set("card:call-1", true);
    expect(b.get("card:call-1")).toBeUndefined();
  });
});
